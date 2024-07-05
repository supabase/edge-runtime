use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task;

use anyhow::Error;
use anyhow::{anyhow, bail};
use deno_core::OpState;

use crate::onnx::ensure_onnx_env_init;

use super::feature_extraction::FeatureExtractionPipelineInput;
use super::feature_extraction::{FeatureExtractionPipeline, FeatureExtractionResult};
use super::{Pipeline, PipelineRequest};

pub(crate) fn get_model_dir(task: String, name: Option<String>) -> PathBuf {
    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    match name {
        Some(name) => Path::new(&models_dir).join(name),
        None => Path::new(&models_dir).join("defaults").join(task),
    }
}

pub fn init_feature_extraction(
    state: &mut OpState,
    task: String,
    name: Option<String>,
) -> Result<(), Error> {
    if let Some(err) = ensure_onnx_env_init() {
        return Err(anyhow!("failed to create onnx environment: {err}"));
    }

    let model_dir = get_model_dir(task, name);

    // TODO: Use Pipeline trait with Enum to get it dynamically
    let pipeline = FeatureExtractionPipeline::init();
    state.put(pipeline.get_sender());

    #[allow(clippy::let_underscore_future)]
    let _handle = task::spawn(async move { pipeline.start_session(&model_dir).await });

    Ok(())
}

pub async fn run_feature_extraction(
    state: Rc<RefCell<OpState>>,
    _name: String,
    input: FeatureExtractionPipelineInput,
) -> Result<FeatureExtractionResult, Error> {
    let req_tx;
    {
        let op_state = state.borrow();
        let maybe_req_tx = op_state.try_borrow::<UnboundedSender<
            PipelineRequest<FeatureExtractionPipelineInput, FeatureExtractionResult>,
        >>();
        if maybe_req_tx.is_none() {
            bail!("Run init model first")
        }
        req_tx = maybe_req_tx.unwrap().clone();
    }

    let (result_tx, mut result_rx) =
        mpsc::unbounded_channel::<Result<FeatureExtractionResult, Error>>();

    req_tx.send(PipelineRequest {
        input,
        sender: result_tx,
    })?;

    let result = result_rx.recv().await;
    result.unwrap()
}
