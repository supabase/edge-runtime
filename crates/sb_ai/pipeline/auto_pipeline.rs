use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task;

use anyhow::Error;
use anyhow::{anyhow, bail};
use deno_core::OpState;
use log::error;
use once_cell::sync::Lazy;

use super::feature_extraction::FeatureExtractionPipelineInput;
use super::feature_extraction::{FeatureExtractionPipeline, FeatureExtractionResult};
use super::{Pipeline, PipelineRequest};

pub(crate) fn get_onnx_env() -> Lazy<Option<ort::Error>> {
    Lazy::new(|| {
        // Create the ONNX Runtime environment, for all sessions created in this process.
        // TODO: Add CUDA execution provider
        if let Err(err) = ort::init().with_name("SB_AI_ONNX").commit() {
            error!("sb_ai: failed to create environment - {}", err);
            return Some(err);
        }

        None
    })
}

pub(crate) fn get_model_dir(task: String, name: Option<String>) -> PathBuf {
    let models_dir = std::env::var("SB_AI_MODELS_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    match name {
        Some(name) => Path::new(&models_dir).join(&name),
        None => Path::new(&models_dir).join("defaults").join(&task),
    }
}

pub fn init_feature_extraction(
    state: &mut OpState,
    task: String,
    name: Option<String>,
) -> Result<(), Error> {
    if let Some(err) = &*get_onnx_env() {
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
/*
use core::panic;
use std::future::IntoFuture;
use std::path::{Path, PathBuf};
use tokio::task;

use anyhow::anyhow;
use anyhow::{bail, Error};
use deno_core::futures::FutureExt;
use deno_core::OpState;
use tokio::task::JoinHandle;

use super::feature_extraction::{FeatureExtractionPipeline, FeatureExtractionResult};
use super::{Pipeline, PipelineCommon};

enum PipelineType {
    FeatureExtraction(FeatureExtractionPipeline),
    OtherPipeline(),
}

impl PipelineType {
    fn put_into_state(&self, state: &mut OpState) {
        match self {
            PipelineType::FeatureExtraction(pipeline) => state.put(pipeline.get_sender()),
            _ => panic!(),
        }
    }

    fn inner(&self) -> Box<&impl PipelineCommon> {
        match self {
            PipelineType::FeatureExtraction(pipeline) => Box::new(pipeline.to_owned()),
            _ => panic!(),
        }
    }
}

pub(crate) struct AutoPipeline {
    task: String,
    //pipeline: Box<dyn PipelineCommon>,
}

pub fn auto_pipeline(task: String, model_name: Option<String>) -> Result<(), Error> {
    let pipeline: Box<dyn PipelineCommon> = match task.trim() {
        "feature-extraction" => Box::new(FeatureExtractionPipeline::init()),
        _ => return Err(anyhow!("Not supported pipeline task: {task}")),
    };

    let model_dir = std::env::var("SB_AI_MODEL_DIR").unwrap_or("/etc/sb_ai/models".to_string());

    let model_dir = match model_name {
        Some(name) => Path::new(&model_dir).join(&name),
        None => Path::new(&model_dir).join("defaults").join(""),
    };

    //state.put(pipeline.get_sender());

    #[allow(clippy::let_underscore_future)]
    let _handle = task::spawn(async move { pipeline.start_session(&model_dir).await });

    Ok(())
}
/*
impl AutoPipeline {
    pub fn init(task: String) -> Result<Self, Error> {
        let pipeline = match task.trim() {
            "feature-extraction" => FeatureExtractionPipeline::init(),
            _ => return Err(anyhow!("Not supported pipeline task: {task}")),
        };

        Ok(Self {
            task,
            pipeline: Box::new(pipeline),
        })
    }

    pub fn load(&self, pipeline: PipelineType, model_name: Option<String>) -> Result<(), Error> {
        let model_dir = std::env::var("SB_AI_MODEL_DIR").unwrap_or("/etc/sb_ai/models".to_string());

        let model_dir = match model_name {
            Some(name) => Path::new(&model_dir).join(&name),
            None => Path::new(&model_dir).join("defaults").join(""),
        };

        let pipeline = self.pipeline.inner();
        //state.put(pipeline.get_sender());

        #[allow(clippy::let_underscore_future)]
        let _handle = task::spawn(async move { pipeline.start_session(&model_dir).await });

        Ok(())
    }
}*/
*/
