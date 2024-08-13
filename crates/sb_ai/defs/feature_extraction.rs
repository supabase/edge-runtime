use std::borrow::Cow;
use std::vec;

use crate::serde_json::{Map, Value};
use anyhow::{anyhow, bail, Context, Error};
use deno_core::error::AnyError;
use deno_core::url::Url;
use ndarray::{Array2, Axis, Ix3};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use ort::{inputs, Session};
use serde::Deserialize;
use tokenizers::{pad_encodings, PaddingParams, Tokenizer};

use crate::pipeline::{
    try_get_model_url_from_env, try_get_tokenizer_url_from_env, PipelineBatchIO, PipelineDefinition,
};

use crate::tensor_ops::mean_pool;

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum FeatureExtractionPipelineInput {
    Single(String),
    Batch(Vec<String>),
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct FeatureExtractionPipelineInputOptions {
    pub mean_pool: bool,
    pub normalize: bool,
}

impl Default for FeatureExtractionPipelineInputOptions {
    fn default() -> Self {
        Self {
            mean_pool: true,
            normalize: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeatureExtractionPipeline;

impl PipelineDefinition for FeatureExtractionPipeline {
    type Input = PipelineBatchIO<String>;
    type InputOptions = FeatureExtractionPipelineInputOptions;
    type Output = PipelineBatchIO<Vec<f32>>;

    fn make() -> Self {
        FeatureExtractionPipeline
    }

    fn name(&self) -> Cow<'static, str> {
        "feature-extraction".into()
    }

    fn model_url(&self, _requested_variation: Option<&str>) -> Result<Url, AnyError> {
        unimplemented!()
    }

    fn tokenizer_url(&self, _requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        unimplemented!()
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        _config: Option<&Map<String, Value>>,
        input: &PipelineBatchIO<String>,
        options: Option<&FeatureExtractionPipelineInputOptions>,
    ) -> Result<<Self as PipelineDefinition>::Output, Error> {
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

        let token_type_ids = encodings
            .iter()
            .flat_map(|e| e.get_type_ids().iter().map(|v| i64::from(*v)))
            .collect::<Vec<_>>();

        let input_ids_array = Array2::from_shape_vec(input_shape, input_ids.to_owned()).unwrap();
        let attention_mask_array = Array2::from_shape_vec(input_shape, attention_mask).unwrap();
        let token_type_ids_array = Array2::from_shape_vec(input_shape, token_type_ids).unwrap();

        let outputs = session.run(inputs! {
            "input_ids" => input_ids_array,
            "token_type_ids" => token_type_ids_array,
            "attention_mask" => attention_mask_array.to_owned(),
        }?)?;

        let embeddings = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()?
            .into_dimensionality::<Ix3>()?;

        let options = options.unwrap_or(&FeatureExtractionPipelineInputOptions {
            mean_pool: true,
            normalize: true,
        });

        let embeddings = if options.mean_pool {
            mean_pool(embeddings, attention_mask_array.view().insert_axis(Axis(2)))
        } else {
            embeddings.into_owned().remove_axis(Axis(0))
        };

        let embeddings = if options.normalize {
            let (normalized, _) = normalize(embeddings, NormalizeAxis::Row);
            normalized
        } else {
            embeddings
        };

        let mut results = vec![];
        for row in embeddings.rows() {
            results.push(row.as_slice().unwrap().to_vec());
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
pub struct SupabaseGte {
    inner: FeatureExtractionPipeline,
}

impl PipelineDefinition for SupabaseGte {
    type Input = <FeatureExtractionPipeline as PipelineDefinition>::Input;
    type InputOptions = <FeatureExtractionPipeline as PipelineDefinition>::InputOptions;
    type Output = <FeatureExtractionPipeline as PipelineDefinition>::Output;

    fn make() -> Self {
        SupabaseGte {
            inner: FeatureExtractionPipeline,
        }
    }

    fn name(&self) -> Cow<'static, str> {
        "supabase-gte".into()
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError> {
        if let Some(Ok(url)) = try_get_model_url_from_env(self, requested_variation) {
            return Ok(url);
        }

        Url::parse(include_str!("repo/supabase-gte/model.txt").trim())
            .context("failed to parse URL")
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        if let Some(Ok(url)) = try_get_tokenizer_url_from_env(self, requested_variation) {
            return Some(Ok(url));
        }

        Some(
            Url::parse(include_str!("repo/supabase-gte/tokenizer.txt").trim())
                .context("failed to parse URL"),
        )
    }

    fn configure_tokenizer(&self, tokenizer: &mut Tokenizer) {
        self.inner.configure_tokenizer(tokenizer)
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, anyhow::Error> {
        self.inner.run(session, tokenizer, config, input, options)
    }
}

#[derive(Debug, Clone)]
pub struct GteSmall {
    inner: SupabaseGte,
}

impl PipelineDefinition for GteSmall {
    type Input = <SupabaseGte as PipelineDefinition>::Input;
    type InputOptions = <FeatureExtractionPipeline as PipelineDefinition>::InputOptions;
    type Output = <SupabaseGte as PipelineDefinition>::Output;

    fn make() -> Self {
        GteSmall {
            inner: SupabaseGte::make(),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        "gte-small".into()
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError> {
        if requested_variation.is_some() {
            bail!("gte-small: model variation is not supported");
        }

        self.inner.model_url(requested_variation)
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        if requested_variation.is_some() {
            Some(anyhow!("gte-small: tokenizer variation is not supported"));
        }

        self.inner.tokenizer_url(requested_variation)
    }

    fn configure_tokenizer(&self, tokenizer: &mut Tokenizer) {
        self.inner.configure_tokenizer(tokenizer)
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        self.inner.run(session, tokenizer, config, input, options)
    }
}
