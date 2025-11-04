use std::borrow::Cow;

use deno_telemetry::OtelRuntimeConfig;
use once_cell::sync::Lazy;

use crate::version;

pub fn edge_runtime_version() -> &'static str {
  option_env!("GIT_V_TAG").unwrap_or("0.1.0")
}

pub fn user_agent() -> &'static str {
  static VALUE: Lazy<String> = Lazy::new(|| {
    let deno_version = version();
    let edge_runtime_version = edge_runtime_version();
    format!(
      // TODO: It should be changed to a well-known name for the ecosystem.
      "Deno/{} (variant; SupabaseEdgeRuntime/{})",
      deno_version, edge_runtime_version
    )
  });

  VALUE.as_str()
}

pub fn is_canary() -> bool {
  false
}

pub fn otel_runtime_config() -> OtelRuntimeConfig {
  OtelRuntimeConfig {
    runtime_name: Cow::Borrowed("deno"),
    runtime_version: Cow::Borrowed(version()),
  }
}
