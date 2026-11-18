mod download;
mod gte;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use base_rt::BlockingScopeCPUUsageMetricExt;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use tracing::error;

deno_core::extension!(
  ai,
  ops = [
    op_ai_init_model,
    op_ai_run_model,
    op_ai_try_cleanup_unused_session,
  ],
  esm_entry_point = "ext:ai/ai.js",
  esm = [
    dir "js",
    "ai.js",
    "util/event_stream_parser.mjs",
    "util/event_source_stream.mjs",
  ]
);

const GTE_SMALL: &str = "gte-small";

#[op2(async)]
pub async fn op_ai_init_model(
  state: Rc<RefCell<OpState>>,
  #[string] name: String,
) -> Result<(), AnyError> {
  if name != GTE_SMALL {
    bail!("model not supported");
  }

  if state.borrow().has::<Arc<gte::GteSmall>>() {
    return Ok(());
  }

  let model = gte::load().await.inspect_err(|err| {
    error!(reason = ?err, "failed to initialize gte-small");
  })?;

  state.borrow_mut().put(model);
  Ok(())
}

#[op2(async)]
#[serde]
pub async fn op_ai_run_model(
  state: Rc<RefCell<OpState>>,
  #[string] name: String,
  #[string] prompt: String,
  mean_pool: bool,
  normalize: bool,
) -> Result<Vec<f32>, AnyError> {
  if name != GTE_SMALL {
    bail!("model not supported");
  }

  // Inference runs on a blocking thread, with its CPU time charged to this
  // worker.
  let handle = {
    let mut state = state.borrow_mut();
    let Some(model) = state.try_borrow::<Arc<gte::GteSmall>>().cloned() else {
      bail!("run init model first");
    };
    state.spawn_cpu_accumul_blocking_scope(move || {
      model.embed(&prompt, mean_pool, normalize)
    })
  };

  handle.await.context("failed to get inference results")?
}

#[op2(async)]
#[bigint]
pub async fn op_ai_try_cleanup_unused_session() -> Result<usize, AnyError> {
  Ok(gte::cleanup().await)
}
