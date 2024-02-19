use anyhow::{bail, Error};
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use ndarray::{Array1, Array2, Axis, Ix2};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use ort::{inputs, GraphOptimizationLevel, Session, Tensor};
use std::path::Path;
use tokenizers::normalizers::bert::BertNormalizer;
use tokenizers::Tokenizer;

deno_core::extension!(
    sb_ai,
    ops = [op_sb_ai_run_model, op_sb_ai_init_model],
    esm_entry_point = "ext:sb_ai/ai.js",
    esm = ["ai.js",]
);

fn init_gte(state: &mut OpState) -> Result<(), Error> {
    // Create the ONNX Runtime environment, for all sessions created in this process.
    ort::init().with_name("GTE").commit()?;

    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Disable)?
        .with_intra_threads(1)?
        .with_model_from_file(
            Path::new(&models_dir)
                .join("gte")
                .join("gte_small_quantized.onnx"),
        )?;

    let tokenizer = Tokenizer::from_file(
        Path::new(&models_dir)
            .join("gte")
            .join("gte_small_tokenizer.json"),
    )
    .map_err(anyhow::Error::msg)?;

    state.put::<Session>(session);
    state.put::<Tokenizer>(tokenizer);

    Ok(())
}

fn run_gte(state: &mut OpState, prompt: String) -> Result<Vec<f32>, Error> {
    let session = state.try_borrow::<Session>();
    if session.is_none() {
        bail!("inference session not available. init model first")
    }
    let session = session.unwrap();

    // Load the tokenizer and encode the prompt into a sequence of tokens.
    let tokenizer = state.try_borrow::<Tokenizer>();
    if tokenizer.is_none() {
        bail!("tokenizer not available. init model first")
    }
    let mut tokenizer = tokenizer.unwrap().clone();

    let tokenizer_impl = tokenizer
        .with_normalizer(BertNormalizer::default())
        .with_padding(None)
        .with_truncation(None)
        .map_err(anyhow::Error::msg)?;

    let tokens = tokenizer_impl
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

    let slice = normalized.view().to_slice().unwrap().to_vec();

    Ok(slice)
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

#[op2]
#[serde]
pub fn op_sb_ai_run_model(
    state: &mut OpState,
    #[string] name: String,
    #[string] prompt: String,
) -> Result<Vec<f32>, AnyError> {
    if name == "gte-small" {
        run_gte(state, prompt)
    } else {
        bail!("model not supported")
    }
}
