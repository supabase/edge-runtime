pub mod feature_extraction;

use anyhow::anyhow;
use anyhow::{bail, Error};
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use log::error;
use ndarray::{Array1, Array2, ArrayView3, Axis, Ix3};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use once_cell::sync::Lazy;
use ort::{inputs, GraphOptimizationLevel, Session};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task;

enum PipelineType {
    FeatureExtraction,
}

struct InitPipelineOpts {
    model_path: String,
}

pub(crate) trait PipelineInput {}
pub(crate) struct PipelineRequest<I: PipelineInput, R> {
    pub input: I,
    pub sender: UnboundedSender<Result<R, Error>>,
}

pub(crate) trait Pipeline<I: PipelineInput, R> {
    fn get_channel() -> Result<
        (
            UnboundedSender<PipelineRequest<I, R>>,
            UnboundedReceiver<PipelineRequest<I, R>>,
        ),
        Error,
    >;

    fn run(session: &Session, tokenizer: &Tokenizer, input: &I) -> Result<R, Error>;
}
