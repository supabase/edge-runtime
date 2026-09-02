use std::borrow::Cow;

use deno_telemetry::OtelRuntimeConfig;
use once_cell::sync::Lazy;

use crate::version;

pub fn edge_runtime_version() -> &'static str {
  option_env!("GIT_V_TAG").unwrap_or("0.1.0")
}

/// Comment identifying this runtime, and the project the worker belongs to
/// when it is known.
///
/// The value is a single [RFC 9110 comment][1], so it can either close a
/// `User-Agent` built by us or be appended to one supplied by the function.
///
/// [1]: https://www.rfc-editor.org/rfc/rfc9110#name-user-agent
pub fn user_agent_comment(project_ref: Option<&str>) -> String {
  let edge_runtime_version = edge_runtime_version();

  match project_ref {
    Some(project_ref) => format!(
      "(variant; SupabaseEdgeRuntime/{}; ref={})",
      edge_runtime_version, project_ref
    ),
    None => format!("(variant; SupabaseEdgeRuntime/{})", edge_runtime_version),
  }
}

pub fn user_agent() -> &'static str {
  static VALUE: Lazy<String> = Lazy::new(|| {
    // TODO: It should be changed to a well-known name for the ecosystem.
    format!("Deno/{} {}", version(), user_agent_comment(None))
  });

  VALUE.as_str()
}

/// Same as [`user_agent`], but tagged with the project the worker belongs to.
pub fn user_agent_for_project(project_ref: Option<&str>) -> Cow<'static, str> {
  match project_ref {
    Some(project_ref) => Cow::Owned(format!(
      "Deno/{} {}",
      version(),
      user_agent_comment(Some(project_ref))
    )),
    None => Cow::Borrowed(user_agent()),
  }
}

/// Accepts a project ref only if it is safe to put in a header comment.
///
/// Refs are alphanumeric in practice; anything else is dropped rather than
/// escaped, since a ref we don't recognize is not worth attributing traffic
/// to.
pub fn sanitize_project_ref(project_ref: &str) -> Option<&str> {
  let is_valid = !project_ref.is_empty()
    && project_ref.len() <= 64
    && project_ref
      .bytes()
      .all(|it| it.is_ascii_alphanumeric() || it == b'-' || it == b'_');

  is_valid.then_some(project_ref)
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_user_agent_for_project() {
    let expected = format!(
      "Deno/{} (variant; SupabaseEdgeRuntime/{}; ref=abcdefghijklmnopqrst)",
      version(),
      edge_runtime_version()
    );

    assert_eq!(
      user_agent_for_project(Some("abcdefghijklmnopqrst")),
      expected
    );
    assert_eq!(user_agent_for_project(None), user_agent());
  }

  #[test]
  fn test_sanitize_project_ref() {
    assert_eq!(
      sanitize_project_ref("abcdefghijklmnopqrst"),
      Some("abcdefghijklmnopqrst")
    );
    assert_eq!(
      sanitize_project_ref("with-dash_and_1"),
      Some("with-dash_and_1")
    );

    assert_eq!(sanitize_project_ref(""), None);
    assert_eq!(sanitize_project_ref("has space"), None);
    assert_eq!(sanitize_project_ref("closes)comment"), None);
    assert_eq!(sanitize_project_ref("new\nline"), None);
    assert_eq!(sanitize_project_ref(&"a".repeat(65)), None);
  }
}
