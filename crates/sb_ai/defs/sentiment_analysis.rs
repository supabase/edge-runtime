use crate::{
    pipeline::try_get_config_url_from_env,
    serde_json::{Map, Value},
};
use anyhow::anyhow;
use deno_core::error::AnyError;
use ndarray::{Array1, Axis};
use ort::{inputs, ArrayExtensions};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::pipeline::{
    try_get_model_url_from_env, try_get_tokenizer_url_from_env, PipelineDefinition,
};

#[derive(Deserialize, Debug)]
pub struct SentimentAnalysisPipelineInput(String);

#[derive(Serialize, Deserialize, Debug)]
pub struct SentimentAnalysisPipelineOutput {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct SentimentAnalysisPipeline;

impl PipelineDefinition for SentimentAnalysisPipeline {
    type Input = SentimentAnalysisPipelineInput;
    type InputOptions = Value;
    type Output = Vec<SentimentAnalysisPipelineOutput>;

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
        let encoded_prompt = tokenizer
            .unwrap()
            .encode(input.0.to_owned(), true)
            .map_err(anyhow::Error::msg)?;

        // TODO: Encode using batch, Accept multiple inputs

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

        // Sentiment Analysis Shape [-1,-1] = Array2
        let input_ids = Array1::from_vec(input_ids).insert_axis(Axis(0));
        let attention_mask = Array1::from_vec(attention_mask).insert_axis(Axis(0));

        let outputs = session.run(inputs! {
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
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

        Ok(results)
    }
}
