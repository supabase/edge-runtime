use std::path::Path;

fn main() {
    dbg!(Path::new("./tests/.env").exists());
    if Path::new("./tests/.env").exists() {
        println!("cargo:rustc-cfg=dotenv")
    }
}
