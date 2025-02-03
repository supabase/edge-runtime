use std::path::Path;

fn main() {
  println!("cargo::rustc-check-cfg=cfg(dotenv)");
  println!("cargo::rerun-if-changed=tests/.env");
  if Path::new("./tests/.env").exists() {
    println!("cargo:rustc-cfg=dotenv")
  }
}
