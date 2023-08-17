// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::error::AnyError;
use deno_core::serde_json;
use std::io::Write;

/// A function that converts a float to a string the represents a human
/// readable version of that number.
pub fn human_size(size: f64) -> String {
    let negative = if size.is_sign_positive() { "" } else { "-" };
    let size = size.abs();
    let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    if size < 1_f64 {
        return format!("{}{}{}", negative, size, "B");
    }
    let delimiter = 1024_f64;
    let exponent = std::cmp::min(
        (size.ln() / delimiter.ln()).floor() as i32,
        (units.len() - 1) as i32,
    );
    let pretty_bytes = format!("{:.2}", size / delimiter.powi(exponent))
        .parse::<f64>()
        .unwrap()
        * 1_f64;
    let unit = units[exponent as usize];
    format!("{negative}{pretty_bytes}{unit}")
}

const BYTES_TO_KIB: u64 = 2u64.pow(10);
const BYTES_TO_MIB: u64 = 2u64.pow(20);

/// Gets the size used for downloading data. The total bytes is used to
/// determine the units to use.
pub fn human_download_size(byte_count: u64, total_bytes: u64) -> String {
    return if total_bytes < BYTES_TO_MIB {
        get_in_format(byte_count, BYTES_TO_KIB, "KiB")
    } else {
        get_in_format(byte_count, BYTES_TO_MIB, "MiB")
    };

    fn get_in_format(byte_count: u64, conversion: u64, suffix: &str) -> String {
        let converted_value = byte_count / conversion;
        let decimal = (byte_count % conversion) * 100 / conversion;
        format!("{converted_value}.{decimal:0>2}{suffix}")
    }
}

/// A function that converts a millisecond elapsed time to a string that
/// represents a human readable version of that time.
pub fn human_elapsed(elapsed: u128) -> String {
    if elapsed < 1_000 {
        return format!("{elapsed}ms");
    }
    if elapsed < 1_000 * 60 {
        return format!("{}s", elapsed / 1000);
    }

    let seconds = elapsed / 1_000;
    let minutes = seconds / 60;
    let seconds_remainder = seconds % 60;
    format!("{minutes}m{seconds_remainder}s")
}

pub fn write_to_stdout_ignore_sigpipe(bytes: &[u8]) -> Result<(), std::io::Error> {
    use std::io::ErrorKind;

    match std::io::stdout().write_all(bytes) {
        Ok(()) => Ok(()),
        Err(e) => match e.kind() {
            ErrorKind::BrokenPipe => Ok(()),
            _ => Err(e),
        },
    }
}

pub fn write_json_to_stdout<T>(value: &T) -> Result<(), AnyError>
where
    T: ?Sized + serde::ser::Serialize,
{
    let mut writer = std::io::BufWriter::new(std::io::stdout());
    serde_json::to_writer_pretty(&mut writer, value)?;
    writeln!(&mut writer)?;
    Ok(())
}
