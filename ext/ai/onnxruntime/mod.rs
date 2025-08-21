mod model;
mod tensor;

pub(crate) mod onnx;
pub(crate) mod session;

use core::str;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use base_rt::BlockingScopeCPUUsageMetricExt;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::JsBuffer;
use deno_core::JsRuntime;
use deno_core::OpState;
use deno_core::ToJsBuffer;
use deno_core::V8CrossThreadTaskSpawner;

use model::Model;
use model::ModelInfo;
use ort::session::Session;
use reqwest::Url;
use tensor::JsTensor;
use tensor::ToJsTensor;
use tokio::sync::oneshot;
use tokio_util::bytes::BufMut;
use tracing::debug;
use tracing::trace;

#[op2(async)]
#[to_v8]
pub async fn op_ai_ort_init_session(
  state: Rc<RefCell<OpState>>,
  #[buffer] model_bytes: JsBuffer,
  // Maybe improve the code style to enum payload or something else
  #[string] req_authorization: Option<String>,
) -> Result<ModelInfo> {
  let model_bytes = model_bytes.into_parts().to_boxed_slice();
  let model_bytes_or_url = str::from_utf8(&model_bytes)
    .map_err(AnyError::from)
    .and_then(|utf8_str| Url::parse(utf8_str).map_err(AnyError::from));

  let model = match model_bytes_or_url {
    Ok(model_url) => {
      trace!(kind = "url", url = %model_url);
      Model::from_url(model_url, req_authorization).await?
    }
    Err(_) => {
      trace!(kind = "bytes", len = model_bytes.len());
      Model::from_bytes(&model_bytes).await?
    }
  };

  let mut state = state.borrow_mut();
  let mut sessions =
    { state.try_take::<Vec<Arc<Session>>>().unwrap_or_default() };

  sessions.push(model.get_session());
  state.put(sessions);

  Ok(model.get_info()).inspect(|it| {
    debug!(model_info = %it);
  })
}

#[op2(async)]
#[serde]
pub async fn op_ai_ort_run_session(
  state: Rc<RefCell<OpState>>,
  #[string] model_id: String,
  #[serde] input_values: HashMap<String, JsTensor>,
) -> Result<HashMap<String, ToJsTensor>> {
  let model = Model::from_id(&model_id)
    .await
    .ok_or(anyhow!("could not found session for id: {model_id:?}"))?;

  let model_session = model.get_session();
  let cross_thread_spawner =
    state.borrow().borrow::<V8CrossThreadTaskSpawner>().clone();
  let (tx, rx) = oneshot::channel();

  cross_thread_spawner.spawn(move |state| {
    let input_values = input_values
      .into_iter()
      .map(|(key, value)| {
        value
          .extract_ort_input()
          .map(|value| (Cow::from(key), value))
      })
      .collect::<Result<Vec<_>>>();

    let input_values = match input_values {
      Ok(v) => v,
      Err(err) => {
        let _ = tx.send(Err(err));
        return;
      }
    };

    JsRuntime::op_state_from(state)
      .borrow_mut()
      .spawn_cpu_accumul_blocking_scope(move || {
        let outputs = match model_session.run(input_values) {
          Ok(v) => v,
          Err(err) => {
            let _ = tx.send(Err(anyhow::Error::from(err)));
            return;
          }
        };

        let outputs = outputs
          .into_iter()
          .map(|(key, value)| {
            ToJsTensor::from_ort_tensor(value)
              .map(|value| (key.to_string(), value))
          })
          .collect::<Result<HashMap<_, _>>>();

        let outputs = match outputs {
          Ok(v) => v,
          Err(err) => {
            let _ = tx.send(Err(err));
            return;
          }
        };

        let _ = tx.send(Ok(outputs));
      });
  });

  rx.await.context("failed to get inference result")?
}

// REF: https://youtu.be/qqjvB_VxMRM?si=7lnYdgbhOC_K7P6S
// http://soundfile.sapp.org/doc/WaveFormat/
#[op2]
#[serde]
pub fn op_ai_ort_encode_tensor_audio(
  #[serde] tensor: JsBuffer,
  sample_rate: u32,
) -> Result<ToJsBuffer> {
  // let copy for now
  let data_buffer = tensor.iter().as_slice();

  let sample_size = 4; // f32 tensor
  let data_chunk_size = data_buffer.len() as u32 * sample_size;
  let total_riff_size = 36 + data_chunk_size; // 36 is the total of bytes until write data

  let mut audio_wav = Vec::new();

  // RIFF HEADER
  audio_wav.extend_from_slice(b"RIFF");
  audio_wav.put_u32_le(total_riff_size);
  audio_wav.extend_from_slice(b"WAVE");

  // FORMAT CHUNK
  audio_wav.extend_from_slice(b"fmt "); // whitespace needed "fmt" + " "
  audio_wav.put_u32_le(16); // PCM chunk size
  audio_wav.put_u16_le(3); // RAW audio format
  audio_wav.put_u16_le(1); // Number of channels
  audio_wav.put_u32_le(sample_rate);
  audio_wav.put_u32_le(sample_rate * sample_size); // Byte rate
  audio_wav.put_u16_le(sample_size as u16); // Block align
  audio_wav.put_u16_le(32); // f32 Bits per sample

  // DATA Chunk
  audio_wav.extend_from_slice(b"data"); // chunk ID
  audio_wav.put_u32_le(data_chunk_size);

  audio_wav.extend_from_slice(data_buffer);

  Ok(ToJsBuffer::from(audio_wav))
}
