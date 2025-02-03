use std::{ffi::c_void, fmt::Debug, mem::size_of, rc::Rc};

use anyhow::anyhow;
use deno_core::{error::AnyError, v8, JsBuffer, ToJsBuffer};
use ort::{
    AllocationDevice, AllocatorType, DynValue, DynValueTypeMarker, IntoTensorElementType,
    MemoryInfo, MemoryType, SessionInputValue, TensorElementType, TensorRefMut, ValueRefMut,
    ValueType,
};
use serde::{Deserialize, Serialize};

// We zero-copy an ORT Tensor to a JS ArrayBuffer like:
// https://github.com/denoland/deno_core/blob/7258aa325368a8e2c1271a25c1b4d537ed41e9c5/core/runtime/ops_rust_to_v8.rs#L370
// We could try `Tensor::try_extract_raw_tensor_mut<T>` with `v8::ArrayBuffer::new_backing_store_from_bytes`
// but it only allows [u8] instead of [T], so we need to get into `unsafe` path.
macro_rules! v8_slice_from {
    (tensor::<$type:ident>($tensor:expr)) => {{
        // We must ensure there's some detection to avoid `null pointer` errors
        // https://github.com/pykeio/ort/issues/185
        let n_detections = $tensor.shape()?[0];
        if n_detections == 0 {
            let buf_store = v8::ArrayBuffer::new_backing_store_from_vec(vec![]).make_shared();
            let buffer_slice =
                unsafe { deno_core::serde_v8::V8Slice::<u8>::from_parts(buf_store, 0..0) };

            buffer_slice
        } else {
            let (_, raw_tensor) = $tensor
                .try_extract_raw_tensor_mut::<$type>()
                .map_err(AnyError::from)?;

            let tensor_ptr = raw_tensor.as_ptr();
            let tensor_len = raw_tensor.len();
            let tensor_rc = Rc::into_raw(Rc::new(raw_tensor)) as *const c_void;

            let buffer_len = tensor_len * size_of::<$type>();

            extern "C" fn drop_tensor(_ptr: *mut c_void, _len: usize, data: *mut c_void) {
                // SAFETY: We know that data is a raw Rc from above
                unsafe { drop(Rc::from_raw(data.cast::<$type>())) }
            }

            let buf_store = unsafe {
                v8::ArrayBuffer::new_backing_store_from_ptr(
                    tensor_ptr as _,
                    buffer_len,
                    drop_tensor,
                    tensor_rc as _,
                )
            }
            .make_shared();

            let buffer_slice =
                unsafe { deno_core::serde_v8::V8Slice::<u8>::from_parts(buf_store, 0..buffer_len) };

            buffer_slice
        }
    }};
}

// Type alias for TensorElementType
// https://serde.rs/remote-derive.html
#[derive(Debug, Serialize, Deserialize)]
#[serde(remote = "TensorElementType", rename_all = "lowercase")]
pub enum JsTensorType {
    /// 32-bit floating point number, equivalent to Rust's `f32`.
    Float32,
    /// Unsigned 8-bit integer, equivalent to Rust's `u8`.
    Uint8,
    /// Signed 8-bit integer, equivalent to Rust's `i8`.
    Int8,
    /// Unsigned 16-bit integer, equivalent to Rust's `u16`.
    Uint16,
    /// Signed 16-bit integer, equivalent to Rust's `i16`.
    Int16,
    /// Signed 32-bit integer, equivalent to Rust's `i32`.
    Int32,
    /// Signed 64-bit integer, equivalent to Rust's `i64`.
    Int64,
    /// String, equivalent to Rust's `String`.
    String,
    /// Boolean, equivalent to Rust's `bool`.
    Bool,
    /// 16-bit floating point number, equivalent to [`half::f16`] (requires the `half` feature).
    Float16,
    /// 64-bit floating point number, equivalent to Rust's `f64`. Also known as `double`.
    Float64,
    /// Unsigned 32-bit integer, equivalent to Rust's `u32`.
    Uint32,
    /// Unsigned 64-bit integer, equivalent to Rust's `u64`.
    Uint64,
    /// Brain 16-bit floating point number, equivalent to [`half::bf16`] (requires the `half` feature).
    Bfloat16,
}

#[derive(Serialize, Deserialize)]
struct JsTensorTypeSerdeHelper(#[serde(with = "JsTensorType")] TensorElementType);

#[derive(Debug, Serialize, Deserialize)]
pub struct JsTensor {
    #[serde(rename = "type", with = "JsTensorType")]
    data_type: TensorElementType,
    data: JsBuffer,
    dims: Vec<i64>,
}

impl JsTensor {
    pub fn extract_ort_tensor_ref<'a, T: IntoTensorElementType + Debug>(
        mut self,
    ) -> anyhow::Result<ValueRefMut<'a, DynValueTypeMarker>> {
        let expected_length = self.dims.iter().product::<i64>() as usize;
        let current_length = self.data.len() / size_of::<T>();

        if current_length != expected_length {
            return Err(anyhow!(
                "invalid tensor length! got '{current_length}' expect '{expected_length}'"
            ));
        };

        // Same impl. as the Tensor::from_array()
        // https://github.com/pykeio/ort/blob/abd527b6a1df8f566c729a9c4398bdfd185d652f/src/value/impl_tensor/create.rs#L170
        let memory_info = MemoryInfo::new(
            AllocationDevice::CPU,
            0,
            AllocatorType::Arena,
            MemoryType::CPUInput,
        )?;

        // Zero-Copying Data to an ORT Tensor based on JS type
        // SAFETY: we did check tensor size above
        let tensor = unsafe {
            TensorRefMut::<T>::from_raw(
                memory_info,
                self.data.as_mut_ptr() as *mut c_void,
                self.dims,
            )
        }?;

        Ok(tensor.into_dyn())
    }

    pub fn extract_ort_input<'a>(self) -> anyhow::Result<SessionInputValue<'a>> {
        let input_value = match self.data_type {
            TensorElementType::Float32 => self.extract_ort_tensor_ref::<f32>()?.into(),
            TensorElementType::Float64 => self.extract_ort_tensor_ref::<f64>()?.into(),
            TensorElementType::String => {
                // TODO: Handle string[] tensors from 'v8::Array'
                return Err(anyhow!("Can't extract tensor from it: 'String' does not implement the 'IntoTensorElementType' trait."));
            }
            TensorElementType::Int8 => self.extract_ort_tensor_ref::<i8>()?.into(),
            TensorElementType::Uint8 => self.extract_ort_tensor_ref::<u8>()?.into(),
            TensorElementType::Int16 => self.extract_ort_tensor_ref::<i16>()?.into(),
            TensorElementType::Uint16 => self.extract_ort_tensor_ref::<u16>()?.into(),
            TensorElementType::Int32 => self.extract_ort_tensor_ref::<i32>()?.into(),
            TensorElementType::Uint32 => self.extract_ort_tensor_ref::<u32>()?.into(),
            TensorElementType::Int64 => self.extract_ort_tensor_ref::<i64>()?.into(),
            TensorElementType::Uint64 => self.extract_ort_tensor_ref::<u64>()?.into(),
            TensorElementType::Bool => self.extract_ort_tensor_ref::<bool>()?.into(),
            TensorElementType::Float16 => {
                return Err(anyhow!("'half::f16' is not supported by JS tensor."))
            }
            TensorElementType::Bfloat16 => {
                return Err(anyhow!("'half::bf16' is not supported by JS tensor."))
            }
        };

        Ok(input_value)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToJsTensor {
    #[serde(rename = "type", with = "JsTensorType")]
    data_type: TensorElementType,
    data: ToJsBuffer,
    pub dims: Vec<i64>,
}

impl ToJsTensor {
    pub fn from_ort_tensor(mut value: DynValue) -> anyhow::Result<Self> {
        let ort_type = value.dtype().map_err(AnyError::from)?;

        let ValueType::Tensor { ty, dimensions } = ort_type else {
            return Err(anyhow!(
                "JS only support 'ort::Value' of 'Tensor' type, got '{ort_type:?}'."
            ));
        };

        let buffer_slice = match ty {
            TensorElementType::Float32 => v8_slice_from!(tensor::<f32>(value)),
            TensorElementType::Float64 => v8_slice_from!(tensor::<f64>(value)),
            TensorElementType::Int8 => v8_slice_from!(tensor::<u8>(value)),
            TensorElementType::Uint8 => v8_slice_from!(tensor::<u8>(value)),
            TensorElementType::Int16 => v8_slice_from!(tensor::<i16>(value)),
            TensorElementType::Uint16 => v8_slice_from!(tensor::<u16>(value)),
            TensorElementType::Int32 => v8_slice_from!(tensor::<i32>(value)),
            TensorElementType::Uint32 => v8_slice_from!(tensor::<u32>(value)),
            TensorElementType::Int64 => v8_slice_from!(tensor::<i64>(value)),
            TensorElementType::Uint64 => v8_slice_from!(tensor::<u64>(value)),
            TensorElementType::Bool => v8_slice_from!(tensor::<bool>(value)),
            TensorElementType::String => todo!(),
            TensorElementType::Float16 => todo!(),
            TensorElementType::Bfloat16 => todo!(),
        };

        Ok(Self {
            data_type: ty,
            data: ToJsBuffer::from(buffer_slice.to_boxed_slice()),
            dims: dimensions,
        })
    }
}

#[cfg(test)]
mod tests {
    use sb_ai_v8_utilities::v8_do;

    use super::*;

    #[test]
    fn test_ort_tensor_extract_ref() {
        v8_do(|| {
            // region: v8-init
            // ref: https://github.com/denoland/deno_core/blob/490079f6b5c9233f476b0a529eace1f5b2c4ed07/serde_v8/tests/magic.rs#L23
            let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
            let handle_scope = &mut v8::HandleScope::new(isolate);
            let context = v8::Context::new(handle_scope);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            // endregion: v8-init

            // Bad Tensor Scenario:
            let tensor_script = r#"({
                type: 'float32',
                data: new Float32Array([]),
                dims: [1, 1],
                size: 300
            })"#;

            let js_tensor = {
                let code = v8::String::new(scope, tensor_script).unwrap();
                let script = v8::Script::compile(scope, code, None).unwrap();
                script.run(scope).unwrap()
            };

            let tensor: JsTensor = deno_core::serde_v8::from_v8(scope, js_tensor).unwrap();

            let tensor_ref_result = tensor.extract_ort_tensor_ref::<f32>();
            assert!(
                tensor_ref_result.is_err(),
                "Since `data.len()` doesn't reflect `dims` it must return Error"
            );

            // Good Tensor Scenario:
            let tensor_script = r#"({
                type: 'float32',
                data: new Float32Array([0.1, 0.2]),
                dims: [1, 2],
                size: 2
            })"#;

            let js_tensor = {
                let code = v8::String::new(scope, tensor_script).unwrap();
                let script = v8::Script::compile(scope, code, None).unwrap();
                script.run(scope).unwrap()
            };

            let tensor: JsTensor = deno_core::serde_v8::from_v8(scope, js_tensor).unwrap();

            let tensor_ref_result = tensor.extract_ort_tensor_ref::<f32>();
            assert!(tensor_ref_result.is_ok());
        });
    }
}
