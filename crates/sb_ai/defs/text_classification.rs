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
use std::iter;
use tokenizers::{pad_encodings, PaddingParams};

use crate::pipeline::{
    try_get_model_url_from_env, try_get_tokenizer_url_from_env, PipelineDefinition,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TextClassificationOutput {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct TextClassificationPipeline;

impl PipelineDefinition for TextClassificationPipeline {
    type Input = PipelineBatchIO<String>;
    type InputOptions = Value;
    type Output = PipelineBatchIO<TextClassificationOutput>;

    fn make() -> Self {
        TextClassificationPipeline
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "text-classification".into()
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

        let input_ids_array = Array2::from_shape_vec(input_shape, input_ids.to_owned())?;
        let attention_mask_array = Array2::from_shape_vec(input_shape, attention_mask)?;

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
                results.push(TextClassificationOutput {
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

#[derive(Debug, Clone)]
pub struct SentimentAnalysis {
    inner: TextClassificationPipeline,
}

impl PipelineDefinition for SentimentAnalysis {
    type Input = <TextClassificationPipeline as PipelineDefinition>::Input;
    type InputOptions = <TextClassificationPipeline as PipelineDefinition>::InputOptions;
    type Output = <TextClassificationPipeline as PipelineDefinition>::Output;

    fn make() -> Self {
        SentimentAnalysis {
            inner: TextClassificationPipeline,
        }
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "sentiment-analysis".into()
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError> {
        self.inner.model_url(requested_variation)
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        self.inner.tokenizer_url(requested_variation)
    }

    fn config_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        self.inner.config_url(requested_variation)
    }

    fn run(
        &self,
        session: &ort::Session,
        tokenizer: Option<&tokenizers::Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        self.inner.run(session, tokenizer, config, input, options)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroShotClassificationInput {
    pub text: String,
    pub candidate_labels: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ZeroShotClassificationOptions {
    pub multi_label: bool,
    pub hypothesis_template: String,
}

impl Default for ZeroShotClassificationOptions {
    fn default() -> Self {
        Self {
            hypothesis_template: String::from("This example is {}."),
            multi_label: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ZeroShotClassificationPipeline;

impl PipelineDefinition for ZeroShotClassificationPipeline {
    type Input = ZeroShotClassificationInput;
    type InputOptions = ZeroShotClassificationOptions;
    type Output = Vec<TextClassificationOutput>;

    fn make() -> Self {
        ZeroShotClassificationPipeline
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "zero-shot-classification".into()
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
        _config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        let options = options
            .cloned()
            .unwrap_or(ZeroShotClassificationOptions::default());

        // Pose sequence as NLI premise and label as a hypothesis
        let input_values = iter::repeat(input.text.to_owned())
            .zip(
                input
                    .candidate_labels
                    .iter()
                    .map(|label| options.hypothesis_template.replace("{}", label)),
            )
            .collect::<Vec<_>>();

        let mut encodings = tokenizer
            .unwrap()
            .encode_batch(input_values, true)
            .map_err(anyhow::Error::msg)?;

        pad_encodings(encodings.as_mut_slice(), &PaddingParams::default()).unwrap();

        let padded_token_length = encodings.first().unwrap().len();
        let input_shape = [input.candidate_labels.len(), padded_token_length];

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

        let input_tensors = inputs! {
            "input_ids" =>  input_ids_array ,
            "attention_mask" => attention_mask_array
        }?;

        let outputs = session.run(input_tensors)?;
        let outputs = outputs.get("logits").unwrap().try_extract_tensor::<f32>()?;
        let mut outputs = outputs.into_owned();

        // We throw away "neutral" (dim 1) and take the probability of
        // "entailment" (2) as the probability of the label being true
        outputs.remove_index(Axis(1), 1);

        let predicts = if options.multi_label {
            // Softmax over the Entailment vs. Contradiction dim for each label independently
            outputs.softmax(Axis(1)).index_axis(Axis(1), 1).into_owned()
        } else {
            // Softmax the "entailment" logits over all candidate labels
            outputs.index_axis(Axis(1), 1).softmax(Axis(0))
        };

        let mut results = predicts
            .iter()
            .enumerate()
            .map(|(idx, score)| TextClassificationOutput {
                label: input.candidate_labels.get(idx).unwrap().to_owned(),
                score: score.to_owned(),
            })
            .collect::<Vec<_>>();

        // Consider using `BinaryHeap<_>` while `collect` instead.
        results.sort_by(|item, other| item.score.total_cmp(&other.score).reverse());

        Ok(results)
    }
}
