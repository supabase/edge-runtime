// DEPRECATED
// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

//! There are many types of errors in Deno:
//! - AnyError: a generic wrapper that can encapsulate any type of error.
//! - JsError: a container for the error message and stack trace for exceptions
//!   thrown in JavaScript code. We use this to pretty-print stack traces.
//! - Diagnostic: these are errors that originate in TypeScript's compiler.
//!   They're similar to JsError, in that they have line numbers. But
//!   Diagnostics are compile-time type errors, whereas JsErrors are runtime
//!   exceptions.

use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_core::url;
use deno_core::ModuleResolutionError;
use deno_fetch::reqwest;
use std::env;
use std::error::Error;
use std::io;
use std::sync::Arc;

fn get_env_var_error_class(error: &env::VarError) -> &'static str {
  use env::VarError::*;
  match error {
    NotPresent => "NotFound",
    NotUnicode(..) => "InvalidData",
  }
}

fn get_io_error_class(error: &io::Error) -> &'static str {
  use io::ErrorKind::*;
  match error.kind() {
    NotFound => "NotFound",
    PermissionDenied => "PermissionDenied",
    ConnectionRefused => "ConnectionRefused",
    ConnectionReset => "ConnectionReset",
    ConnectionAborted => "ConnectionAborted",
    NotConnected => "NotConnected",
    AddrInUse => "AddrInUse",
    AddrNotAvailable => "AddrNotAvailable",
    BrokenPipe => "BrokenPipe",
    AlreadyExists => "AlreadyExists",
    InvalidInput => "TypeError",
    InvalidData => "InvalidData",
    TimedOut => "TimedOut",
    Interrupted => "Interrupted",
    WriteZero => "WriteZero",
    UnexpectedEof => "UnexpectedEof",
    Other => "Error",
    WouldBlock => "WouldBlock",
    // Non-exhaustive enum - might add new variants
    // in the future
    _ => "Error",
  }
}

fn get_module_resolution_error_class(
  _: &ModuleResolutionError,
) -> &'static str {
  "URIError"
}

fn get_request_error_class(error: &reqwest::Error) -> &'static str {
  error
    .source()
    .and_then(|inner_err| {
      (inner_err
        .downcast_ref::<io::Error>()
        .map(get_io_error_class))
      .or_else(|| {
        inner_err
          .downcast_ref::<serde_json::error::Error>()
          .map(get_serde_json_error_class)
      })
      .or_else(|| {
        inner_err
          .downcast_ref::<url::ParseError>()
          .map(get_url_parse_error_class)
      })
    })
    .unwrap_or("Http")
}

fn get_serde_json_error_class(
  error: &serde_json::error::Error,
) -> &'static str {
  use deno_core::serde_json::error::*;
  match error.classify() {
    Category::Io => error
      .source()
      .and_then(|e| e.downcast_ref::<io::Error>())
      .map(get_io_error_class)
      .unwrap(),
    Category::Syntax => "SyntaxError",
    Category::Data => "InvalidData",
    Category::Eof => "UnexpectedEof",
  }
}

fn get_url_parse_error_class(_error: &url::ParseError) -> &'static str {
  "URIError"
}

fn get_hyper_error_class(_error: &hyper::Error) -> &'static str {
  "Http"
}

pub fn get_error_class_name(e: &AnyError) -> Option<&'static str> {
  deno_core::error::get_custom_error_class(e)
    .or_else(|| deno_web::get_error_class_name(e))
    .or_else(|| deno_websocket::get_network_error_class_name(e))
    .or_else(|| e.downcast_ref::<hyper::Error>().map(get_hyper_error_class))
    .or_else(|| {
      e.downcast_ref::<Arc<hyper::Error>>()
        .map(|e| get_hyper_error_class(e))
    })
    .or_else(|| {
      e.downcast_ref::<deno_core::Canceled>().map(|e| {
        let io_err: io::Error = e.to_owned().into();
        get_io_error_class(&io_err)
      })
    })
    .or_else(|| {
      e.downcast_ref::<env::VarError>()
        .map(get_env_var_error_class)
    })
    .or_else(|| e.downcast_ref::<io::Error>().map(get_io_error_class))
    .or_else(|| {
      e.downcast_ref::<ModuleResolutionError>()
        .map(get_module_resolution_error_class)
    })
    .or_else(|| {
      e.downcast_ref::<reqwest::Error>()
        .map(get_request_error_class)
    })
    .or_else(|| {
      e.downcast_ref::<serde_json::error::Error>()
        .map(get_serde_json_error_class)
    })
    .or_else(|| {
      e.downcast_ref::<url::ParseError>()
        .map(get_url_parse_error_class)
    })
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn io_error_not_found() {
    let err = io::Error::new(io::ErrorKind::NotFound, "file not found");
    assert_eq!(get_io_error_class(&err), "NotFound");
  }

  #[test]
  fn io_error_permission_denied() {
    let err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
    assert_eq!(get_io_error_class(&err), "PermissionDenied");
  }

  #[test]
  fn io_error_connection_refused() {
    let err = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
    assert_eq!(get_io_error_class(&err), "ConnectionRefused");
  }

  #[test]
  fn io_error_connection_reset() {
    let err = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
    assert_eq!(get_io_error_class(&err), "ConnectionReset");
  }

  #[test]
  fn io_error_timed_out() {
    let err = io::Error::new(io::ErrorKind::TimedOut, "timeout");
    assert_eq!(get_io_error_class(&err), "TimedOut");
  }

  #[test]
  fn io_error_addr_in_use() {
    let err = io::Error::new(io::ErrorKind::AddrInUse, "in use");
    assert_eq!(get_io_error_class(&err), "AddrInUse");
  }

  #[test]
  fn io_error_broken_pipe() {
    let err = io::Error::new(io::ErrorKind::BrokenPipe, "broken");
    assert_eq!(get_io_error_class(&err), "BrokenPipe");
  }

  #[test]
  fn io_error_invalid_input() {
    let err = io::Error::new(io::ErrorKind::InvalidInput, "bad input");
    assert_eq!(get_io_error_class(&err), "TypeError");
  }

  #[test]
  fn io_error_other() {
    let err = io::Error::new(io::ErrorKind::Other, "other");
    assert_eq!(get_io_error_class(&err), "Error");
  }

  #[test]
  fn env_var_not_present() {
    let err = env::VarError::NotPresent;
    assert_eq!(get_env_var_error_class(&err), "NotFound");
  }

  #[test]
  fn url_parse_error() {
    let err = url::ParseError::EmptyHost;
    assert_eq!(get_url_parse_error_class(&err), "URIError");
  }

  #[test]
  fn module_resolution_error() {
    let err = ModuleResolutionError::InvalidBaseUrl(url::ParseError::EmptyHost);
    assert_eq!(get_module_resolution_error_class(&err), "URIError");
  }

  #[test]
  fn serde_json_syntax_error() {
    let err = serde_json::from_str::<serde_json::Value>("{invalid")
      .unwrap_err();
    assert_eq!(get_serde_json_error_class(&err), "SyntaxError");
  }

  #[test]
  fn get_error_class_name_io_error() {
    let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
    let err: AnyError = io_err.into();
    assert_eq!(get_error_class_name(&err), Some("NotFound"));
  }

  #[test]
  fn get_error_class_name_env_var_error() {
    let err: AnyError = env::VarError::NotPresent.into();
    assert_eq!(get_error_class_name(&err), Some("NotFound"));
  }

  #[test]
  fn get_error_class_name_url_parse_error() {
    let err: AnyError = url::ParseError::EmptyHost.into();
    assert_eq!(get_error_class_name(&err), Some("URIError"));
  }

  #[test]
  fn get_error_class_name_unknown_error() {
    let err = deno_core::anyhow::anyhow!("some unknown error");
    // Unknown errors should return None (not classified)
    assert!(get_error_class_name(&err).is_none());
  }
}
