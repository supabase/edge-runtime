use num_traits::NumCast;

// bytes size for 1 kibibyte
pub const KIB: u64 = 1_024;
// bytes size for 1 mebibyte
pub const MIB: u64 = 1_048_576;

pub fn mib_to_bytes(val: u64) -> u64 {
  val * MIB
}

fn get_in_format(byte_count: u64, conversion: u64, suffix: &str) -> String {
  let converted_value = byte_count / conversion;
  let decimal = (byte_count % conversion) * 100 / conversion;
  format!("{}.{:0>2}{}", converted_value, decimal, suffix)
}

pub fn bytes_to_display(val: u64) -> String {
  if val < MIB {
    get_in_format(val, KIB, "KiB")
  } else {
    get_in_format(val, MIB, "MiB")
  }
}

// A function that converts a milisecond elapsed time to a string that
// represents a human readable version of that time.
pub fn human_elapsed(elapsed: u64) -> String {
  if elapsed < 1_000 {
    return format!("{}ms", elapsed);
  }
  if elapsed < 1_000 * 60 {
    return format!("{}s", elapsed / 1000);
  }

  let seconds = elapsed / 1_000;
  let minutes = seconds / 60;
  let seconds_remainder = seconds % 60;
  format!("{}m{}s", minutes, seconds_remainder)
}

pub fn percentage_value<T, U>(value: U, percent: T) -> Option<U>
where
  T: NumCast + Ord,
  U: NumCast,
{
  let percent = std::cmp::min(percent, T::from(100)?).to_f64()?;
  let p = percent / 100.0f64;

  if p.is_normal() {
    U::from(value.to_f64()? * p)
  } else {
    None
  }
}

#[cfg(test)]
mod test {
  use super::*;

  // -- mib_to_bytes --

  #[test]
  fn mib_to_bytes_basic() {
    assert_eq!(mib_to_bytes(1), 1_048_576);
    assert_eq!(mib_to_bytes(256), 268_435_456);
    assert_eq!(mib_to_bytes(0), 0);
  }

  // -- bytes_to_display --

  #[test]
  fn bytes_to_display_kib() {
    assert_eq!(bytes_to_display(1024), "1.00KiB");
    assert_eq!(bytes_to_display(512), "0.50KiB");
    assert_eq!(bytes_to_display(0), "0.00KiB");
  }

  #[test]
  fn bytes_to_display_mib() {
    assert_eq!(bytes_to_display(MIB), "1.00MiB");
    assert_eq!(bytes_to_display(MIB * 256), "256.00MiB");
    assert_eq!(bytes_to_display(MIB + MIB / 2), "1.50MiB");
  }

  // -- human_elapsed --

  #[test]
  fn human_elapsed_milliseconds() {
    assert_eq!(human_elapsed(0), "0ms");
    assert_eq!(human_elapsed(500), "500ms");
    assert_eq!(human_elapsed(999), "999ms");
  }

  #[test]
  fn human_elapsed_seconds() {
    assert_eq!(human_elapsed(1000), "1s");
    assert_eq!(human_elapsed(5000), "5s");
    assert_eq!(human_elapsed(59_999), "59s");
  }

  #[test]
  fn human_elapsed_minutes() {
    assert_eq!(human_elapsed(60_000), "1m0s");
    assert_eq!(human_elapsed(90_000), "1m30s");
    assert_eq!(human_elapsed(125_000), "2m5s");
  }

  // -- percentage_value --

  #[test]
  fn percentage_value_basic() {
    assert_eq!(percentage_value(50u64, 1000u64), Some(500));
    assert_eq!(percentage_value(100u64, 256u64), Some(256));
  }

  #[test]
  fn percentage_value_caps_at_100() {
    // percent > 100 should be capped
    assert_eq!(percentage_value(200u64, 1000u64), Some(1000));
  }

  #[test]
  fn percentage_value_zero_percent_returns_none() {
    // 0% is not "normal" for f64 (it's subnormal), so returns None
    assert_eq!(percentage_value::<u64, u64>(0, 1000), None);
  }

  // -- constants --

  #[test]
  fn constants_correct() {
    assert_eq!(KIB, 1024);
    assert_eq!(MIB, 1024 * 1024);
  }
}
