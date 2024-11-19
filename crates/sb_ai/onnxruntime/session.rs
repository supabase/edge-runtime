use deno_core::error::AnyError;
use futures::io::AllowStdIo;
use futures::StreamExt;
use once_cell::sync::Lazy;
use reqwest::Url;
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::Mutex;
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio_util::compat::FuturesAsyncWriteCompatExt;
use tracing::{debug, error, info, info_span, instrument, trace, Instrument};
use xxhash_rust::xxh3::Xxh3;

use anyhow::{anyhow, bail, Context, Error};
use ort::{
    CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider, ExecutionProviderDispatch,
    GraphOptimizationLevel, Session, SessionBuilder,
};

use crate::onnx::ensure_onnx_env_init;

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<Session>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(crate) fn get_session_builder() -> Result<SessionBuilder, AnyError> {
    let orm_threads = std::env::var("OMP_NUM_THREADS")
        .map_or(None, |val| val.parse::<usize>().ok())
        .unwrap_or(1);

    let builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        // NOTE(Nyannyacha): This is set to prevent memory leaks caused by different input
        // shapes.
        //
        // Backgrounds:
        // [1]: https://github.com/microsoft/onnxruntime/issues/11118
        // [2]: https://github.com/microsoft/onnxruntime/blob/main/onnxruntime/core/framework/session_options.h#L95-L110
        .with_memory_pattern(false)?
        .with_intra_threads(orm_threads)?;

    Ok(builder)
}

fn cpu_execution_provider() -> Box<dyn Iterator<Item = ExecutionProviderDispatch>> {
    Box::new(
        [
            // NOTE(Nyannacha): See the comment above. This makes `enable_cpu_mem_arena` set to
            // False.
            //
            // Backgrounds:
            // [1]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#9-18
            // [2]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#46-50
            CPUExecutionProvider::default().build(),
        ]
        .into_iter(),
    )
}

fn cuda_execution_provider() -> Box<dyn Iterator<Item = ExecutionProviderDispatch>> {
    let cuda = CUDAExecutionProvider::default();
    let providers = match cuda.is_available() {
        Ok(is_cuda_available) => {
            debug!(cuda_support = is_cuda_available);
            if is_cuda_available {
                vec![cuda.build()]
            } else {
                vec![]
            }
        }

        _ => vec![],
    };

    Box::new(providers.into_iter().chain(cpu_execution_provider()))
}

fn create_session(model_bytes: &[u8]) -> Result<Arc<Session>, Error> {
    let session = {
        if let Some(err) = ensure_onnx_env_init() {
            return Err(anyhow!("failed to create onnx environment: {err}"));
        }

        get_session_builder()?
            .with_execution_providers(cuda_execution_provider())?
            .commit_from_memory(model_bytes)?
    };

    let session = Arc::new(session);

    Ok(session)
}

#[instrument(level = "debug", ret)]
pub(crate) fn load_session_from_file(
    model_file_path: PathBuf,
) -> Result<(String, Arc<Session>), Error> {
    let session_id = fxhash::hash(&model_file_path.to_string_lossy()).to_string();

    let mut sessions = SESSIONS.lock().unwrap();

    if let Some(session) = sessions.get(&session_id) {
        trace!(session_id, "use existing session");
        return Ok((session_id, session.clone()));
    }
    let model_bytes = std::fs::read(model_file_path)?;

    let session = create_session(model_bytes.as_slice())?;

    trace!(session_id, "new session");
    sessions.insert(session_id.to_owned(), session.clone());

    Ok((session_id, session))
}

#[instrument(level = "debug", ret)]
pub(crate) fn load_session_from_bytes(model_bytes: &[u8]) -> Result<(String, Arc<Session>), Error> {
    let session_id = {
        let mut model_bytes = model_bytes;
        let mut hasher = Xxh3::new();
        let _ = std::io::copy(&mut model_bytes, &mut hasher);

        let hash = hasher.finish().to_be_bytes();
        faster_hex::hex_string(&hash)
    };

    let mut sessions = SESSIONS.lock().unwrap();

    if let Some(session) = sessions.get(&session_id) {
        return Ok((session_id, session.clone()));
    }

    trace!(session_id, "new session");
    let session = create_session(model_bytes)?;

    sessions.insert(session_id.to_owned(), session.clone());

    Ok((session_id, session))
}

#[instrument(level = "debug", ret)]
pub(crate) async fn load_session_from_url(model_url: Url) -> Result<(String, Arc<Session>), Error> {
    let session_id = fxhash::hash(model_url.as_str()).to_string();

    let mut sessions = SESSIONS.lock().unwrap();

    if let Some(session) = sessions.get(&session_id) {
        trace!(session_id, "use existing session");
        return Ok((session_id, session.clone()));
    }

    let model_file_path = fetch_and_cache_from_url(&model_url, Some(session_id.to_owned())).await?;

    let model_bytes = std::fs::read(model_file_path)?;

    let session = create_session(model_bytes.as_slice())?;

    trace!(session_id, "new session");
    sessions.insert(session_id.to_owned(), session.clone());

    Ok((session_id, session))
}

pub(crate) fn get_session(session_id: &String) -> Option<Arc<Session>> {
    let sessions = SESSIONS.lock().unwrap();

    sessions.get(session_id).cloned()
}

#[instrument(skip(url), fields(url = %url))]
async fn fetch_and_cache_from_url(
    url: &Url,
    cache_id: Option<String>,
) -> Result<PathBuf, AnyError> {
    let cache_id = cache_id.unwrap_or(fxhash::hash(url.as_str()).to_string());
    let download_dir = ort::sys::internal::dirs::cache_dir()
        .context("could not determine cache directory")?
        .join(&cache_id);

    std::fs::create_dir_all(&download_dir).context("could not able to create directories")?;

    let filename = url
        .path_segments()
        .and_then(Iterator::last)
        .context("missing filename in URL")?;

    let filepath = download_dir.join(filename);
    let checksum_path = filepath.with_file_name(format!("{filename}.checksum"));

    let is_filepath_exists = tokio::fs::try_exists(&filepath)
        .await
        .map_err(AnyError::from)?;

    let is_checksum_path_exists = tokio::fs::try_exists(&checksum_path)
        .await
        .map_err(AnyError::from)?;

    let is_checksum_valid = is_checksum_path_exists
        && 'scope: {
            let Some(old_checksum_str) = tokio::fs::read_to_string(&checksum_path).await.ok()
            else {
                break 'scope false;
            };

            let filepath = filepath.clone();
            let checksum = tokio::task::spawn_blocking(move || {
                let mut file = std::fs::File::open(filepath).ok()?;
                let mut hasher = Xxh3::new();
                let _ = std::io::copy(&mut file, &mut hasher).ok()?;

                Some(hasher.finish().to_be_bytes())
            })
            .await;

            let Ok(Some(checksum)) = checksum else {
                break 'scope false;
            };

            old_checksum_str == faster_hex::hex_string(&checksum)
        };

    let span = info_span!("download", filepath = %filepath.to_string_lossy());
    async move {
        if is_filepath_exists && is_checksum_valid {
            info!("binary already exists, skipping download");
            Ok(filepath.clone())
        } else {
            info!("downloading binary");

            if is_filepath_exists {
                let _ = tokio::fs::remove_file(&filepath).await;
            }

            use reqwest::*;

            let resp = Client::builder()
                .build()
                .context("failed to create http client")?
                .get(url.clone())
                .send()
                .await
                .context("failed to download")?;

            let len = resp
                .headers()
                .get(header::CONTENT_LENGTH)
                .map(|it| it.to_str().map_err(AnyError::new))
                .transpose()?
                .map(|it| it.parse::<usize>().map_err(AnyError::new))
                .transpose()?
                .context("invalid Content-Length header")?;

            debug!(total_bytes = len);

            let file = tokio::fs::File::create(&filepath)
                .await
                .context("failed to create file")?;

            let mut stream = resp.bytes_stream();
            let mut writer = tokio::io::BufWriter::new(file);
            let mut hasher = AllowStdIo::new(Xxh3::new()).compat_write();
            let mut written = 0;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("failed to get chunks")?;

                written += tokio::io::copy(&mut chunk.as_ref(), &mut writer)
                    .await
                    .context("failed to store chunks to file")?;

                let _ = tokio::io::copy(&mut chunk.as_ref(), &mut hasher)
                    .await
                    .context("failed to calculate checksum")?;

                trace!(bytes_written = written);
            }

            let checksum_str = {
                let hasher = hasher.into_inner().into_inner();
                faster_hex::hex_string(&hasher.finish().to_be_bytes())
            };

            if written == len as u64 {
                info!({ bytes_written = written, checksum = &checksum_str }, "done");

                let mut checksum_file = tokio::fs::File::create(&checksum_path)
                    .await
                    .context("failed to create checksum file")?;

                let _ = checksum_file
                    .write(checksum_str.as_bytes())
                    .await
                    .context("failed to write checksum to file system")?;

                Ok(filepath)
            } else {
                error!({ expected = len, got = written }, "bytes mismatch");
                bail!("error copying data to file: expected {len} length, but got {written}");
            }
        }
    }
    .instrument(span)
    .await
}

pub fn cleanup() -> Result<usize, AnyError> {
    let mut remove_counter = 0;
    {
        let mut guard = SESSIONS.lock().unwrap();
        let mut to_be_removed = vec![];

        for (key, session) in &mut *guard {
            // Since we're currently referencing the session at this point
            // It also will increments the counter, so we need to check: counter > 1
            if Arc::strong_count(session) > 1 {
                continue;
            }

            to_be_removed.push(key.clone());
        }

        for key in to_be_removed {
            let old_store = guard.remove(&key);
            debug_assert!(old_store.is_some());

            remove_counter += 1;
        }
    }

    Ok(remove_counter)
}
