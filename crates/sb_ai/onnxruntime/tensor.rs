use core::slice;
use std::{
    borrow::Cow,
    ffi::c_void,
    fmt::Debug,
    marker::PhantomData,
    mem::{size_of, MaybeUninit},
    ops::{Deref, DerefMut},
    rc::Rc,
    str::Bytes,
    sync::Arc,
};

use anyhow::anyhow;
use deno_core::{
    error::{AnyError, StdAnyError},
    parking_lot::Mutex,
    serde_v8::{from_v8, to_v8},
    v8::{self},
    FromV8, ToV8,
};
use ort::{
    AllocationDevice, AllocatorType, DynTensor, DynTensorRefMut, DynValue, DynValueTypeMarker,
    IntoTensorElementType, MemoryInfo, MemoryType, SessionInputValue, SessionInputs,
    SessionOutputs, Tensor as OrtTensor, TensorElementType, TensorRefMut, ValueRefMut, ValueType,
};
use tracing_subscriber::filter::targets;

// We expect that a given JS tensor type string be less than 1 byte
// since the largest string is 'floatNN': 7
const EXPECTED_TENSOR_TYPE_STR_LENGHT: usize = 8;

// We zero-copy an ORT Tensor to a JS ArrayBuffer like:
// https://github.com/denoland/deno_core/blob/7258aa325368a8e2c1271a25c1b4d537ed41e9c5/core/runtime/ops_rust_to_v8.rs#L370
// We could try `Tensor::try_extract_raw_tensor_mut<T>` with `v8::ArrayBuffer::new_backing_store_from_bytes`
// but it only allows [u8] instead of [T], so we need to get into `unsafe` path.
macro_rules! v8_typed_array_from {
    ($array_type:ident; tensor::<$type:ident>($tensor:expr), $scope:expr) => {{
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

        // Zero-Copying using ptr
        let buf_store = unsafe {
            v8::ArrayBuffer::new_backing_store_from_ptr(
                tensor_ptr as _,
                buffer_len,
                drop_tensor,
                tensor_rc as _,
            )
        }
        .make_shared();

        let buffer = v8::ArrayBuffer::with_backing_store($scope, &buf_store);

        v8::$array_type::new($scope, buffer, 0, tensor_len)
            .ok_or(anyhow!("Could not create '$array_type' from tensor."))?
    }};
}

#[derive(Debug)]
pub struct JsTensorElementType(TensorElementType);

impl Deref for JsTensorElementType {
    type Target = TensorElementType;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&str> for JsTensorElementType {
    type Error = AnyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let tensor_type = match value {
            "float32" => TensorElementType::Float32,
            "float64" => TensorElementType::Float64,
            "string" => TensorElementType::String,
            "int8" => TensorElementType::Int8,
            "uint8" => TensorElementType::Uint8,
            "int16" => TensorElementType::Int16,
            "uint16" => TensorElementType::Uint16,
            "int32" => TensorElementType::Int32,
            "uint32" => TensorElementType::Uint32,
            "int64" => TensorElementType::Int64,
            "uint64" => TensorElementType::Uint64,
            "bool" => TensorElementType::Bool,
            _ => return Err(anyhow!("value '{value}' is not a valid tensor type.")),
        };

        Ok(Self(tensor_type))
    }
}

impl TryFrom<JsTensorElementType> for &str {
    type Error = AnyError;

    fn try_from(value: JsTensorElementType) -> Result<Self, Self::Error> {
        let js_tensor_type = match value.0 {
            TensorElementType::Float32 => "float32",
            TensorElementType::Float64 => "float64",
            TensorElementType::String => "string",
            TensorElementType::Int8 => "int8",
            TensorElementType::Uint8 => "uint8",
            TensorElementType::Int16 => "int16",
            TensorElementType::Uint16 => "uint16",
            TensorElementType::Int32 => "int32",
            TensorElementType::Uint32 => "uint32",
            TensorElementType::Int64 => "int64",
            TensorElementType::Uint64 => "uint64",
            TensorElementType::Bool => "bool",
            _ => return Err(anyhow!("value '{value:?}' is not a valid tensor type.")),
        };

        Ok(js_tensor_type)
    }
}

impl<'a> FromV8<'a> for JsTensorElementType {
    type Error = StdAnyError;

    fn from_v8(
        scope: &mut v8::HandleScope<'a>,
        value: v8::Local<'a, v8::Value>,
    ) -> Result<Self, Self::Error> {
        let value = v8::Local::<v8::String>::try_from(value).map_err(AnyError::from)?;

        // Here we try to zero-copy the given Js string.
        let mut buffer = [MaybeUninit::<u8>::uninit(); EXPECTED_TENSOR_TYPE_STR_LENGHT];
        let value_str = value.to_rust_cow_lossy(scope, &mut buffer);

        let tensor_type = match value_str {
            std::borrow::Cow::Borrowed(value_str) => JsTensorElementType::try_from(value_str)?,
            std::borrow::Cow::Owned(value_str) => {
                JsTensorElementType::try_from(value_str.as_str())?
            }
        };

        Ok(tensor_type)
    }
}

impl<'a> ToV8<'a> for JsTensorElementType {
    type Error = StdAnyError;

    fn to_v8(
        self,
        scope: &mut v8::HandleScope<'a>,
    ) -> Result<v8::Local<'a, v8::Value>, Self::Error> {
        let tensor_type_str = self.try_into()?;
        let js_tensor_type = v8::String::new(scope, tensor_type_str)
            .ok_or(anyhow!("could not parse '{tensor_type_str}' to V8 String",))?;

        Ok(js_tensor_type.into())
    }
}

#[derive(Debug)]
pub struct JsDynTensorValue(pub DynValue);

impl Deref for JsDynTensorValue {
    type Target = DynValue;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for JsDynTensorValue {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> ToV8<'a> for JsDynTensorValue {
    type Error = StdAnyError;

    fn to_v8(
        mut self,
        scope: &mut v8::HandleScope<'a>,
    ) -> Result<v8::Local<'a, v8::Value>, Self::Error> {
        let ort_type = self.dtype().map_err(AnyError::from)?;

        let ValueType::Tensor { ty, dimensions } = ort_type else {
            return Err(anyhow!(
                "JS only support 'ort::Value' of 'Tensor' type, got '{ort_type:?}'."
            )
            .into());
        };

        let tensor_type = JsTensorElementType(ty);

        // Zero-Copying Data from ORT to a JS ArrayBuffer based on type
        let tensor_buffer = match *tensor_type {
            TensorElementType::Float32 => {
                v8_typed_array_from!(Float32Array; tensor::<f32>(self), scope).into()
            }
            TensorElementType::Float64 => {
                v8_typed_array_from!(Float64Array; tensor::<f64>(self), scope).into()
            }
            TensorElementType::String => {
                // TODO: Handle string[] tensors from 'v8::Array'
                return Err(anyhow!("Can't zero-copy tensor from it: 'String' does not implement the 'IntoTensorElementType' trait.").into());
            }
            TensorElementType::Int8 => {
                v8_typed_array_from!(Int8Array; tensor::<i8>(self), scope).into()
            }
            TensorElementType::Uint8 => {
                v8_typed_array_from!(Uint8Array; tensor::<i8>(self), scope).into()
            }
            TensorElementType::Int16 => {
                v8_typed_array_from!(Int16Array; tensor::<i16>(self), scope).into()
            }
            TensorElementType::Uint16 => {
                v8_typed_array_from!(Uint16Array; tensor::<u16>(self), scope).into()
            }
            TensorElementType::Int32 => {
                v8_typed_array_from!(Int32Array; tensor::<i32>(self), scope).into()
            }
            TensorElementType::Uint32 => {
                v8_typed_array_from!(Uint32Array; tensor::<u32>(self), scope).into()
            }
            TensorElementType::Int64 => {
                v8_typed_array_from!(BigInt64Array; tensor::<i64>(self), scope).into()
            }
            TensorElementType::Uint64 => {
                v8_typed_array_from!(BigUint64Array; tensor::<u64>(self), scope).into()
            }
            TensorElementType::Bool => {
                v8_typed_array_from!(Uint8Array; tensor::<bool>(self), scope).into()
            }
            TensorElementType::Float16 => {
                return Err(anyhow!("'half::f16' is not supported by JS tensor.").into())
            }
            TensorElementType::Bfloat16 => {
                return Err(anyhow!("'half::bf16' is not supported by JS tensor.").into())
            }
        };

        let tensor_type = tensor_type.to_v8(scope).map_err(AnyError::from)?;
        let tensor_dims = to_v8(scope, dimensions).map_err(AnyError::from)?;

        // We pass tensor as tuples to avoid string allocation
        // https://docs.rs/deno_core/latest/deno_core/convert/trait.ToV8.html#structs
        let tensor =
            v8::Array::new_with_elements(scope, &[tensor_type, tensor_buffer, tensor_dims]);

        Ok(tensor.into())
    }
}

#[derive(Debug)]
pub struct JsTensorBufferView<'v> {
    data_type: JsTensorElementType,
    data_ptr: *mut c_void,
    shape: Vec<i64>,
    lifetime: PhantomData<&'v ()>,
}

impl<'v> JsTensorBufferView<'v> {
    pub fn to_ort_tensor_ref(&self) -> anyhow::Result<ValueRefMut<'v, DynValueTypeMarker>> {
        // Same impl. as the Tensor::from_array()
        // https://github.com/pykeio/ort/blob/abd527b6a1df8f566c729a9c4398bdfd185d652f/src/value/impl_tensor/create.rs#L170
        let memory_info = MemoryInfo::new(
            AllocationDevice::CPU,
            0,
            AllocatorType::Arena,
            MemoryType::CPUInput,
        )?;

        // TODO: Try to zero-copy shape also
        let shape = self.shape.to_owned();

        // Zero-Copying Data to an ORT Tensor based on JS type
        let tensor = match *self.data_type {
            TensorElementType::Float32 => unsafe {
                TensorRefMut::<f32>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Float64 => unsafe {
                TensorRefMut::<f64>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::String => {
                // TODO: Handle string[] tensors from 'v8::Array'
                return Err(anyhow!("Can't zero-copy tensor from it: 'String' does not implement the 'IntoTensorElementType' trait."));
            }
            TensorElementType::Int8 => unsafe {
                TensorRefMut::<i8>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Uint8 => unsafe {
                TensorRefMut::<u8>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Int16 => unsafe {
                TensorRefMut::<i16>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Uint16 => unsafe {
                TensorRefMut::<u16>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Int32 => unsafe {
                TensorRefMut::<i32>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Uint32 => unsafe {
                TensorRefMut::<u32>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Int64 => unsafe {
                TensorRefMut::<i64>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Uint64 => unsafe {
                TensorRefMut::<u64>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Bool => unsafe {
                TensorRefMut::<bool>::from_raw(memory_info, self.data_ptr, shape)?.into_dyn()
            },
            TensorElementType::Float16 => {
                return Err(anyhow!("'half::f16' is not supported by JS tensor."))
            }
            TensorElementType::Bfloat16 => {
                return Err(anyhow!("'half::bf16' is not supported by JS tensor."))
            }
        };

        Ok(tensor.into_dyn())
    }
}

impl<'a> FromV8<'a> for JsTensorBufferView<'a> {
    type Error = StdAnyError;

    fn from_v8(
        scope: &mut deno_core::v8::HandleScope<'a>,
        value: deno_core::v8::Local<'a, deno_core::v8::Value>,
    ) -> Result<Self, Self::Error> {
        let value = v8::Local::<v8::Array>::try_from(value).map_err(AnyError::from)?;

        if value.length() != 3 {
            return Err(anyhow!(
                "expected a JS tuple of 3 elements, found {}",
                value.length()
            )
            .into());
        }
        let data_type = v8::Local::<v8::String>::try_from(
            value
                .get_index(scope, 0)
                .ok_or(anyhow!("tensor type was expected at tuple.0"))?,
        )
        .map_err(AnyError::from)?;

        let data_type = JsTensorElementType::from_v8(scope, data_type.into())?;

        // TODO: Handle 'v8::Array' for string[] tensors,
        // since string[] tensors are not passed as 'v8::TypedArray'
        let data_values = v8::Local::<v8::TypedArray>::try_from(
            value
                .get_index(scope, 1)
                .ok_or(anyhow!("tensor data was expected at tuple.1"))?,
        )
        .map_err(AnyError::from)?;

        let shape_dims = v8::Local::<v8::Array>::try_from(
            value
                .get_index(scope, 2)
                .ok_or(anyhow!("tensor dims was expected at tuple.2"))?,
        )
        .map_err(AnyError::from)?;

        // TODO: Pass dims as 'TypedArray' from Js
        // Since dims are 'v8::Array' and not 'v8::TypedArray' we can't get the inner ptr to zero copy.
        let shape_dims = from_v8::<Vec<_>>(scope, shape_dims.into()).map_err(AnyError::from)?;

        Ok(JsTensorBufferView {
            data_type,
            data_ptr: data_values.data(),
            shape: shape_dims,
            lifetime: PhantomData::default(),
        })
    }
}

// Useful to receive the model inputs as JS sequence
#[derive(Debug)]
pub struct JsSessionInputs<'a>(Arc<Vec<JsTensorBufferView<'a>>>);

impl<'a> JsSessionInputs<'a> {
    pub fn to_ort_session_inputs(
        self,
        keys: impl Iterator<Item = String>,
    ) -> Result<SessionInputs<'a, 'a>, AnyError> {
        let mut session_inputs = vec![];

        for (key, tensor_view) in keys.zip(self.iter()) {
            let value = SessionInputValue::from(tensor_view.to_ort_tensor_ref()?);

            session_inputs.push((Cow::from(key), value));
        }

        Ok(SessionInputs::ValueMap(session_inputs))
    }
}

impl<'a> Deref for JsSessionInputs<'a> {
    type Target = Vec<JsTensorBufferView<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> FromV8<'a> for JsSessionInputs<'a> {
    type Error = StdAnyError;

    fn from_v8(
        scope: &mut deno_core::v8::HandleScope<'a>,
        value: deno_core::v8::Local<'a, deno_core::v8::Value>,
    ) -> Result<Self, Self::Error> {
        let mut sequence = vec![];
        let value = v8::Local::<v8::Array>::try_from(value).map_err(AnyError::from)?;

        for idx in 0..value.length() {
            let value = value
                .get_index(scope, idx)
                .ok_or(anyhow!("could no get value for index {idx}"))?;

            let tensor = JsTensorBufferView::from_v8(scope, value)?;

            sequence.push(tensor);
        }

        Ok(Self(Arc::new(sequence)))
    }
}

#[derive(Debug)]
pub struct JsSessionOutputs(pub Vec<JsDynTensorValue>);

impl Deref for JsSessionOutputs {
    type Target = Vec<JsDynTensorValue>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for JsSessionOutputs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Useful to receive the model inputs as JS sequence
impl<'a> ToV8<'a> for JsSessionOutputs {
    type Error = StdAnyError;

    fn to_v8(
        mut self,
        scope: &mut v8::HandleScope<'a>,
    ) -> Result<v8::Local<'a, v8::Value>, Self::Error> {
        // We pass as tuples to avoid string allocation
        // https://docs.rs/deno_core/latest/deno_core/convert/trait.ToV8.html#structs

        let mut elements = vec![];

        for idx in 0..self.len() {
            let value = self.remove(idx);
            let js_value = value.to_v8(scope)?;

            elements.push(js_value);
        }

        let output_tuple = v8::Array::new_with_elements(scope, &elements);

        Ok(output_tuple.into())
    }
}
