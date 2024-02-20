use anyhow::{bail, Error};
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use ndarray::{Array1, Array2, Axis, Ix2};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use ort::{inputs, GraphOptimizationLevel, Session, Tensor};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use tokenizers::normalizers::bert::BertNormalizer;
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
    result_tx: mpsc::UnboundedSender<Vec<f32>>,
}

fn init_gte(state: &mut OpState) -> Result<(), Error> {
    // Create the ONNX Runtime environment, for all sessions created in this process.
    ort::init().with_name("GTE").commit()?;

    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<GteModelRequest>();
    state.put::<mpsc::UnboundedSender<GteModelRequest>>(req_tx);

    #[allow(clippy::let_underscore_future)]
    let _: task::JoinHandle<Result<(), Error>> = task::spawn(async move {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Disable)?
            .with_intra_threads(1)?
            .with_model_from_file(
                Path::new(&models_dir)
                    .join("gte")
                    .join("gte_small_quantized.onnx"),
            )?;

        let mut tokenizer = Tokenizer::from_file(
            Path::new(&models_dir)
                .join("gte")
                .join("gte_small_tokenizer.json"),
        )
        .map_err(anyhow::Error::msg)?;

        let tokenizer_impl = tokenizer
            .with_normalizer(BertNormalizer::default())
            .with_padding(None)
            .with_truncation(None)
            .map_err(anyhow::Error::msg)?;

        loop {
            let req = req_rx.recv().await;
            if req.is_none() {
                break;
            }
            let req = req.unwrap();

            let tokens = tokenizer_impl
                .encode(req.prompt, true)
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

            let result = normalized.view().to_slice().unwrap().to_vec();
            req.result_tx.send(result)?;
        }
        Ok(())
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

    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<Vec<f32>>();

    req_tx.send(GteModelRequest {
        prompt,
        result_tx: result_tx.clone(),
    })?;

    let result = result_rx.recv().await;
    Ok(result.unwrap())
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
