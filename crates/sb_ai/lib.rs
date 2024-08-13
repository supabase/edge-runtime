use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::{serde_json, OpState};
use std::cell::RefCell;
use std::rc::Rc;

mod onnx;
mod pipeline;
mod tensor_ops;

pub mod defs;

deno_core::extension!(
    sb_ai,
    ops = [
        op_sb_ai_init_model,
        op_sb_ai_run_model,
        op_sb_ai_init_pipeline,
        op_sb_ai_run_pipeline,
        op_sb_ai_try_cleanup_unused_pipeline,
    ],
    esm_entry_point = "ext:sb_ai/js/ai.js",
    esm = [
        "js/ai.js",
        "js/util/event_stream_parser.js",
        "js/util/event_source_stream.js"
    ]
);

#[op2(async)]
pub async fn op_sb_ai_init_model(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
) -> Result<(), AnyError> {
    defs::init(Some(state), &name, None).await
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_run_model(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] input: serde_json::Value,
    #[serde] options: serde_json::Value,
) -> Result<serde_json::Value, AnyError> {
    pipeline::run(
        state,
        input,
        options,
        pipeline::PipelineRunArg::Simple(name.as_str()),
    )
    .await
}

#[op2(async)]
pub async fn op_sb_ai_init_pipeline(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[string] variation: Option<String>,
) -> Result<(), AnyError> {
    defs::init(Some(state), &name, variation).await
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_run_pipeline(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[string] variation: Option<String>,
    #[serde] input: serde_json::Value,
    #[serde] options: serde_json::Value,
) -> Result<serde_json::Value, AnyError> {
    pipeline::run(
        state,
        input,
        options,
        if let Some(variation) = variation.as_deref() {
            pipeline::PipelineRunArg::WithVariation {
                name: name.as_str(),
                variation,
            }
        } else {
            pipeline::PipelineRunArg::Simple(name.as_str())
        },
    )
    .await
}

#[op2(async)]
#[serde]
pub async fn op_sb_ai_try_cleanup_unused_pipeline() -> Result<serde_json::Value, anyhow::Error> {
    pipeline::cleanup().await
}
