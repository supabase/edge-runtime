use std::path::PathBuf;

use anyhow::bail;
use anyhow::Context;
use anyhow::Error;
use futures::StreamExt;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tracing::info;
use tracing::instrument;

/// A file pinned by URL and expected SHA-256 digest.
pub struct PinnedFile {
  pub kind: &'static str,
  pub cache_id: &'static str,
  pub url: &'static str,
  pub sha256: &'static str,
}

/// Root of the on-disk model cache.
///
/// Resolution order matches the previous ONNX Runtime based implementation
/// so hosts with a warm cache keep using it.
fn cache_root() -> Result<PathBuf, Error> {
  if let Some(dir) = std::env::var_os("EXT_AI_CACHE_DIR") {
    return Ok(PathBuf::from(dir));
  }

  if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
    .map(PathBuf::from)
    .filter(|it| it.is_absolute())
  {
    return Ok(dir);
  }

  let home = std::env::var_os("HOME")
    .map(PathBuf::from)
    .context("could not determine cache directory")?;

  if cfg!(target_os = "macos") {
    Ok(home.join("Library/Caches/ort.pyke.io"))
  } else {
    Ok(home.join(".cache/ort.pyke.io"))
  }
}

fn sha256_hex(bytes: &[u8]) -> String {
  faster_hex::hex_string(&Sha256::digest(bytes))
}

/// Returns the bytes of `file`, downloading it into the cache if missing.
///
/// The content is always verified against the pinned digest, both for cached
/// copies and fresh downloads, so a tampered cache or upstream file is
/// rejected instead of being loaded.
#[instrument(skip_all, fields(kind = file.kind, url = file.url))]
pub async fn fetch_pinned(file: &PinnedFile) -> Result<Vec<u8>, Error> {
  let url = reqwest::Url::parse(file.url).context("invalid URL")?;
  let filename = url
    .path_segments()
    .and_then(Iterator::last)
    .context("missing filename in URL")?
    .to_owned();
  let dir = cache_root()?.join(file.kind).join(file.cache_id);
  let path = dir.join(&filename);

  if let Ok(bytes) = tokio::fs::read(&path).await {
    if sha256_hex(&bytes) == file.sha256 {
      info!("using cached file");
      return Ok(bytes);
    }
    info!("cached file failed verification, downloading again");
  }

  tokio::fs::create_dir_all(&dir)
    .await
    .context("failed to create cache directory")?;

  info!("downloading file");
  let resp = reqwest::Client::builder()
    .http1_only()
    .build()
    .context("failed to create http client")?
    .get(url)
    .send()
    .await
    .context("failed to download")?
    .error_for_status()
    .context("failed to download")?;

  let mut bytes = Vec::new();
  let mut stream = resp.bytes_stream();
  while let Some(chunk) = stream.next().await {
    bytes.extend_from_slice(&chunk.context("failed to read response body")?);
  }

  let digest = sha256_hex(&bytes);
  if digest != file.sha256 {
    bail!(
      "checksum mismatch for {}: expected {}, got {digest}",
      file.url,
      file.sha256
    );
  }

  // Write to a temporary file and rename so concurrent readers never observe
  // a partially written file.
  let tmp_path = dir.join(format!("{filename}.{}.tmp", std::process::id()));
  let mut tmp = tokio::fs::File::create(&tmp_path)
    .await
    .context("failed to create file")?;
  tmp
    .write_all(&bytes)
    .await
    .context("failed to write file")?;
  tmp.flush().await.context("failed to write file")?;
  drop(tmp);
  tokio::fs::rename(&tmp_path, &path)
    .await
    .context("failed to move file into cache")?;

  info!(bytes = bytes.len(), "downloaded file");
  Ok(bytes)
}
