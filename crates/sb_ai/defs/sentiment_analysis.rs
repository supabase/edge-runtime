use crate::{
    pipeline::{try_get_config_url_from_env, PipelineBatchIO},
    serde_json::{Map, Value},
};
use anyhow::anyhow;
use deno_core::error::AnyError;
use ndarray::{Array2, Axis};
use ort::{inputs, ArrayExtensions};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokenizers::{pad_encodings, PaddingParams};

use crate::pipeline::{
    try_get_model_url_from_env, try_get_tokenizer_url_from_env, PipelineDefinition,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SentimentAnalysisPipelineOutput {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct SentimentAnalysisPipeline;

impl PipelineDefinition for SentimentAnalysisPipeline {
    type Input = PipelineBatchIO<String>;
    type InputOptions = Value;
    type Output = PipelineBatchIO<SentimentAnalysisPipelineOutput>;

    fn make() -> Self {
        SentimentAnalysisPipeline
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "sentiment-analysis".into()
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError> {
        try_get_model_url_from_env(self, requested_variation).unwrap_or(Err(anyhow!(
            "{}: no default or variation found",
            self.name()
        )))
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        try_get_tokenizer_url_from_env(self, requested_variation)
    }

    fn config_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        try_get_config_url_from_env(self, requested_variation)
    }

    fn run(
        &self,
        session: &ort::Session,
        tokenizer: Option<&tokenizers::Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        _options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        let input_values = match input {
            PipelineBatchIO::Single(value) => vec![value.to_owned()],
            PipelineBatchIO::Batch(values) => values.to_owned(),
        };
        let mut encodings = tokenizer
            .unwrap()
            .encode_batch(input_values.to_owned(), true)
            .map_err(anyhow::Error::msg)?;

        // TODO: Encode using batch, Accept multiple inputs

        // We use it instead of overriding the Tokenizer
        pad_encodings(encodings.as_mut_slice(), &PaddingParams::default())
            .map_err(anyhow::Error::msg)?;

        let padded_token_length = encodings.first().unwrap().len();
        let input_shape = [input_values.len(), padded_token_length];

        let input_ids = encodings
            .iter()
            .flat_map(|e| e.get_ids().iter().map(|v| i64::from(*v)))
            .collect::<Vec<_>>();

        let attention_mask = encodings
            .iter()
            .flat_map(|e| e.get_attention_mask().iter().map(|v| i64::from(*v)))
            .collect::<Vec<_>>();

        let input_ids_array = Array2::from_shape_vec(input_shape, input_ids.to_owned()).unwrap();
        let attention_mask_array = Array2::from_shape_vec(input_shape, attention_mask).unwrap();

        let outputs = session.run(inputs! {
            "input_ids" => input_ids_array,
            "attention_mask" => attention_mask_array,
        }?)?;

        let predicts = outputs.get("logits").unwrap().try_extract_tensor::<f32>()?;

        let predicts = predicts.softmax(Axis(1));
        let labels = config
            .unwrap()
            .get("id2label")
            .ok_or(anyhow!("failed on get output labels"))?;

        let mut results = vec![];
        for row in predicts.rows() {
            if let Some((label, score)) = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            {
                results.push(SentimentAnalysisPipelineOutput {
                    label: labels
                        .get(label.to_string())
                        .map(|label| label.as_str().unwrap().into())
                        .unwrap(),
                    score: *score,
                })
            }
        }

        match input {
            PipelineBatchIO::Single(_) => {
                Ok(PipelineBatchIO::Single(results.first().unwrap().to_owned()))
            }
            PipelineBatchIO::Batch(_) => Ok(PipelineBatchIO::Batch(results)),
        }
    }
}
