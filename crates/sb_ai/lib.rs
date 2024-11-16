mod onnxruntime;

use anyhow::{anyhow, bail, Error};
use base_rt::BlockingScopeCPUUsageMetricExt;
use deno_core::error::AnyError;
use deno_core::OpState;
use deno_core::{op2, JsRuntime, V8CrossThreadTaskSpawner, V8TaskSpawner};
use ndarray::{Array1, Array2, ArrayView3, Axis, Ix3};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use onnxruntime::session::load_session_from_file;
use ort::inputs;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::sync::{mpsc, oneshot};
use tokio::task;

use onnxruntime::*;
use tracing::{error, trace_span};

deno_core::extension!(
    sb_ai,
    ops = [
        op_sb_ai_run_model,
        op_sb_ai_init_model,
        op_sb_ai_try_cleanup_unused_session,
        op_sb_ai_ort_init_session,
        op_sb_ai_ort_run_session,
        op_sb_ai_ort_fetch,
    ],
    esm_entry_point = "ext:sb_ai/js/ai.js",
    esm = [
        "js/ai.js",
        "js/util/event_stream_parser.mjs",
        "js/util/event_source_stream.mjs",
        "js/onnxruntime/onnx.js",
        "js/onnxruntime/cache_adapter.js"
    ]
);

struct GteModelRequest {
    prompt: String,
    mean_pool: bool,
    normalize: bool,
    result_tx: mpsc::UnboundedSender<Result<Vec<f32>, Error>>,
}

fn mean_pool(last_hidden_states: ArrayView3<f32>, attention_mask: ArrayView3<i64>) -> Array2<f32> {
    let masked_hidden_states = last_hidden_states.into_owned() * &attention_mask.mapv(|x| x as f32);
    let sum_hidden_states = masked_hidden_states.sum_axis(Axis(1));
    let sum_attention_mask = attention_mask.mapv(|x| x as f32).sum_axis(Axis(1));

    sum_hidden_states / sum_attention_mask
}

async fn init_gte(state: Rc<RefCell<OpState>>) -> Result<(), Error> {
    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let (tx, rx) = oneshot::channel::<Option<Error>>();
    let (spawner, cross_thread_spawner) = {
        let state = state.borrow_mut();
        let spawner = state.borrow::<V8TaskSpawner>().clone();
        let cross_thread_spawner = state.borrow::<V8CrossThreadTaskSpawner>().clone();

        (spawner, cross_thread_spawner)
    };

    spawner.spawn(move |scope| {
        let state = JsRuntime::op_state_from(scope);
        let mut state = state.borrow_mut();

        let mut req_rx = {
            let (req_tx, req_rx) = mpsc::unbounded_channel::<GteModelRequest>();
            let _ = state.try_take::<mpsc::UnboundedSender<GteModelRequest>>();

            state.put::<mpsc::UnboundedSender<GteModelRequest>>(req_tx);
            req_rx
        };

        let (_, session) = match load_session_from_file(
            Path::new(&models_dir).join("gte-small").join("model.onnx"),
        ) {
            Ok(session) => session,
            Err(err) => {
                error!(reason = ?err, "failed to create session");
                let _ = tx.send(Some(err));
                return;
            }
        };

        let mut tokenizer = match Tokenizer::from_file(
            Path::new(&models_dir)
                .join("gte-small")
                .join("tokenizer.json"),
        )
        .map_err(anyhow::Error::msg)
        {
            Ok(tokenizer) => tokenizer,
            Err(err) => {
                error!(reason = ?err, "failed to create tokenizer");
                let _ = tx.send(Some(err));
                return;
            }
        };

        // model's default max length is 128. Increase it to 512.
        let Some(truncation) = tokenizer.get_truncation_mut() else {
            let err = anyhow!("failed to get mutable truncation parameter");
            error!(reason = ?err);
            let _ = tx.send(Some(err));
            return;
        };

        truncation.max_length = 512;

        let run_inference = Arc::new(
            move |prompt: String,
                  do_mean_pooling: bool,
                  do_normalize: bool|
                  -> Result<Vec<f32>, Error> {
                let encoded_prompt = tokenizer.encode(prompt, true).map_err(anyhow::Error::msg)?;
                let input_ids = encoded_prompt
                    .get_ids()
                    .iter()
                    .map(|i| *i as i64)
                    .collect::<Vec<_>>();

                let attention_mask = encoded_prompt
                    .get_attention_mask()
                    .iter()
                    .map(|i| *i as i64)
                    .collect::<Vec<_>>();

                let token_type_ids = encoded_prompt
                    .get_type_ids()
                    .iter()
                    .map(|i| *i as i64)
                    .collect::<Vec<_>>();

                let input_ids_array = Array1::from_iter(input_ids.iter().cloned());
                let input_ids_array = input_ids_array.view().insert_axis(Axis(0));

                let attention_mask_array = Array1::from_iter(attention_mask.iter().cloned());
                let attention_mask_array = attention_mask_array.view().insert_axis(Axis(0));

                let token_type_ids_array = Array1::from_iter(token_type_ids.iter().cloned());
                let token_type_ids_array = token_type_ids_array.view().insert_axis(Axis(0));

                let outputs = trace_span!("infer_gte").in_scope(|| {
                    session.run(inputs! {
                    "input_ids" => input_ids_array,
                    "token_type_ids" => token_type_ids_array,
                    "attention_mask" => attention_mask_array,
                    }?)
                })?;

                let embeddings = outputs["last_hidden_state"].try_extract_tensor()?;
                let embeddings = embeddings.into_dimensionality::<Ix3>()?;

                let result = if do_mean_pooling {
                    mean_pool(embeddings, attention_mask_array.insert_axis(Axis(2)))
                } else {
                    embeddings.into_owned().remove_axis(Axis(0))
                };

                let result = if do_normalize {
                    let (normalized, _) = normalize(result, NormalizeAxis::Row);
                    normalized
                } else {
                    result
                };

                Ok(result.view().to_slice().unwrap().to_vec())
            },
        );

        drop(task::spawn(async move {
            loop {
                let run_inference_fn = run_inference.clone();
                let req = req_rx.recv().await;

                if req.is_none() {
                    break;
                }

                let req = req.unwrap();

                cross_thread_spawner.spawn(move |state| {
                    JsRuntime::op_state_from(state)
                        .borrow_mut()
                        .spawn_cpu_accumul_blocking_scope(move || {
                            let result = run_inference_fn(req.prompt, req.mean_pool, req.normalize);

                            if let Err(err) = req.result_tx.send(result) {
                                error!(reason = ?err, "failed to send inference results");
                            };
                        });
                });
            }
        }));

        let _ = tx.send(None);
    });

    let Some(err) = rx.await.map_err(AnyError::from)? else {
        return Ok(());
    };

    Err(err)
}

async fn run_gte(
    state: Rc<RefCell<OpState>>,
    prompt: String,
    mean_pool: bool,
    normalize: bool,
) -> Result<Vec<f32>, Error> {
    let req_tx;
    {
        let op_state = state.borrow();
        let maybe_req_tx = op_state.try_borrow::<mpsc::UnboundedSender<GteModelRequest>>();
        if maybe_req_tx.is_none() {
            bail!("Run init model first")
        }
        req_tx = maybe_req_tx.unwrap().clone();
    }

    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<Result<Vec<f32>, Error>>();

    req_tx.send(GteModelRequest {
        prompt,
        mean_pool,
        normalize,
        result_tx: result_tx.clone(),
    })?;

    let result = result_rx.recv().await;
    result.unwrap()
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_init_model(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
) -> Result<(), AnyError> {
    if name == "gte-small" {
        init_gte(state).await
    } else {
        bail!("model not supported")
    }
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_run_model(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[string] prompt: String,
    mean_pool: bool,
    normalize: bool,
) -> Result<Vec<f32>, AnyError> {
    if name == "gte-small" {
        run_gte(state, prompt, mean_pool, normalize).await
    } else {
        bail!("model not supported")
    }
}

#[op2(fast)]
#[bigint]
pub fn op_sb_ai_try_cleanup_unused_session() -> Result<usize, anyhow::Error> {
    session::cleanup()
}
