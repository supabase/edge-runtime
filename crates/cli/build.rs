use std::{env, path::Path};

fn main() {
    println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
    println!("cargo:rustc-env=PROFILE={}", env::var("PROFILE").unwrap());

    dotenv_build::output(dotenv_build::Config {
        filename: Path::new(".env.build"),
        recursive_search: false,
        fail_if_missing_dotenv: true,
    })
    .unwrap();
}
