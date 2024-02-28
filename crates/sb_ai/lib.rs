use anyhow::{bail, Error};
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use log::error;
use ndarray::{Array1, Array2, Axis, Ix2};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use ort::{inputs, GraphOptimizationLevel, Session, Tensor};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tokenizers::normalizers::bert::BertNormalizer;
use tokenizers::utils::padding::{PaddingDirection, PaddingParams, PaddingStrategy};
use tokenizers::utils::truncation::{TruncationDirection, TruncationParams, TruncationStrategy};
use tokenizers::Tokenizer;
use tokio::sync::mpsc;
use tokio::task;

deno_core::extension!(
    sb_ai,
    ops = [op_sb_ai_run_model, op_sb_ai_init_model],
    esm_entry_point = "ext:sb_ai/ai.js",
    esm = ["ai.js",]
);

struct GteModelRequest {
    prompt: String,
    result_tx: mpsc::UnboundedSender<Result<Vec<f32>, Error>>,
}

fn create_session(model_file_path: PathBuf) -> Result<Session, Error> {
    let session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(1)?
        .with_model_from_file(model_file_path)?;

    Ok(session)
}

fn init_gte(state: &mut OpState) -> Result<(), Error> {
    // Create the ONNX Runtime environment, for all sessions created in this process.
    ort::init().with_name("GTE").commit()?;

    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<GteModelRequest>();
    state.put::<mpsc::UnboundedSender<GteModelRequest>>(req_tx);

    #[allow(clippy::let_underscore_future)]
    let _handle: task::JoinHandle<()> = task::spawn(async move {
        let session = create_session(
            Path::new(&models_dir)
                .join("gte")
                .join("gte_small_quantized.onnx"),
        );
        if session.is_err() {
            let err = session.as_ref().unwrap_err();
            error!("sb_ai: failed to create session - {}", err);
            return ();
        }
        let session = session.unwrap();

        let tokenizer = Tokenizer::from_file(
            Path::new(&models_dir)
                .join("gte")
                .join("gte_small_tokenizer.json"),
        )
        .map_err(anyhow::Error::msg);
        if tokenizer.is_err() {
            let err = tokenizer.as_ref().unwrap_err();
            error!("sb_ai: failed to create tokenizer - {}", err);
            return ();
        }
        let mut tokenizer = tokenizer.unwrap();

        //let mut bert_normalizer = BertNormalizer::default();
        //bert_normalizer.clean_text = true;
        //bert_normalizer.handle_chinese_chars = true;
        //bert_normalizer.strip_accents = None;
        //bert_normalizer.lowercase = true;

        //let mut padding_params = PaddingParams::default();
        ////padding_params.strategy = PaddingStrategy::Fixed(128);
        //padding_params.direction = PaddingDirection::Right;
        //padding_params.pad_to_multiple_of = None;
        //padding_params.pad_id = 0;
        //padding_params.pad_type_id = 0;
        //padding_params.pad_token = "[PAD]".to_string();

        //let tokenizer_impl = tokenizer
        //    .with_normalizer(bert_normalizer)
        //    .with_padding(Some(padding_params))
        //    .with_truncation(Some(TruncationParams {
        //        direction: TruncationDirection::Right,
        //        stride: 0,
        //        max_length: 128,
        //        strategy: TruncationStrategy::LongestFirst,
        //    }))
        //    .map_err(anyhow::Error::msg);
        //if tokenizer_impl.is_err() {
        //    let err = tokenizer_impl.as_ref().unwrap_err();
        //    error!("sb_ai: failed to initialize tokenizer - {}", err);
        //    return ();
        //}
        //let tokenizer_impl = tokenizer_impl.unwrap();

        let run_inference = move |prompt: String| -> Result<Vec<f32>, Error> {
            let tokens = tokenizer
                .encode(prompt, true)
                .map_err(anyhow::Error::msg)?
                .get_ids()
                .iter()
                .map(|i| *i as i64)
                .collect::<Vec<_>>();

            let tokens = Array1::from_iter(tokens.iter().cloned());
            let array = tokens.view().insert_axis(Axis(0));

            let dims = array.raw_dim();
            let token_type_ids = Array2::<i64>::zeros(dims);
            let attention_mask = Array2::<i64>::ones(dims);
            let outputs = session.run(inputs! {
                "input_ids" => array,
                "token_type_ids" => token_type_ids,
                "attention_mask" => attention_mask,
            }?)?;

            let embeddings: Tensor<f32> = outputs["last_hidden_state"].extract_tensor()?;

            let embeddings_view = embeddings.view();
            let mean_pool = embeddings_view.mean_axis(Axis(1)).unwrap();
            let (normalized, _) = normalize(
                mean_pool.into_dimensionality::<Ix2>().unwrap(),
                NormalizeAxis::Row,
            );

            Ok(normalized.view().to_slice().unwrap().to_vec())
        };

        loop {
            let req = req_rx.recv().await;
            if req.is_none() {
                break;
            }
            let req = req.unwrap();

            let result = run_inference(req.prompt);
            if req.result_tx.send(result).is_err() {
                error!("sb_ai: failed to send inference results (channel error)");
            };
        }
        return ();
    });

    Ok(())
}

async fn run_gte(state: Rc<RefCell<OpState>>, prompt: String) -> Result<Vec<f32>, Error> {
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
) -> Result<Vec<f32>, AnyError> {
    if name == "gte-small" {
        run_gte(state, prompt).await
    } else {
        bail!("model not supported")
    }
}
