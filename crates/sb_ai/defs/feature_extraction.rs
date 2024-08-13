use std::borrow::Cow;

use crate::serde_json::{Map, Value};
use anyhow::{anyhow, bail, Context, Error};
use deno_core::error::AnyError;
use deno_core::url::Url;
use ndarray::{Array1, Axis, Ix3};
use ndarray_linalg::norm::{normalize, NormalizeAxis};
use ort::{inputs, Session};
use serde::Deserialize;
use tokenizers::Tokenizer;

use crate::pipeline::{
    try_get_model_url_from_env, try_get_tokenizer_url_from_env, PipelineDefinition,
};

use crate::tensor_ops::mean_pool;

#[derive(Deserialize, Debug)]
pub struct FeatureExtractionPipelineInput(String);

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
    type Input = FeatureExtractionPipelineInput;
    type InputOptions = FeatureExtractionPipelineInputOptions;
    type Output = Vec<f32>;

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
        input: &FeatureExtractionPipelineInput,
        options: Option<&FeatureExtractionPipelineInputOptions>,
    ) -> Result<<Self as PipelineDefinition>::Output, Error> {
        let encoded_prompt = tokenizer
            .unwrap()
            .encode(input.0.to_owned(), true)
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

        let embeddings = outputs["last_hidden_state"].try_extract_tensor()?;
        let embeddings = embeddings.into_dimensionality::<Ix3>()?;

        let options = options.unwrap_or(&FeatureExtractionPipelineInputOptions {
            mean_pool: true,
            normalize: true,
        });

        let result = if options.mean_pool {
            mean_pool(embeddings, attention_mask_array.insert_axis(Axis(2)))
        } else {
            embeddings.into_owned().remove_axis(Axis(0))
        };

        let result = if options.normalize {
            let (normalized, _) = normalize(result, NormalizeAxis::Row);
            normalized
        } else {
            result
        };

        Ok(result.view().to_slice().unwrap().to_vec())
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
