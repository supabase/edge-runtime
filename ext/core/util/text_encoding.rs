// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::ModuleCodeString;
use encoding_rs::*;
use std::borrow::Cow;
use std::io::Error;
use std::io::ErrorKind;

pub const BOM_CHAR: char = '\u{FEFF}';

/// Attempts to detect the character encoding of the provided bytes.
///
/// Supports UTF-8, UTF-16 Little Endian and UTF-16 Big Endian.
pub fn detect_charset(bytes: &'_ [u8]) -> &'static str {
    const UTF16_LE_BOM: &[u8] = b"\xFF\xFE";
    const UTF16_BE_BOM: &[u8] = b"\xFE\xFF";

    if bytes.starts_with(UTF16_LE_BOM) {
        "utf-16le"
    } else if bytes.starts_with(UTF16_BE_BOM) {
        "utf-16be"
    } else {
        // Assume everything else is utf-8
        "utf-8"
    }
}

/// Attempts to convert the provided bytes to a UTF-8 string.
///
/// Supports all encodings supported by the encoding_rs crate, which includes
/// all encodings specified in the WHATWG Encoding Standard, and only those
/// encodings (see: <https://encoding.spec.whatwg.org/>).
pub fn convert_to_utf8<'a>(bytes: &'a [u8], charset: &'_ str) -> Result<Cow<'a, str>, Error> {
    match Encoding::for_label(charset.as_bytes()) {
        Some(encoding) => encoding
            .decode_without_bom_handling_and_without_replacement(bytes)
            .ok_or_else(|| ErrorKind::InvalidData.into()),
        None => Err(Error::new(
            ErrorKind::InvalidInput,
            format!("Unsupported charset: {charset}"),
        )),
    }
}

/// Strips the byte order mark from the provided text if it exists.
pub fn strip_bom(text: &str) -> &str {
    if text.starts_with(BOM_CHAR) {
        &text[BOM_CHAR.len_utf8()..]
    } else {
        text
    }
}

static SOURCE_MAP_PREFIX: &[u8] = b"//# sourceMappingURL=data:application/json;base64,";

#[allow(deprecated)]
pub fn source_map_from_code(code: &ModuleCodeString) -> Option<Vec<u8>> {
    let bytes = code.as_bytes();
    let last_line = bytes.rsplit(|u| *u == b'\n').next()?;
    if last_line.starts_with(SOURCE_MAP_PREFIX) {
        let input = last_line.split_at(SOURCE_MAP_PREFIX.len()).1;
        let decoded_map =
            base64::decode(input).expect("Unable to decode source map from emitted file.");
        Some(decoded_map)
    } else {
        None
    }
}

/// Truncate the source code before the source map.
pub fn code_without_source_map(mut code: ModuleCodeString) -> ModuleCodeString {
    let bytes = code.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'\n' {
            if bytes[i + 1..].starts_with(SOURCE_MAP_PREFIX) {
                code.truncate(i + 1);
            }
            return code;
        }
    }
    code
}
