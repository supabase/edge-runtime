use deno_config::deno_json::TsConfigForEmit;
use deno_core::serde_json;

pub fn check_warn_tsconfig(ts_config: &TsConfigForEmit) {
  if let Some(ignored_options) = &ts_config.maybe_ignored_options {
    log::warn!("{}", ignored_options);
  }
  let serde_json::Value::Object(obj) = &ts_config.ts_config.0 else {
    return;
  };
  if obj.get("experimentalDecorators") == Some(&serde_json::Value::Bool(true)) {
    log::warn!(
        "{} experimentalDecorators compiler option is deprecated and may be removed at any time",
        "Warning",
      );
  }
}
