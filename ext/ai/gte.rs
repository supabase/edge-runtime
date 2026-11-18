//! `gte-small` sentence embeddings, run on the pure-Rust `tract` engine.
//!
//! This uses the same ONNX model file and the same pre/post-processing as
//! the previous ONNX Runtime based implementation, so embeddings stay
//! compatible with vectors produced before.

use std::num::FpCategory;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use anyhow::Error;
use tokenizers::Tokenizer;
use tokio::sync::Mutex;
use tracing::debug;
use tracing::instrument;
use tracing::trace_span;
use tract_onnx::prelude::*;

use crate::download::fetch_pinned;
use crate::download::PinnedFile;

const HF_REVISION: &str = "93b36ff09519291b77d6000d2e86bd8565378086";

const MODEL: PinnedFile = PinnedFile {
  kind: "model",
  cache_id: "gte-small-93b36ff09519291b77d6000d2e86bd8565378086",
  url: "https://huggingface.co/Supabase/gte-small/resolve/93b36ff09519291b77d6000d2e86bd8565378086/onnx/model.onnx",
  sha256: "398a29991324e0b383afa13375d681ced3079c83e097fb1ebd9290d7498523b3",
};

const TOKENIZER: PinnedFile = PinnedFile {
  kind: "tokenizer",
  cache_id: "gte-small-93b36ff09519291b77d6000d2e86bd8565378086",
  url: "https://huggingface.co/Supabase/gte-small/resolve/93b36ff09519291b77d6000d2e86bd8565378086/tokenizer.json",
  sha256: "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
};

/// The model's default max length is 128; we allow up to 512 tokens.
const MAX_TOKENS: usize = 512;

const OUTPUT_NAME: &str = "last_hidden_state";

type Plan = TypedRunnableModel<TypedModel>;

#[derive(Clone, Copy)]
enum Input {
  Ids,
  AttentionMask,
  TokenTypeIds,
}

pub struct GteSmall {
  plan: Plan,
  inputs: Vec<Input>,
  /// The tokenizer as configured upstream: pads inputs to 128 tokens.
  tokenizer: Tokenizer,
  /// Same tokenizer without padding, used when mean pooling. Padding is
  /// masked out of attention and pooling, so it would only cost CPU time.
  unpadded_tokenizer: Tokenizer,
}

/// The process-wide model. Workers hold an `Arc` to it; it is dropped by
/// [`cleanup`] once no worker references it anymore.
static MODEL_CACHE: Mutex<Option<Arc<GteSmall>>> = Mutex::const_new(None);

/// Returns the shared model, loading it on first use.
pub async fn load() -> Result<Arc<GteSmall>, Error> {
  let mut cache = MODEL_CACHE.lock().await;

  if let Some(model) = cache.as_ref() {
    return Ok(model.clone());
  }

  let (model_bytes, tokenizer_bytes) =
    tokio::try_join!(fetch_pinned(&MODEL), fetch_pinned(&TOKENIZER))?;

  let model = tokio::task::spawn_blocking(move || {
    GteSmall::new(&model_bytes, &tokenizer_bytes)
  })
  .await
  .context("failed to initialize gte-small")??;

  let model = Arc::new(model);
  debug!(revision = HF_REVISION, "loaded gte-small");
  *cache = Some(model.clone());

  Ok(model)
}

/// Drops the shared model if no worker is using it. Returns the number of
/// models released.
pub async fn cleanup() -> usize {
  let mut cache = MODEL_CACHE.lock().await;

  match cache.as_ref() {
    Some(model) if Arc::strong_count(model) == 1 => {
      *cache = None;
      1
    }
    _ => 0,
  }
}

impl GteSmall {
  fn new(model_bytes: &[u8], tokenizer_bytes: &[u8]) -> Result<Self, Error> {
    let mut model = tract_onnx::onnx()
      .model_for_read(&mut &model_bytes[..])
      .context("failed to parse model")?;
    model
      .set_output_names([OUTPUT_NAME])
      .context("model has no last_hidden_state output")?;
    let plan = model
      .into_optimized()
      .context("failed to optimize model")?
      .into_runnable()
      .context("failed to prepare model")?;

    let graph = plan.model();
    let inputs = graph
      .input_outlets()?
      .iter()
      .map(|outlet| match graph.node(outlet.node).name.as_str() {
        "input_ids" => Ok(Input::Ids),
        "attention_mask" => Ok(Input::AttentionMask),
        "token_type_ids" => Ok(Input::TokenTypeIds),
        other => Err(anyhow!("unexpected model input: {other}")),
      })
      .collect::<Result<Vec<_>, _>>()?;

    let mut tokenizer =
      Tokenizer::from_bytes(tokenizer_bytes).map_err(Error::msg)?;
    let Some(truncation) = tokenizer.get_truncation_mut() else {
      bail!("failed to get mutable truncation parameter");
    };
    truncation.max_length = MAX_TOKENS;

    let mut unpadded_tokenizer = tokenizer.clone();
    unpadded_tokenizer.with_padding(None);

    Ok(Self {
      plan,
      inputs,
      tokenizer,
      unpadded_tokenizer,
    })
  }

  /// Embeds `prompt`.
  ///
  /// With `mean_pool`, returns one 384-dimensional vector; otherwise one per
  /// token position (including padding), flattened. With `normalize`, the
  /// whole result is scaled to unit L2 norm. Both match the previous
  /// implementation.
  #[instrument(level = "trace", skip_all)]
  pub fn embed(
    &self,
    prompt: &str,
    mean_pool: bool,
    normalize: bool,
  ) -> Result<Vec<f32>, Error> {
    let tokenizer = if mean_pool {
      &self.unpadded_tokenizer
    } else {
      &self.tokenizer
    };
    let encoding = tokenizer.encode(prompt, true).map_err(Error::msg)?;
    let len = encoding.get_ids().len();
    let attention_mask = encoding.get_attention_mask();

    let to_tensor = |values: &[u32]| -> Result<TValue, Error> {
      let values = values.iter().map(|&v| v as i64).collect::<Vec<_>>();
      Ok(tract_ndarray::Array2::from_shape_vec((1, len), values)?.into_tvalue())
    };

    let inputs = self
      .inputs
      .iter()
      .map(|input| match input {
        Input::Ids => to_tensor(encoding.get_ids()),
        Input::AttentionMask => to_tensor(attention_mask),
        Input::TokenTypeIds => to_tensor(encoding.get_type_ids()),
      })
      .collect::<Result<TVec<_>, _>>()?;

    let outputs =
      trace_span!("infer_gte").in_scope(|| self.plan.run(inputs))?;
    let hidden = outputs[0]
      .to_array_view::<f32>()?
      .into_dimensionality::<tract_ndarray::Ix3>()?;
    let hidden = hidden.index_axis(tract_ndarray::Axis(0), 0);
    let dims = hidden.shape()[1];

    let mut result = if mean_pool {
      let mut sum = vec![0f32; dims];
      let mut count = 0f32;
      for (token, &mask) in hidden.outer_iter().zip(attention_mask) {
        let mask = mask as f32;
        count += mask;
        for (acc, value) in sum.iter_mut().zip(token.iter()) {
          *acc += value * mask;
        }
      }
      sum.iter_mut().for_each(|value| *value /= count);
      sum
    } else {
      hidden.iter().copied().collect()
    };

    if normalize {
      // Accumulate in f64 so long unpooled outputs normalize accurately.
      let l2 = result
        .iter()
        .map(|&value| f64::from(value) * f64::from(value))
        .sum::<f64>()
        .sqrt();
      if l2.classify() != FpCategory::Zero {
        result
          .iter_mut()
          .for_each(|value| *value = (f64::from(*value) / l2) as f32);
      }
    }

    Ok(result)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Embeddings must stay compatible with vectors users stored while
  /// gte-small ran on ONNX Runtime.
  #[tokio::test]
  async fn matches_onnx_runtime_reference() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
      "testdata/gte_small_ort_reference.json"
    ))
    .unwrap();
    let model = load().await.unwrap();

    let inputs = reference["inputs"].as_array().unwrap();
    let expected = reference["embeddings"].as_array().unwrap();

    for (input, expected) in inputs.iter().zip(expected) {
      let expected = expected
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect::<Vec<_>>();
      let actual = model.embed(input.as_str().unwrap(), true, true).unwrap();

      assert_eq!(actual.len(), 384);

      let max_diff = actual
        .iter()
        .zip(&expected)
        .map(|(a, e)| (a - e).abs())
        .fold(0f32, f32::max);
      let cosine = actual
        .iter()
        .zip(&expected)
        .map(|(a, e)| a * e)
        .sum::<f32>();

      assert!(max_diff <= 1e-4, "max abs diff {max_diff} for {input}");
      assert!(cosine >= 0.99999, "cosine {cosine} for {input}");
    }
  }

  #[tokio::test]
  async fn unpooled_output_is_per_token_and_globally_normalized() {
    let model = load().await.unwrap();
    let tokens = model.tokenizer.encode("meow", true).unwrap().len();
    // Unpooled output keeps the upstream padding, as before.
    assert_eq!(tokens, 128);

    let raw = model.embed("meow", false, false).unwrap();
    assert_eq!(raw.len(), tokens * 384);

    let normalized = model.embed("meow", false, true).unwrap();
    let norm = normalized
      .iter()
      .map(|&v| f64::from(v) * f64::from(v))
      .sum::<f64>()
      .sqrt();
    assert!((norm - 1.0).abs() < 1e-5, "norm {norm}");
  }
}
