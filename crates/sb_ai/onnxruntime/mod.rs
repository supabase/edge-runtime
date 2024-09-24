mod model_session;
pub(crate) mod onnx;
pub(crate) mod session;
mod tensor;

use anyhow::Result;
use deno_core::op2;

use model_session::{ModelInfo, ModelSession};
use tensor::{JsDynTensorValue, JsSessionInputs, JsSessionOutputs};

#[op2]
#[to_v8]
pub fn op_sb_ai_ort_init_session(#[buffer] model_bytes: &[u8]) -> Result<ModelInfo> {
    let model_info = ModelSession::from_bytes(model_bytes)?;

    Ok(model_info.info())
}

#[op2]
#[to_v8]
pub fn op_sb_ai_ort_run_session(
    #[string] model_id: String,
    #[from_v8] input_values: JsSessionInputs,
) -> Result<JsSessionOutputs> {
    let model = ModelSession::from_id(model_id).unwrap();
    let model_session = model.inner();

    let input_values = input_values.to_ort_session_inputs(model.info().input_names.into_iter())?;

    let mut outputs = model_session.run(input_values)?;
    let mut output_values = vec![];

    // We need to `pop` over outputs to get it ownership, since keys are attached to 'model_session' lifetime
    // and since we are sending tuples to JS, we don't need the model's output keys.
    for _ in 0..outputs.len() {
        let (_, ort_value) = outputs.pop_first().unwrap();

        let js_value = JsDynTensorValue(ort_value);

        output_values.push(js_value);
    }

    let outputs = JsSessionOutputs(output_values);

    Ok(outputs)
}
