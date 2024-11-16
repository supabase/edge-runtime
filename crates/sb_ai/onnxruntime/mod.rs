mod model_session;
pub(crate) mod onnx;
pub(crate) mod session;
mod tensor;

use core::str;
use std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use anyhow::{anyhow, Result};
use deno_core::{op2, JsBuffer, OpState};

use model_session::{ModelInfo, ModelSession};
use ort::Session;
use tensor::{JsTensor, ToJsTensor};

#[op2(async)]
#[to_v8]
pub async fn op_sb_ai_ort_init_session(
    state: Rc<RefCell<OpState>>,
    #[buffer] model_bytes: JsBuffer,
) -> Result<ModelInfo> {
    let model_bytes = model_bytes.into_parts().to_boxed_slice();

    let model_info = match str::from_utf8(&model_bytes) {
        Ok(model_url) => ModelSession::from_url(model_url).await?,
        Err(_) => ModelSession::from_bytes(&model_bytes)?,
    };

    let mut state = state.borrow_mut();

    let mut sessions = { state.try_take::<Vec<Arc<Session>>>().unwrap_or_default() };

    sessions.push(model_info.inner());
    state.put(sessions);

    Ok(model_info.info())
}

#[op2]
#[serde]
pub fn op_sb_ai_ort_run_session(
    #[string] model_id: String,
    #[serde] input_values: HashMap<String, JsTensor>,
) -> Result<HashMap<String, ToJsTensor>> {
    let model = ModelSession::from_id(model_id.to_owned())
        .ok_or(anyhow!("could not found session for id={model_id:?}"))?;

    let model_session = model.inner();

    let input_values = input_values
        .into_iter()
        .map(|(key, value)| {
            value
                .extract_ort_input()
                .map(|value| (Cow::from(key), value))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut outputs = model_session.run(input_values)?;
    let mut output_values = HashMap::new();

    // We need to `pop` over outputs to get 'value' ownership, since keys are attached to 'model_session' lifetime
    // it can't be iterated with `into_iter()`
    for _ in 0..outputs.len() {
        let (key, value) = outputs.pop_first().ok_or(anyhow!(
            "could not retrieve output value from model session"
        ))?;

        let value = ToJsTensor::from_ort_tensor(value)?;

        output_values.insert(key.to_owned(), value);
    }

    Ok(output_values)
}

#[op2(fast)]
pub fn op_sb_ai_ort_fetch(#[string] model_url: String) {
    println!("Hello from fetch: {model_url:?}");
}
