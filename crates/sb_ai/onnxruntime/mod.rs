mod model_session;
pub(crate) mod onnx;
pub(crate) mod session;
mod tensor;

use std::{borrow::Cow, collections::HashMap};

use anyhow::Result;
use deno_core::op2;

use model_session::{ModelInfo, ModelSession};
use tensor::{JsTensor, ToJsTensor};

#[op2]
#[to_v8]
pub fn op_sb_ai_ort_init_session(#[buffer] model_bytes: &[u8]) -> Result<ModelInfo> {
    let model_info = ModelSession::from_bytes(model_bytes)?;

    Ok(model_info.info())
}

#[op2]
#[serde]
pub fn op_sb_ai_ort_run_session<'a>(
    #[string] model_id: String,
    #[serde] input_values: HashMap<String, JsTensor>,
) -> Result<HashMap<String, ToJsTensor>> {
    let model = ModelSession::from_id(model_id).unwrap();
    let model_session = model.inner();

    let input_values = input_values
        .into_iter()
        .map(|(key, value)| value.as_ort_input().map(|value| (Cow::from(key), value)))
        .collect::<Result<Vec<_>>>()?;

    let mut outputs = model_session.run(input_values)?;
    let mut output_values = HashMap::new();

    // We need to `pop` over outputs to get 'value' ownership, since keys are attached to 'model_session' lifetime
    // it can't be iterated with `into_iter()`
    for _ in 0..outputs.len() {
        let (key, value) = outputs.pop_first().unwrap();

        let value = ToJsTensor::from_ort_tensor(value)?;

        output_values.insert(key.to_owned(), value);
    }

    Ok(output_values)
}
