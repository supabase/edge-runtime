use std::{borrow::Cow, env, io::Write, path::PathBuf, process::ExitCode};

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

    /// Path to save an .env file containing the urls loaded from preload.
    #[arg(short = 'o', env = "PRELOAD_OUTPUT_ENV_PATH")]
    output_env_path: Option<PathBuf>,

    /// Load all defs regardless of failure.
    #[arg(long, action = ArgAction::SetFalse)]
    no_fail_fast: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    variation: Option<String>,
    model_url: String,
    tokenizer_url: String,
    config_url: Option<String>,
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
    for manifest_item in manifests {
        print!("{}", manifest_item.name);

        let model_url_key = sb_ai::compose_url_env_key(
            "model_url",
            &manifest_item.name,
            manifest_item.variation.as_deref(),
        );
        let tokenizer_url_key = sb_ai::compose_url_env_key(
            "tokenizer_url",
            &manifest_item.name,
            manifest_item.variation.as_deref(),
        );

        // TODO: set_var should be encapsulate in unsafe block in later rust versions.
        env::set_var(model_url_key, manifest_item.model_url);
        env::set_var(tokenizer_url_key, manifest_item.tokenizer_url);

        if let Some(config_url) = manifest_item.config_url {
            let config_url_key = sb_ai::compose_url_env_key(
                "config_url",
                &manifest_item.name,
                manifest_item.variation.as_deref(),
            );

            env::set_var(config_url_key, config_url);
        }

        if let Some(var) = manifest_item.variation.as_ref() {
            print!("(with {var})");
        }
        print!(" ... ");
        stdout.flush().unwrap();

        if let Err(err) =
            sb_ai::defs::init(None, manifest_item.name.as_str(), manifest_item.variation).await
        {
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

    if let Some(output_env_path) = args.output_env_path.filter(|_| failed == 0) {
        let sb_var_prefix = env!("CARGO_PKG_NAME").to_uppercase();
        let sb_vars = std::env::vars()
            .filter_map(|(key, value)| {
                if !key.starts_with(&sb_var_prefix) {
                    return None;
                }

                Some(format!("{key}={value}"))
            })
            .collect::<Vec<_>>();

        println!(
            "saving {} enviroment variables to {output_env_path:?}",
            sb_vars.len()
        );

        if let Err(err) = fs::write(output_env_path, sb_vars.join("\r\n")).await {
            failed += 1;

            println!("{err:?}");
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
