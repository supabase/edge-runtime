use std::fs::read_dir;
use std::path::Path;
use std::path::PathBuf;

use anyhow::bail;
use base::worker::TerminationToken;
use base::worker::WorkerSurfaceBuilder;
use deno_ast::MediaType;
use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_core::ModuleSpecifier;
use eszip::v2::EszipV2Module;
use eszip::v2::EszipV2SourceSlot;
use graph::eszip_migrate;
use graph::payload_to_eszip;
use graph::EszipPayloadKind;
use sb_workers::context::UserWorkerMsgs;
use sb_workers::context::UserWorkerRuntimeOpts;
use sb_workers::context::WorkerContextInitOpts;
use sb_workers::context::WorkerRuntimeOpts;
use tokio::fs::read;
use tokio::fs::write;
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::test]
async fn test_eszip_migration() {
    let paths = get_testdata_paths().unwrap();
    let mut passed = 0;
    let mut failed = 0;
    let mut snapshot_created = 0;

    println!("running {} eszip tests", paths.len());
    for path in paths {
        assert!(path.is_file());
        let result = test_eszip_migration_inner(&path).await;
        let snapshot_path = PathBuf::from(format!("{}.snapshot", path.to_string_lossy()));
        print!("eszip test {} ... ", path.to_string_lossy());
        let expected = match result {
            Ok(_) => String::new(),
            Err(err) => format!("{err:#}"),
        };
        let buf = expected.as_bytes();
        if !snapshot_path.exists() {
            write(snapshot_path, buf).await.unwrap();
            println!("snapshot created");
            passed += 1;
            snapshot_created += 1;
        } else {
            let snapshot_buf = read(snapshot_path).await.unwrap();
            if snapshot_buf == buf {
                println!("ok");
                passed += 1;
            } else {
                println!("FAILED");
                failed += 1;
            }
        }
    }
    let status = if failed > 0 { "FAILED" } else { "ok" };
    let msg =
        format!("eszip test result: {status}. {passed} passed ({snapshot_created} snapshot created); {failed} failed");
    if failed > 0 {
        panic!("{msg}");
    } else {
        eprintln!("{msg}");
    }
}

async fn test_eszip_migration_inner<P>(path: P) -> Result<(), AnyError>
where
    P: AsRef<Path>,
{
    let buf = read(path.as_ref()).await?;
    let eszip = EszipPayloadKind::VecKind(buf.clone());
    let (maybe_entrypoint, maybe_import_map_path) =
        guess_eszip_entrypoint_and_import_map_path(path, buf).await?;
    let termination_token = TerminationToken::new();
    let (pool_msg_tx, mut pool_msg_rx) = mpsc::unbounded_channel();
    let worker_surface = WorkerSurfaceBuilder::new()
        .termination_token(termination_token.clone())
        .init_opts(WorkerContextInitOpts {
            service_path: PathBuf::from("meow"),
            no_module_cache: false,
            // XXX: This seems insufficient as it may rely on the env contained in
            // Edge Functions' metadata.
            env_vars: std::env::vars().collect(),
            conf: WorkerRuntimeOpts::UserWorker(UserWorkerRuntimeOpts {
                service_path: Some(String::from("meow")),
                key: Some(Uuid::new_v4()),
                pool_msg_tx: Some(pool_msg_tx),
                events_msg_tx: None,
                cancel: None,
                ..Default::default()
            }),
            static_patterns: vec![],
            import_map_path: maybe_import_map_path,
            timing: None,
            maybe_eszip: Some(eszip),
            maybe_module_code: None,
            maybe_entrypoint,
            maybe_decorator: None,
            maybe_jsx_import_source_config: None,
            maybe_s3_fs_config: None,
            maybe_tmp_fs_config: None,
        })
        .build()
        .await;

    tokio::spawn({
        let token = termination_token.clone();
        async move {
            while let Some(msg) = pool_msg_rx.recv().await {
                if matches!(msg, UserWorkerMsgs::Shutdown(_)) {
                    token.outbound.cancel();
                    break;
                }
            }
        }
    });

    termination_token.cancel_and_wait().await;
    worker_surface.map(|_| ())
}

async fn guess_eszip_entrypoint_and_import_map_path<P>(
    path: P,
    buf: Vec<u8>,
) -> Result<(Option<String>, Option<String>), AnyError>
where
    P: AsRef<Path>,
{
    let metadata_path = PathBuf::from(format!("{}.metadata", &path.as_ref().to_string_lossy()));
    let (metadata_entrypoint, metadata_import_map_path) = if metadata_path.exists() {
        fn get_str(value: &serde_json::Value, key: &str) -> Option<String> {
            value
                .get(key)
                .and_then(|it| it.as_str().map(str::to_string))
        }

        let buf = read(metadata_path).await?;
        let metadata = serde_json::from_slice::<serde_json::Value>(&buf)?;
        (
            get_str(&metadata, "entrypoint"),
            get_str(&metadata, "importMap"),
        )
    } else {
        (None, None)
    };
    let mut eszip = eszip_migrate::try_migrate_if_needed(
        payload_to_eszip(EszipPayloadKind::VecKind(buf)).await?,
    )
    .await?;

    eszip.ensure_read_all().await?;

    let entries = eszip.modules.0.lock().unwrap();
    let import_map_path = metadata_import_map_path.or(entries
        .keys()
        .find(|it| {
            it.starts_with("file://")
                && (it.ends_with("import_map.json") || it.ends_with("deno.json"))
        })
        .cloned());
    let has_inline_source_code = entries
        .keys()
        .any(|it| it == eszip_async_trait::SOURCE_CODE_ESZIP_KEY);

    // for (key, value) in entries.iter() {
    //     if !key.starts_with("file://") {
    //         continue;
    //     }
    //     let EszipV2Module::Module {
    //         source: EszipV2SourceSlot::Ready(buf),
    //         ..
    //     } = value
    //     else {
    //         continue;
    //     };
    //     let source = String::from_utf8_lossy(buf);
    //     println!("--");
    //     println!("{key}");
    //     println!("{source}");
    //     println!("--");
    // }

    let mut indexes = vec![];
    if let Some(entrypoint) = metadata_entrypoint.and_then(|it| ModuleSpecifier::parse(&it).ok()) {
        indexes.push(entrypoint);
    } else {
        for key in entries.keys() {
            if !key.starts_with("file://") {
                continue;
            }
            let Ok(key) = ModuleSpecifier::parse(key) else {
                continue;
            };
            let Ok(path) = key.to_file_path() else {
                continue;
            };
            match MediaType::from_path(&path) {
                MediaType::JavaScript
                | MediaType::Mjs
                | MediaType::Jsx
                | MediaType::TypeScript
                | MediaType::Tsx => {}
                _ => continue,
            }
            if path.file_stem().unwrap() == "index" {
                indexes.push(key);
            }
        }
    }
    if has_inline_source_code && indexes.len() != 1 {
        let value = entries
            .get(eszip_async_trait::SOURCE_CODE_ESZIP_KEY)
            .unwrap();
        let EszipV2Module::Module {
            source: EszipV2SourceSlot::Ready(buf),
            ..
        } = value
        else {
            bail!("invalid inline source code");
        };
        let code = String::from_utf8_lossy(buf);
        for (key, value) in entries.iter() {
            if !key.starts_with("file://") {
                continue;
            }
            let EszipV2Module::Module {
                source: EszipV2SourceSlot::Ready(buf),
                ..
            } = value
            else {
                continue;
            };
            let target = String::from_utf8_lossy(buf);
            if code == target || target.starts_with(&*code) || code.starts_with(&*target) {
                return Ok((Some(key.clone()), import_map_path));
            }
        }
        Ok((Some(String::from("file:///src/index.ts")), import_map_path))
    } else if indexes.len() != 1 {
        Ok((Some(String::from("file:///src/index.ts")), import_map_path))
    } else {
        Ok((Some(indexes.first().unwrap().to_string()), import_map_path))
    }
}

fn get_testdata_paths() -> Result<Vec<PathBuf>, AnyError> {
    let mut paths = vec![];
    let dir_path = std::env::var("TESTDATA_DIR")
        .map(PathBuf::from)
        .unwrap_or(PathBuf::from("./tests/fixture/testdata"));
    if !dir_path.exists() || !dir_path.is_dir() {
        return Ok(paths);
    }
    let dir = read_dir(dir_path)?;
    for entry in dir {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let filename = entry.file_name();
        let filename = filename.to_string_lossy();
        if metadata.is_file()
            && (!filename.ends_with(".snapshot") && !filename.ends_with(".metadata"))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}
