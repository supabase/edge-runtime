use thiserror::Error;

#[derive(Error, Debug)]
pub enum EszipError {
  #[error("unsupported supabase eszip version (expected {expected:?}, found {found:?})")]
  UnsupportedVersion {
    expected: &'static [u8],
    found: Option<Vec<u8>>,
  },
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn unsupported_version_error_display_with_found() {
    let err = EszipError::UnsupportedVersion {
      expected: b"v2",
      found: Some(b"v1".to_vec()),
    };
    let msg = err.to_string();
    assert!(msg.contains("unsupported supabase eszip version"));
    assert!(msg.contains("expected"));
    assert!(msg.contains("found"));
  }

  #[test]
  fn unsupported_version_error_display_without_found() {
    let err = EszipError::UnsupportedVersion {
      expected: b"v2",
      found: None,
    };
    let msg = err.to_string();
    assert!(msg.contains("None"));
  }
}
