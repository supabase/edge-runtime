use std::sync::Arc;

pub fn arc_u8_to_arc_str(
  arc_u8: Arc<[u8]>,
) -> Result<Arc<str>, std::str::Utf8Error> {
  // Check that the string is valid UTF-8.
  std::str::from_utf8(&arc_u8)?;
  // SAFETY: the string is valid UTF-8, and the layout Arc<[u8]> is the same as
  // Arc<str>. This is proven by the From<Arc<str>> impl for Arc<[u8]> from the
  // standard library.
  Ok(unsafe {
    std::mem::transmute::<std::sync::Arc<[u8]>, std::sync::Arc<str>>(arc_u8)
  })
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn valid_utf8_converts_successfully() {
    let bytes: Arc<[u8]> = Arc::from(b"hello world" as &[u8]);
    let result = arc_u8_to_arc_str(bytes).unwrap();
    assert_eq!(&*result, "hello world");
  }

  #[test]
  fn empty_bytes_converts_to_empty_str() {
    let bytes: Arc<[u8]> = Arc::from(b"" as &[u8]);
    let result = arc_u8_to_arc_str(bytes).unwrap();
    assert_eq!(&*result, "");
  }

  #[test]
  fn unicode_converts_successfully() {
    let bytes: Arc<[u8]> = Arc::from("日本語テスト".as_bytes());
    let result = arc_u8_to_arc_str(bytes).unwrap();
    assert_eq!(&*result, "日本語テスト");
  }

  #[test]
  fn invalid_utf8_returns_error() {
    let bytes: Arc<[u8]> = Arc::from(vec![0xff, 0xfe, 0xfd].as_slice());
    assert!(arc_u8_to_arc_str(bytes).is_err());
  }

  #[test]
  fn partial_utf8_returns_error() {
    // Valid start of a 3-byte sequence but incomplete
    let bytes: Arc<[u8]> = Arc::from(vec![0xe2, 0x82].as_slice());
    assert!(arc_u8_to_arc_str(bytes).is_err());
  }
}
