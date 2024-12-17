use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=tests/.env");
    if Path::new("./tests/.env").exists() {
        println!("cargo:rustc-cfg=dotenv")
    }
}
