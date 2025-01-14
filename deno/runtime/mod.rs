pub mod errors;
pub mod permissions;
pub mod shared;

pub fn exit(code: i32) -> ! {
  #[allow(clippy::disallowed_methods)]
  std::process::exit(code);
}
