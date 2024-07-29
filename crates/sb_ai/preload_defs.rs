use std::{borrow::Cow, io::Write, path::PathBuf, process::ExitCode};

use anyhow::Context;
use clap::{ArgAction, Parser};
use deno_core::{error::AnyError, serde_json};
use serde::Deserialize;
use tokio::fs;
use tracing_subscriber::EnvFilter;

/// Preload the defs of sb_ai
#[derive(Debug, Parser)]
struct Cli {
    /// Path to the JSON file containing the def list to preload.
    #[arg(short = 'p', env = "PRELOAD_DEFS_PATH")]
    path: Option<PathBuf>,

    /// Load all defs regardless of failure.
    #[arg(long, action = ArgAction::SetFalse)]
    no_fail_fast: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    variation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PreloadManifest {
    Primitive(Manifest),
    List(Vec<Manifest>),
}

impl PreloadManifest {
    fn into_vec(self) -> Vec<Manifest> {
        match self {
            Self::Primitive(m) => vec![m],
            Self::List(vec) => vec,
        }
    }
}

#[tokio::main]
async fn main() -> Result<ExitCode, AnyError> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut stdout = std::io::stdout();

    let args = Cli::parse();
    let manifest_str: Cow<'static, str> = if let Some(path) = args.path {
        fs::read_to_string(&path)
            .await
            .context("failed to load manifest file")?
            .into()
    } else {
        include_str!("preload.json").into()
    };

    let manifests = serde_json::from_str::<PreloadManifest>(&manifest_str)
        .context("failed to parse manifest file")?
        .into_vec();

    let mut loaded = 0;
    let mut failed = 0;

    println!("loading {} defs", manifests.len());
    for Manifest { name, variation } in manifests {
        print!("{name}");
        if let Some(var) = variation.as_ref() {
            print!("(with {var})");
        }
        print!(" ... ");
        stdout.flush().unwrap();

        if let Err(err) = sb_ai::defs::init(None, name.as_str(), variation).await {
            failed += 1;

            println!("FAILED");
            if args.no_fail_fast {
                println!("{err:?}");
            } else {
                return Err(err);
            }
        } else {
            println!("ok");
            loaded += 1;
        }
    }

    println!();
    println!(
        "load result: {}. {} loaded; {} failed",
        if failed > 0 { "FAILED" } else { "ok" },
        loaded,
        failed
    );

    let exit_code = if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };

    Ok(exit_code)
}
