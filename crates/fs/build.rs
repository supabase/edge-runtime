use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

fn main() {
  let env_file = "tests/.env";
  let env_path = Path::new(env_file);

  println!("cargo::rustc-check-cfg=cfg(dotenv)");

  if env_path.exists() {
    println!("cargo:rustc-cfg=dotenv")
  } else if let Err(err) = File::create_new(env_path) {
    // A concurrent build (e.g. rust-analyzer checking the crate while cargo
    // builds it from its own target dir) may have created the file between
    // the `exists` check and here — that's fine, it exists either way.
    if err.kind() != ErrorKind::AlreadyExists {
      panic!("failed to create {env_file}: {err}");
    }
  }

  println!("cargo::rerun-if-changed={}", env_file);
}
