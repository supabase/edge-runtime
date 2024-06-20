mod pipeline;
mod session;
pub(crate) mod tensor_ops;

use crate::pipeline::auto_pipeline::init_feature_extraction;
use crate::session::create_session;
use crate::tensor_ops::mean_pool;

use anyhow::anyhow;
use anyhow::{bail, Error};
use deno_core::error::AnyError;
use deno_core::OpState;
use deno_core::{op2, V8CrossThreadTaskSpawner, V8TaskSpawner};
use log::error;
use ndarray::{Array1, Axis, Ix3};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use once_cell::sync::Lazy;
use ort::inputs;
use pipeline::auto_pipeline::run_feature_extraction;
use pipeline::feature_extraction::FeatureExtractionPipelineInput;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::sync::mpsc::{self};
use tokio::task;

use tracing::trace_span;

deno_core::extension!(
    sb_ai,
    ops = [
        op_sb_ai_run_model,
        op_sb_ai_init_model,
        op_sb_ai_run_pipeline,
        op_sb_ai_init_pipeline
    ],
    esm_entry_point = "ext:sb_ai/js/ai.js",
    esm = [
        "js/ai.js",
        "js/util/event_stream_parser.mjs",
        "js/util/event_source_stream.mjs"
    ]
);

struct GteModelRequest {
    prompt: String,
    mean_pool: bool,
    normalize: bool,
    result_tx: mpsc::UnboundedSender<Result<Vec<f32>, Error>>,
}

fn init_gte(state: &mut OpState) -> Result<(), Error> {
    static ONNX_ENV_INIT: Lazy<Option<ort::Error>> = Lazy::new(|| {
        // Create the ONNX Runtime environment, for all sessions created in this process.
        if let Err(err) = ort::init().with_name("GTE").commit() {
            error!("sb_ai: failed to create environment - {}", err);
            return Some(err);
        }

        None
    });

    if let Some(err) = &*ONNX_ENV_INIT {
        return Err(anyhow!("failed to create onnx environment: {err}"));
    }

    let spawner = state.borrow::<V8TaskSpawner>().clone();
    let cross_thread_spawner = state.borrow::<V8CrossThreadTaskSpawner>().clone();

    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<GteModelRequest>();

    state.put::<mpsc::UnboundedSender<GteModelRequest>>(req_tx);

    spawner.spawn(move |_| {
        let session = create_session(Path::new(&models_dir).join("gte-small").join("model.onnx"));

        if session.is_err() {
            let err = session.as_ref().unwrap_err();
            error!("sb_ai: failed to create session - {}", err);
            return;
        }

        let session = session.unwrap();
        let tokenizer = Tokenizer::from_file(
            Path::new(&models_dir)
                .join("gte-small")
                .join("tokenizer.json"),
        )
        .map_err(anyhow::Error::msg);

        if tokenizer.is_err() {
            let err = tokenizer.as_ref().unwrap_err();
            error!("sb_ai: failed to create tokenizer: {}", err);
            return;
        }

        let mut tokenizer = tokenizer.unwrap();

        // model's default max length is 128. Increase it to 512.
        let truncation = tokenizer.get_truncation_mut().unwrap();

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

                let embeddings = outputs["last_hidden_state"].extract_tensor()?;
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

                cross_thread_spawner.spawn(move |_| {
                    let result = run_inference_fn(req.prompt, req.mean_pool, req.normalize);

                    if req.result_tx.send(result).is_err() {
                        error!("sb_ai: failed to send inference results (channel error)");
                    };
                });
            }
        }));
    });

    Ok(())
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

#[op2]
#[serde]
pub fn op_sb_ai_init_model(state: &mut OpState, #[string] name: String) -> Result<(), AnyError> {
    if name == "gte-small" {
        init_gte(state)
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

#[op2]
#[serde]
pub fn op_sb_ai_init_pipeline(
    state: &mut OpState,
    #[string] task: String,
    #[string] name: String,
) -> Result<(), AnyError> {
    let name = Option::from(name).filter(|name| !name.is_empty());

    match task.trim() {
        "feature-extraction" => init_feature_extraction(state, task, name),
        _ => Err(anyhow!("Not supported pipeline task: {task}")),
    }
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_run_pipeline(
    state: Rc<RefCell<OpState>>,
    #[string] task: String,
    #[string] name: String,
    #[string] prompt: String,
    mean_pool: bool,
    normalize: bool,
) -> Result<Vec<f32>, AnyError> {
    match task.trim() {
        "feature-extraction" => {
            run_feature_extraction(
                state,
                name,
                FeatureExtractionPipelineInput {
                    prompt,
                    normalize,
                    mean_pool,
                },
            )
            .await
        }
        _ => Err(anyhow!("Not supported pipeline task: {task}")),
    }
}
