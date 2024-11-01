use core::slice;
use std::{
    borrow::Cow,
    ffi::OsStr,
    io::{self, Cursor},
    mem,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    pin::Pin,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use aws_config::{retry::RetryConfig, AppName, BehaviorVersion, Region};
use aws_credential_types::{credential_fn::provide_credentials_fn, Credentials};
use aws_sdk_s3::{
    config::{SharedCredentialsProvider, SharedHttpClient},
    error::SdkError,
    operation::{
        head_object::HeadObjectError,
        upload_part::{UploadPartError, UploadPartOutput},
    },
    primitives::{ByteStream, DateTime},
    types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier},
    Config,
};
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use deno_core::{AsyncRefCell, BufMutView, BufView, RcRef, ResourceHandleFd, WriteOutcome};
use deno_fs::{AccessCheckCb, FsDirEntry, FsFileType, OpenOptions};
use deno_io::fs::{File, FsError, FsResult, FsStat};
use enum_as_inner::EnumAsInner;
use futures::{
    future::{BoxFuture, Shared},
    io::AllowStdIo,
    stream::FuturesUnordered,
    AsyncWriteExt, FutureExt, StreamExt, TryFutureExt,
};
use memmap2::{MmapOptions, MmapRaw};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use tempfile::tempfile;
use tokio::{
    io::{AsyncBufRead, AsyncReadExt},
    task::JoinError,
};
use tracing::{debug, debug_span, error, info_span, instrument, warn, Instrument};

use super::TryNormalizePath;

const MIN_PART_SIZE: usize = 1024 * 1024 * 5;

type BackgroundTask = Shared<BoxFuture<'static, Result<(), Arc<JoinError>>>>;

#[derive(Debug, Clone)]
pub struct S3Fs {
    client: aws_sdk_s3::Client,
    background_tasks: Arc<FuturesUnordered<BackgroundTask>>,
}

impl S3Fs {
    pub async fn try_flush_background_tasks(&self) -> bool {
        let Ok(mut background_tasks) = Arc::try_unwrap(self.background_tasks.clone()) else {
            return false;
        };

        loop {
            if background_tasks.next().await.is_none() {
                break;
            }
        }

        true
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct S3CredentialsObject {
    access_key_id: Cow<'static, str>,
    secret_access_key: Cow<'static, str>,
    expires_after: Option<u64>,
}

impl S3CredentialsObject {
    fn into_credentials(self) -> Credentials {
        Credentials::new(
            self.access_key_id,
            self.secret_access_key,
            None,
            self.expires_after
                .map(Duration::from_secs)
                .map(|it| UNIX_EPOCH + it),
            "EdgeRuntime",
        )
    }
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum RetryMode {
    #[default]
    Standard,
    Adaptive,
}

impl RetryMode {
    fn to_aws_retry_config_builder(self) -> aws_config::retry::RetryConfigBuilder {
        use aws_config::retry::*;

        RetryConfigBuilder::new().mode(match self {
            Self::Standard => RetryMode::Standard,
            Self::Adaptive => RetryMode::Adaptive,
        })
    }
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ReconnectMode {
    #[default]
    ReconnectOnTransientError,
    ReuseAllConnections,
}

impl ReconnectMode {
    fn to_s3(self) -> aws_sdk_s3::config::retry::ReconnectMode {
        match self {
            Self::ReconnectOnTransientError => {
                aws_sdk_s3::config::retry::ReconnectMode::ReconnectOnTransientError
            }
            Self::ReuseAllConnections => {
                aws_sdk_s3::config::retry::ReconnectMode::ReuseAllConnections
            }
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct S3ClientRetryConfig {
    mode: RetryMode,
    max_attempts: Option<u32>,
    initial_backoff_sec: Option<u64>,
    max_backoff_sec: Option<u64>,
    reconnect_mode: Option<ReconnectMode>,
    use_static_exponential_base: Option<bool>,
}

impl S3ClientRetryConfig {
    fn into_aws_retry_config(self) -> RetryConfig {
        self.mode
            .to_aws_retry_config_builder()
            .max_attempts(self.max_attempts.unwrap_or(3))
            .initial_backoff(
                self.initial_backoff_sec
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(1)),
            )
            .max_backoff(
                self.max_backoff_sec
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(20)),
            )
            .reconnect_mode(self.reconnect_mode.unwrap_or_default().to_s3())
            .build()
            .with_use_static_exponential_base(self.use_static_exponential_base.unwrap_or_default())
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct S3FsConfig {
    app_name: Option<Cow<'static, str>>,
    endpoint_url: Option<Cow<'static, str>>,
    region: Option<Cow<'static, str>>,
    credentials: S3CredentialsObject,
    force_path_style: Option<bool>,
    retry_config: Option<S3ClientRetryConfig>,
}

impl TryInto<S3Fs> for S3FsConfig {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<S3Fs, Self::Error> {
        S3Fs::new(self)
    }
}

impl S3FsConfig {
    fn try_into_s3_config(self) -> Result<Config, anyhow::Error> {
        let mut builder = Config::builder();

        if let Some(app_name) = self.app_name {
            builder.set_app_name(Some(AppName::new(app_name).context("invalid app name")?));
        }

        builder
            .set_endpoint_url(self.endpoint_url.map(Cow::into_owned))
            .set_force_path_style(self.force_path_style)
            .set_region(self.region.to_owned().map(Region::new))
            .set_credentials_provider({
                let cred = self.credentials.into_credentials();
                Some(SharedCredentialsProvider::new(provide_credentials_fn(
                    move || std::future::ready(Ok(cred.clone())),
                )))
            })
            .set_http_client(Some(Self::get_shared_http_client()))
            .set_behavior_version(Some(BehaviorVersion::latest()))
            .set_retry_config(
                self.retry_config
                    .map(S3ClientRetryConfig::into_aws_retry_config),
            );

        Ok(builder.build())
    }

    fn get_shared_http_client() -> SharedHttpClient {
        static CLIENT: OnceCell<SharedHttpClient> = OnceCell::new();

        CLIENT
            .get_or_init(|| HyperClientBuilder::new().build_https())
            .clone()
    }
}

impl S3Fs {
    pub fn new(config: S3FsConfig) -> Result<Self, anyhow::Error> {
        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(config.try_into_s3_config()?),
            background_tasks: Arc::default(),
        })
    }
}

#[async_trait::async_trait(?Send)]
impl deno_fs::FileSystem for S3Fs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    fn tmp_dir(&self) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    fn chdir(&self, _path: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn umask(&self, _mask: Option<u32>) -> FsResult<u32> {
        Err(FsError::NotSupported)
    }

    fn open_sync(
        &self,
        _path: &Path,
        _options: OpenOptions,
        _access_check: Option<AccessCheckCb>,
    ) -> FsResult<Rc<dyn File>> {
        Err(FsError::NotSupported)
    }

    async fn open_async<'a>(
        &'a self,
        path: PathBuf,
        options: OpenOptions,
        _access_check: Option<AccessCheckCb<'a>>,
    ) -> FsResult<Rc<dyn File>> {
        let (bucket_name, key) = try_get_bucket_name_and_key(path.try_normalize()?)?;

        Ok(Rc::new(S3Object {
            bucket_name,
            key,
            fs: self.clone(),
            open_options: options,
            op_slot: AsyncRefCell::default(),
        }))
    }

    fn mkdir_sync(&self, _path: &Path, _recursive: bool, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn mkdir_async(&self, path: PathBuf, recursive: bool, _mode: u32) -> FsResult<()> {
        let (bucket_name, key) = try_get_bucket_name_and_key(path.try_normalize()?)?;
        let keys = if recursive {
            PathBuf::from(key)
                .iter()
                .map({
                    let mut stack = vec![];
                    move |it| {
                        stack.push(it.to_string_lossy());
                        stack
                            .iter()
                            .cloned()
                            .chain([Cow::Borrowed("")])
                            .collect::<Vec<_>>()
                            .join("/")
                    }
                })
                .collect::<Vec<_>>()
        } else {
            vec![key]
        };

        let mut futs = FuturesUnordered::new();
        let mut errors = vec![];

        for folder_key in keys {
            debug_assert!(folder_key.ends_with('/'));

            let client = self.client.clone();
            let bucket_name = bucket_name.clone();

            futs.push(async move {
                let resp = client
                    .head_object()
                    .bucket(&bucket_name)
                    .key(&folder_key)
                    .send()
                    .await;

                if let Err(err) = resp {
                    if !matches!(err.as_service_error(), Some(HeadObjectError::NotFound(_))) {
                        return Err(io::Error::other(err));
                    }
                }

                client
                    .put_object()
                    .bucket(bucket_name)
                    .key(folder_key)
                    .body(ByteStream::from_static(&[]))
                    .send()
                    .await
                    .map_err(io::Error::other)
            });
        }

        while let Some(result) = futs.next().await {
            match result {
                Ok(_) => {}
                Err(err) => errors.push(err),
            }
        }

        to_combined_message(errors)
    }

    fn chmod_sync(&self, _path: &Path, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chmod_async(&self, _path: PathBuf, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn chown_sync(&self, _path: &Path, _uid: Option<u32>, _gid: Option<u32>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chown_async(
        &self,
        _path: PathBuf,
        _uid: Option<u32>,
        _gid: Option<u32>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn lchown_sync(&self, _path: &Path, _uid: Option<u32>, _gid: Option<u32>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn lchown_async(
        &self,
        _path: PathBuf,
        _uid: Option<u32>,
        _gid: Option<u32>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn remove_sync(&self, _path: &Path, _recursive: bool) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn remove_async(&self, path: PathBuf, recursive: bool) -> FsResult<()> {
        let had_slash = path.ends_with("/");
        let (bucket_name, key) = try_get_bucket_name_and_key(path.try_normalize()?)?;

        if recursive {
            let builder = self
                .client
                .list_objects_v2()
                .bucket(&bucket_name)
                .prefix(format!("{}/", key));

            let mut errors = vec![];
            let mut stream = builder.into_paginator().send();

            while let Some(resp) = stream.next().await {
                let v = match resp {
                    Ok(v) => v,
                    Err(err) => return Err(io::Error::other(err).into()),
                };

                match v.key_count() {
                    None => continue,
                    Some(v) if v <= 0 => continue,
                    _ => {}
                }

                let delete = Delete::builder()
                    .set_quiet(Some(true))
                    .set_objects(Some(
                        v.contents()
                            .iter()
                            .filter_map(|it| {
                                it.key()
                                    .and_then(|it| ObjectIdentifier::builder().key(it).build().ok())
                            })
                            .collect::<Vec<_>>(),
                    ))
                    .build()
                    .map_err(io::Error::other)?;

                let resp = self
                    .client
                    .delete_objects()
                    .bucket(&bucket_name)
                    .delete(delete)
                    .send()
                    .await;

                let v = match resp {
                    Ok(v) => v,
                    Err(err) => return Err(io::Error::other(err).into()),
                };

                if !v.errors().is_empty() {
                    errors.extend_from_slice(v.errors());
                }
            }

            return to_combined_message(errors.into_iter().map(|it| {
                format!(
                    "{}({}): {}",
                    it.key().unwrap_or("null"),
                    it.code().unwrap_or("unknown"),
                    it.message().unwrap_or("unknown error message")
                )
            }));
        }

        let _resp = self
            .client
            .delete_object()
            .bucket(bucket_name)
            .key(if had_slash { format!("{}/", key) } else { key })
            .send()
            .await
            .map_err(io::Error::other)?;

        Ok(())
    }

    fn copy_file_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn copy_file_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        // TODO
        Err(FsError::NotSupported)
    }

    fn cp_sync(&self, _path: &Path, _new_path: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn cp_async(&self, _path: PathBuf, _new_path: PathBuf) -> FsResult<()> {
        // TODO
        Err(FsError::NotSupported)
    }

    fn stat_sync(&self, _path: &Path) -> FsResult<FsStat> {
        Err(FsError::NotSupported)
    }

    async fn stat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        self.open_async(path.try_normalize()?, OpenOptions::read(), None)
            .and_then(|it| it.stat_async())
            .await
    }

    fn lstat_sync(&self, _path: &Path) -> FsResult<FsStat> {
        Err(FsError::NotSupported)
    }

    async fn lstat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        self.stat_async(path).await
    }

    fn realpath_sync(&self, _path: &Path) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    async fn realpath_async(&self, _path: PathBuf) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    fn read_dir_sync(&self, _path: &Path) -> FsResult<Vec<FsDirEntry>> {
        Err(FsError::NotSupported)
    }

    async fn read_dir_async(&self, path: PathBuf) -> FsResult<Vec<FsDirEntry>> {
        let (bucket_name, mut key) = try_get_bucket_name_and_key(path.try_normalize()?)?;

        debug_assert!(!key.ends_with('/'));
        key.push('/');

        let builder = self
            .client
            .list_objects_v2()
            .set_bucket(Some(bucket_name))
            .set_delimiter(Some("/".into()))
            .set_prefix(Some(key));

        let mut entries = vec![];
        let mut stream = builder.into_paginator().send();

        while let Some(resp) = stream.next().await {
            let v = resp.map_err(io::Error::other)?;
            let Some(prefix) = v.prefix() else {
                continue;
            };

            let common_prefixes = v.common_prefixes();
            let contents = v.contents();

            for common_prefix in common_prefixes {
                entries.push(FsDirEntry {
                    name: {
                        let Some(name) = common_prefix
                            .prefix()
                            .and_then(|it| it.strip_prefix(prefix))
                            .and_then(|it| it.strip_suffix('/'))
                            .map(str::to_owned)
                        else {
                            continue;
                        };

                        name
                    },

                    is_file: false,
                    is_directory: true,
                    is_symlink: false,
                });
            }

            for content in contents {
                let Some(name) = content.key() else {
                    continue;
                };

                if name.ends_with('/') {
                    continue;
                }

                entries.push(FsDirEntry {
                    name: {
                        let Some(name) = name.strip_prefix(prefix).map(str::to_owned) else {
                            continue;
                        };

                        name
                    },

                    is_file: true,
                    is_directory: false,
                    is_symlink: false,
                });
            }
        }

        Ok(entries)
    }

    fn rename_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn rename_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn link_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn link_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn symlink_sync(
        &self,
        _oldpath: &Path,
        _newpath: &Path,
        _file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn symlink_async(
        &self,
        _oldpath: PathBuf,
        _newpath: PathBuf,
        _file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn read_link_sync(&self, _path: &Path) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    async fn read_link_async(&self, _path: PathBuf) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    fn truncate_sync(&self, _path: &Path, _len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn truncate_async(&self, _path: PathBuf, _len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn utime_sync(
        &self,
        _path: &Path,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn utime_async(
        &self,
        _path: PathBuf,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn lutime_sync(
        &self,
        _path: &Path,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn lutime_async(
        &self,
        _path: PathBuf,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
}

#[derive(EnumAsInner)]
enum S3ObjectOpSlot {
    Read(S3ObjectReadState),
    Write(S3ObjectWriteState),
}

struct S3ObjectReadState(Pin<Box<dyn AsyncBufRead>>, usize);

#[derive(Debug)]
struct FileBackedMmapBuffer {
    cursor: AllowStdIo<Cursor<&'static mut [u8]>>,
    raw: MmapRaw,
    _file: std::fs::File,
}

impl FileBackedMmapBuffer {
    fn new() -> Result<Self, anyhow::Error> {
        let file = tempfile().context("could not create a temp file")?;
        let raw = MmapOptions::new()
            .len(MIN_PART_SIZE)
            .map_raw(file.as_raw_fd())
            .context("failed to create a file backed memory buffer")?;

        let cursor = AllowStdIo::new(Cursor::new(unsafe {
            slice::from_raw_parts_mut(raw.as_mut_ptr(), raw.len())
        }));

        Ok(Self {
            cursor,
            raw,
            _file: file,
        })
    }
}

#[derive(Debug)]
struct S3ObjectWriteState {
    buf: FileBackedMmapBuffer,
    total_written: usize,
    method: Option<S3WriteUploadMethod>,
}

impl std::ops::Deref for S3ObjectWriteState {
    type Target = FileBackedMmapBuffer;

    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

impl std::ops::DerefMut for S3ObjectWriteState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buf
    }
}

impl S3ObjectWriteState {
    fn new() -> Result<Self, anyhow::Error> {
        Ok(Self {
            buf: FileBackedMmapBuffer::new()?,
            total_written: 0,
            method: None,
        })
    }

    fn try_swap_buffer(&mut self) -> Result<FileBackedMmapBuffer, anyhow::Error> {
        Ok(mem::replace(&mut self.buf, FileBackedMmapBuffer::new()?))
    }
}

enum S3WriteErrorSubject {
    MultiPartUploadTask((i32, io::Error)),
    Join(tokio::task::JoinError),
}

impl std::fmt::Display for S3WriteErrorSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MultiPartUploadTask((idx, inner)) => write!(f, "{idx}: {inner}"),
            Self::Join(inner) => write!(f, "{inner:?}: {inner}"),
        }
    }
}

type BoxedUploadPartTask = BoxFuture<
    'static,
    Result<
        (
            i32,
            Result<UploadPartOutput, SdkError<UploadPartError, HttpResponse>>,
        ),
        JoinError,
    >,
>;

#[derive(Debug, Default)]
struct S3MultiPartUploadMethod {
    recent_part_idx: i32,
    upload_id: String,
    parts: Vec<CompletedPart>,
    tasks: FuturesUnordered<BoxedUploadPartTask>,
}

impl S3MultiPartUploadMethod {
    async fn sync(&mut self) -> FsResult<()> {
        let mut errors = vec![];

        while let Some(result) = self.tasks.next().await {
            match result {
                Err(err) => errors.push(S3WriteErrorSubject::Join(err)),
                Ok((part_idx, resp)) => match resp {
                    Ok(output) => {
                        let Some(e_tag) = output.e_tag else {
                            errors.push(S3WriteErrorSubject::MultiPartUploadTask((
                                part_idx,
                                io::Error::other(
                                    "no e-tag field was found in upload part response",
                                ),
                            )));

                            continue;
                        };

                        self.parts.push(
                            CompletedPart::builder()
                                .e_tag(e_tag)
                                .part_number(part_idx)
                                .build(),
                        );
                    }

                    Err(err) => errors.push(S3WriteErrorSubject::MultiPartUploadTask((
                        part_idx,
                        io::Error::other(err),
                    ))),
                },
            }
        }

        to_combined_message(errors)
    }
}

#[derive(Debug, EnumAsInner)]
enum S3WriteUploadMethod {
    PutObject,
    MultiPartUpload(S3MultiPartUploadMethod),
}

impl S3WriteUploadMethod {
    async fn sync(&mut self) -> FsResult<()> {
        if let Self::MultiPartUpload(multi_part) = self {
            multi_part.sync().await?
        }

        Ok(())
    }

    async fn cleanup(
        &mut self,
        fs: S3Fs,
        bucket_name: String,
        key: String,
        state: S3ObjectWriteState,
    ) -> FsResult<()> {
        match self {
            Self::MultiPartUpload(multi_part) => {
                if state.buf.cursor.get_ref().position() > 0 {
                    state.buf.raw.flush_async()?;
                    multi_part.tasks.push(
                        tokio::task::spawn({
                            let upload_id = multi_part.upload_id.clone();
                            let client = fs.client.clone();
                            let bucket_name = bucket_name.clone();
                            let key = key.clone();
                            let part_idx = multi_part.recent_part_idx;
                            let data = unsafe {
                                slice::from_raw_parts(
                                    state.buf.raw.as_ptr(),
                                    state.buf.cursor.get_ref().position() as usize,
                                )
                            };

                            multi_part.recent_part_idx += 1;

                            async move {
                                client
                                    .upload_part()
                                    .bucket(bucket_name)
                                    .key(key)
                                    .upload_id(upload_id)
                                    .part_number(part_idx)
                                    .body(ByteStream::new(data.into()))
                                    .send()
                                    .map(|it| (part_idx, it))
                                    .await
                            }
                            .instrument(debug_span!(
                                "upload part",
                                last = true,
                                part = part_idx
                            ))
                        })
                        .boxed(),
                    );

                    multi_part.sync().await?;
                }

                if multi_part.parts.is_empty() {
                    return Ok(());
                }

                debug_assert!(multi_part.tasks.is_empty());
                debug_assert!(multi_part.recent_part_idx > 0);

                fs.client
                    .complete_multipart_upload()
                    .bucket(bucket_name)
                    .key(key)
                    .upload_id(mem::take(&mut multi_part.upload_id))
                    .multipart_upload(
                        CompletedMultipartUpload::builder()
                            .set_parts(Some(mem::take(&mut multi_part.parts)))
                            .build(),
                    )
                    .send()
                    .await
                    .map_err(io::Error::other)?;
            }

            Self::PutObject => {
                fs.client
                    .put_object()
                    .bucket(bucket_name)
                    .key(key)
                    .body(ByteStream::new(
                        unsafe {
                            slice::from_raw_parts(state.buf.raw.as_ptr(), state.total_written)
                        }
                        .into(),
                    ))
                    .send()
                    .await
                    .map_err(io::Error::other)?;
            }
        }

        Ok(())
    }
}

pub struct S3Object {
    fs: S3Fs,
    op_slot: AsyncRefCell<Option<S3ObjectOpSlot>>,

    #[allow(unused)]
    open_options: OpenOptions,

    bucket_name: String,
    key: String,
}

impl Drop for S3Object {
    fn drop(&mut self) {
        if let Some(S3ObjectOpSlot::Write(mut state)) = Rc::new(mem::take(&mut self.op_slot))
            .try_borrow_mut()
            .unwrap()
            .take()
        {
            let Some(mut method) = state.method.take() else {
                return;
            };

            let fs = self.fs.clone();
            let bucket_name = mem::take(&mut self.bucket_name);
            let key = mem::take(&mut self.key);
            let span = info_span!(
                "background task",
                bucket_name = bucket_name.as_str(),
                key = key.as_str()
            );

            self.fs.background_tasks.push(
                tokio::task::spawn(
                    async move {
                        if let Err(err) = method.sync().await {
                            error!(reason = ?err, "sync operation failed");
                        }
                        if let Err(err) = method.cleanup(fs, bucket_name, key, state).await {
                            error!(reason = ?err, "cleanup operation failed");
                        }
                    }
                    .instrument(span),
                )
                .map_err(Arc::new)
                .boxed()
                .shared(),
            );
        }
    }
}

#[async_trait::async_trait(?Send)]
impl deno_io::fs::File for S3Object {
    fn read_sync(self: Rc<Self>, _buf: &mut [u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }

    #[instrument(level = "info", skip_all, fields(self.bucket_name, self.key))]
    async fn read_byob(self: Rc<Self>, mut buf: BufMutView) -> FsResult<(usize, BufMutView)> {
        let mut op_slot = RcRef::map(&self, |r| &r.op_slot).borrow_mut().await;
        let Some(op_slot_mut) = op_slot.as_mut() else {
            let resp = self
                .fs
                .client
                .get_object()
                .bucket(&self.bucket_name)
                .key(&self.key)
                .send()
                .await
                .map_err(io::Error::other)?;

            let mut body_buf = resp.body.into_async_read();
            let nread = body_buf.read(&mut buf).await?;

            *op_slot = Some(S3ObjectOpSlot::Read(S3ObjectReadState(
                Box::pin(body_buf),
                nread,
            )));

            return Ok((nread, buf));
        };

        let Some(state) = op_slot_mut.as_read_mut() else {
            return Err(io::Error::other("read operation was blocked by another operation").into());
        };

        let err = match state.0.read(&mut buf).await {
            Ok(nread) => {
                state.1 += nread;

                if nread == 0 {
                    op_slot.take();
                }

                return Ok((nread, buf));
            }

            Err(err) => err,
        };

        let is_retryable = {
            use io::ErrorKind as E;
            matches!(
                err.kind(),
                E::ConnectionRefused
                    | E::ConnectionReset
                    | E::ConnectionAborted
                    | E::BrokenPipe
                    | E::TimedOut
                    | E::NotConnected
            )
        };

        warn!(kind = %err.kind(), reason = ?err, "stream closed abnormally");
        debug!(is_retryable);

        if is_retryable {
            let resp = self
                .fs
                .client
                .get_object()
                .bucket(&self.bucket_name)
                .key(&self.key)
                .range(format!("{}-", state.1))
                .send()
                .await
                .map_err(io::Error::other)?;

            let mut body_buf = resp.body.into_async_read();
            let nread = body_buf.read(&mut buf).await?;

            state.1 += nread;
            state.0 = Box::pin(body_buf);
            Ok((nread, buf))
        } else {
            op_slot.take();
            Err(io::Error::other(err).into())
        }
    }

    fn write_sync(self: Rc<Self>, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }

    #[instrument(level = "info", skip_all, fields(self.bucket_name, self.key))]
    async fn write(self: Rc<Self>, buf: BufView) -> FsResult<WriteOutcome> {
        let mut op_slot = RcRef::map(&self, |r| &r.op_slot).borrow_mut().await;
        let Some(op_slot_mut) = op_slot.as_mut() else {
            let mut state = S3ObjectWriteState::new().map_err(io::Error::other)?;
            let size = buf.len();
            let nwritten = state.cursor.write(&buf).await?;
            let written = if size == nwritten {
                WriteOutcome::Full { nwritten }
            } else {
                assert!(size > nwritten);
                WriteOutcome::Partial {
                    nwritten,
                    view: buf,
                }
            };

            state.total_written += nwritten;
            state.method = (nwritten < MIN_PART_SIZE).then_some(S3WriteUploadMethod::PutObject);

            *op_slot = Some(S3ObjectOpSlot::Write(state));

            return Ok(written);
        };

        let Some(state) = op_slot_mut.as_write_mut() else {
            return Err(
                io::Error::other("write operation was blocked by another operation").into(),
            );
        };

        let size = buf.len();
        let nwritten = state.cursor.write(&buf).await?;

        state.total_written += nwritten;

        if nwritten < size {
            // Now that the tmp buffer is full and we still have bytes to write, we must switch
            // to a multi-part upload.
            let mmap_buf = state.try_swap_buffer().map_err(io::Error::other)?;

            mmap_buf.raw.flush_async()?;
            assert_eq!(state.buf.cursor.get_ref().position(), 0);

            let method = match &mut state.method {
                Some(S3WriteUploadMethod::MultiPartUpload(method)) => method,
                method @ (Some(S3WriteUploadMethod::PutObject) | None) => {
                    method.take();
                    method
                        .insert(S3WriteUploadMethod::MultiPartUpload(
                            S3MultiPartUploadMethod {
                                upload_id: self
                                    .fs
                                    .client
                                    .create_multipart_upload()
                                    .bucket(&self.bucket_name)
                                    .key(&self.key)
                                    .send()
                                    .await
                                    .map_err(io::Error::other)?
                                    .upload_id
                                    .ok_or(io::Error::other(concat!(
                                        "upload id could not be found in ",
                                        "the multipart upload response"
                                    )))?,

                                ..Default::default()
                            },
                        ))
                        .as_multi_part_upload_mut()
                        .unwrap()
                }
            };

            method.tasks.push(
                tokio::task::spawn({
                    let upload_id = method.upload_id.clone();
                    let client = self.fs.client.clone();
                    let bucket_name = self.bucket_name.clone();
                    let key = self.key.clone();
                    let part_idx = method.recent_part_idx;

                    method.recent_part_idx += 1;

                    async move {
                        client
                            .upload_part()
                            .bucket(bucket_name)
                            .key(key)
                            .upload_id(upload_id)
                            .part_number(part_idx)
                            .body(ByteStream::new(
                                unsafe {
                                    slice::from_raw_parts(mmap_buf.raw.as_ptr(), mmap_buf.raw.len())
                                }
                                .into(),
                            ))
                            .send()
                            .map(|it| (part_idx, it))
                            .await
                    }
                    .instrument(debug_span!("upload part", part = part_idx))
                })
                .boxed(),
            );

            return Ok(WriteOutcome::Partial {
                nwritten,
                view: buf,
            });
        }

        assert_eq!(nwritten, size);
        Ok(WriteOutcome::Full { nwritten })
    }

    fn write_all_sync(self: Rc<Self>, _buf: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    #[instrument(level = "info", skip_all, fields(self.bucket_name, self.key))]
    async fn write_all(self: Rc<Self>, mut buf: BufView) -> FsResult<()> {
        loop {
            match self.clone().write(buf).await? {
                WriteOutcome::Full { .. } => return Ok(()),
                WriteOutcome::Partial { nwritten, mut view } => {
                    view.advance_cursor(nwritten);
                    buf = view;
                }
            }
        }
    }

    fn read_all_sync(self: Rc<Self>) -> FsResult<Vec<u8>> {
        Err(FsError::NotSupported)
    }

    #[instrument(level = "info", skip_all, fields(self.bucket_name, self.key))]
    async fn read_all_async(self: Rc<Self>) -> FsResult<Vec<u8>> {
        let resp = self
            .fs
            .client
            .get_object()
            .bucket(&self.bucket_name)
            .key(&self.key)
            .send()
            .await;

        match resp {
            Ok(v) => Ok(v.body.collect().await.map_err(io::Error::other)?.to_vec()),
            Err(err) => Err(io::Error::other(err).into()),
        }
    }

    fn chmod_sync(self: Rc<Self>, _pathmode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chmod_async(self: Rc<Self>, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn seek_sync(self: Rc<Self>, _pos: io::SeekFrom) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    async fn seek_async(self: Rc<Self>, _pos: io::SeekFrom) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn datasync_sync(self: Rc<Self>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn datasync_async(self: Rc<Self>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn sync_sync(self: Rc<Self>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn sync_async(self: Rc<Self>) -> FsResult<()> {
        let mut op_slot = RcRef::map(&self, |r| &r.op_slot).borrow_mut().await;
        let Some(state) = &mut *op_slot else {
            return Ok(());
        };

        let result = match state {
            S3ObjectOpSlot::Write(S3ObjectWriteState {
                method: Some(method),
                ..
            }) => method.sync().await,

            _ => Ok(()),
        };

        if result.is_err() {
            op_slot.take();
        }

        result
    }

    fn stat_sync(self: Rc<Self>) -> FsResult<FsStat> {
        Err(FsError::NotSupported)
    }

    async fn stat_async(self: Rc<Self>) -> FsResult<FsStat> {
        let resp = self
            .fs
            .client
            .head_object()
            .bucket(&self.bucket_name)
            .key(&self.key)
            .send()
            .await
            .map_err(io::Error::other)?;

        Ok(FsStat {
            is_file: true,
            is_directory: false,
            is_symlink: false,
            size: resp
                .content_length()
                .map(u64::try_from)
                .transpose()
                .map_err(|err| {
                    io::Error::other(
                        anyhow!(err)
                            .context("object's content-length header was not read correctly"),
                    )
                })?
                .ok_or(io::Error::other(
                    "unable to get object's content-length header",
                ))?,

            mtime: resp.last_modified.and_then(to_msec),
            atime: None,
            birthtime: None,

            dev: 0,
            ino: 0,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: 0,
            blocks: 0,
            is_block_device: false,
            is_char_device: false,
            is_fifo: false,
            is_socket: false,
        })
    }

    fn lock_sync(self: Rc<Self>, _exclusive: bool) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn lock_async(self: Rc<Self>, _exclusive: bool) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn unlock_sync(self: Rc<Self>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn unlock_async(self: Rc<Self>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn truncate_sync(self: Rc<Self>, _len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn truncate_async(self: Rc<Self>, _len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn utime_sync(
        self: Rc<Self>,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn utime_async(
        self: Rc<Self>,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn as_stdio(self: Rc<Self>) -> FsResult<std::process::Stdio> {
        Err(FsError::NotSupported)
    }

    fn backing_fd(self: Rc<Self>) -> Option<ResourceHandleFd> {
        None
    }

    fn try_clone_inner(self: Rc<Self>) -> FsResult<Rc<dyn File>> {
        Err(FsError::NotSupported)
    }
}

fn try_get_bucket_name_and_key(path: PathBuf) -> FsResult<(String, String)> {
    let mut iter = path.iter();

    Ok((
        iter.next()
            .ok_or(io::Error::other("no bucket name specified"))?
            .to_string_lossy()
            .to_string(),
        iter.map(OsStr::to_string_lossy)
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

#[inline(always)]
fn to_msec(maybe_time: DateTime) -> Option<u64> {
    match SystemTime::try_from(maybe_time) {
        Ok(time) => Some(
            time.duration_since(UNIX_EPOCH)
                .map(|t| t.as_millis() as u64)
                .unwrap_or_else(|err| err.duration().as_millis() as u64),
        ),
        Err(_) => None,
    }
}

fn to_combined_message<I, E>(errors: I) -> FsResult<()>
where
    I: IntoIterator<Item = E>,
    E: ToString,
{
    let iter = errors.into_iter();
    let messages = iter.map(|err| err.to_string()).collect::<Vec<_>>();

    if messages.is_empty() {
        return Ok(());
    }

    Err(io::Error::other(messages.join("\n")).into())
}
