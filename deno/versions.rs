use std::borrow::Cow;

use deno_telemetry::OtelRuntimeConfig;

use crate::version;

pub fn deno() -> &'static str {
  concat!("Supa:", "0")
}

pub fn get_user_agent() -> &'static str {
  concat!("Supa:", "0")
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
