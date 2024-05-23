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

use crate::tensor_ops::mean_pool;

use super::{InitPipelineOpts, Pipeline, PipelineInput, PipelineRequest};

pub type FeatureExtractionResult = Vec<f32>;

pub(crate) struct FeatureExtractionPipelineInput {
    pub prompt: String,
    pub mean_pool: bool,
    pub normalize: bool,
}
impl PipelineInput for FeatureExtractionPipelineInput {}

pub struct FeatureExtractionPipeline;
impl Pipeline<FeatureExtractionPipelineInput, FeatureExtractionResult>
    for FeatureExtractionPipeline
{
    fn get_channel() -> Result<
        (
            UnboundedSender<
                PipelineRequest<FeatureExtractionPipelineInput, FeatureExtractionResult>,
            >,
            UnboundedReceiver<
                PipelineRequest<FeatureExtractionPipelineInput, FeatureExtractionResult>,
            >,
        ),
        Error,
    > {
        let channel = mpsc::unbounded_channel();

        Ok(channel)
    }
    fn run(
        session: &Session,
        tokenizer: &Tokenizer,
        input: &FeatureExtractionPipelineInput,
    ) -> Result<FeatureExtractionResult, Error> {
        let encoded_prompt = tokenizer
            .encode(input.prompt.to_owned(), true)
            .map_err(anyhow::Error::msg)?;

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

        let outputs = session.run(inputs! {
            "input_ids" => input_ids_array,
            "token_type_ids" => token_type_ids_array,
            "attention_mask" => attention_mask_array,
        }?)?;

        let embeddings = outputs["last_hidden_state"].extract_tensor()?;
        let embeddings = embeddings.into_dimensionality::<Ix3>()?;

        let result = if input.mean_pool {
            mean_pool(embeddings, attention_mask_array.insert_axis(Axis(2)))
        } else {
            embeddings.into_owned().remove_axis(Axis(0))
        };

        let result = if input.normalize {
            let (normalized, _) = normalize(result, NormalizeAxis::Row);
            normalized
        } else {
            result
        };

        Ok(result.view().to_slice().unwrap().to_vec())
    }
}
