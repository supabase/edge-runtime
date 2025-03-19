use std::fs::File;
use std::path::Path;

fn main() {
  let env_file = "tests/.env";
  let env_path = Path::new(env_file);

  println!("cargo::rustc-check-cfg=cfg(dotenv)");

  if env_path.exists() {
    println!("cargo:rustc-cfg=dotenv")
  } else {
    File::create_new(env_path).unwrap();
  }

  println!("cargo::rerun-if-changed={}", env_file);
}
