use std::{
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use anyhow::Context;
use deno_core::{BufMutView, BufView, ResourceHandleFd, WriteOutcome};
use deno_fs::{AccessCheckCb, FsDirEntry, FsFileType, RealFs};
use deno_io::fs::{File, FsError, FsResult, FsStat};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::sync::Mutex;
use tracing::{error, instrument};

use super::TryNormalizePath;

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TmpFsConfig {
    base: Option<PathBuf>,
    prefix: Option<String>,
    suffix: Option<String>,
    random_len: Option<usize>,
    quota: Option<usize>,
}

impl TryFrom<TmpFsConfig> for TmpFs {
    type Error = anyhow::Error;

    fn try_from(value: TmpFsConfig) -> Result<Self, Self::Error> {
        let mut builder = tempfile::Builder::new();

        if let Some(prefix) = value.prefix.as_ref() {
            builder.prefix(prefix);
        }
        if let Some(suffix) = value.suffix.as_ref() {
            builder.suffix(suffix);
        }
        if let Some(random_len) = value.random_len {
            builder.rand_bytes(random_len);
        }

        let root = Arc::new(
            value
                .base
                .map(|it| builder.tempdir_in(it))
                .unwrap_or_else(|| builder.tempdir())
                .context("could not create a temporary directory for tmpfs")?,
        );

        Ok(Self {
            root: root.clone(),
            quota: Arc::new(Quota::new(root, value.quota)),
        })
    }
}

#[derive(Debug)]
struct Quota {
    root: Arc<TempDir>,
    usage: Arc<AtomicUsize>,
    lock: Mutex<()>,
    limit: usize,
}

impl std::ops::Deref for Quota {
    type Target = AtomicUsize;

    fn deref(&self) -> &Self::Target {
        &self.usage
    }
}

impl Quota {
    fn new(root: Arc<TempDir>, limit: Option<usize>) -> Self {
        Self {
            root,
            usage: Arc::default(),
            lock: Mutex::default(),
            limit: limit.unwrap_or(usize::MAX),
        }
    }

    async fn check<A>(&self, buf: A) -> FsResult<()>
    where
        A: AsRef<[u8]>,
    {
        let _guard = self.lock.lock().await;
        self.check_inner(buf)
    }

    fn blocking_check<A>(&self, buf: A) -> FsResult<()>
    where
        A: AsRef<[u8]>,
    {
        let _guard = self.lock.blocking_lock();
        self.check_inner(buf)
    }

    fn check_inner<A>(&self, buf: A) -> FsResult<()>
    where
        A: AsRef<[u8]>,
    {
        if self.usage.load(Ordering::Acquire) + buf.as_ref().len() > self.limit {
            // NOTE: `ErrorKind::FilesystemQuotaExceeded` is not stable yet, so replace it with
            // `ErrorKind::Other`.
            //
            // [1]: https://github.com/rust-lang/rust/issues/86442
            return Err(FsError::Io(io::Error::other("filesystem quota exceeded")));
        }

        Ok(())
    }

    #[instrument(level = "info", skip(self))]
    async fn sync(&self) {
        let _guard = self.lock.lock().await;

        match tokio::task::spawn_blocking(self.make_sync_inner_fn()).await {
            Ok(Err(err)) => error!(?err),
            Err(err) => error!(?err),
            _ => {}
        }
    }

    #[instrument(level = "info", skip(self))]
    fn blocking_sync(&self) {
        let _guard = self.lock.blocking_lock();

        if let Err(err) = self.make_sync_inner_fn()() {
            error!(?err);
        }
    }

    fn make_sync_inner_fn(&self) -> impl FnOnce() -> FsResult<()> {
        let root_path = self.root.path().to_path_buf();
        let usage = self.usage.clone();

        fn get_dir_size(path: PathBuf) -> io::Result<u64> {
            use std::fs;

            let entires = fs::read_dir(path)?;
            let mut size = 0;

            for entry in entires {
                let entry = entry?;
                let metadata = entry.metadata()?;

                if metadata.is_dir() {
                    size += get_dir_size(entry.path())?;
                } else {
                    size += metadata.len();
                }
            }

            Ok(size)
        }

        move || -> FsResult<()> {
            usage.fetch_add(
                get_dir_size(root_path).map_err(FsError::Io)? as usize,
                Ordering::Release,
            );

            Ok(())
        }
    }

    fn fetch_delta(&self, val: i64, order: Ordering) -> usize {
        match val.cmp(&0) {
            std::cmp::Ordering::Greater => self.fetch_add(val as usize, order),
            std::cmp::Ordering::Less => self.fetch_sub(i64::abs(val) as usize, order),
            std::cmp::Ordering::Equal => self.load(Ordering::Acquire),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmpFs {
    root: Arc<TempDir>,
    quota: Arc<Quota>,
}

#[async_trait::async_trait(?Send)]
impl deno_fs::FileSystem for TmpFs {
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
        path: &Path,
        options: deno_fs::OpenOptions,
        access_check: Option<AccessCheckCb>,
    ) -> FsResult<Rc<dyn File>> {
        Ok(Rc::new(TmpObject {
            fs: self.clone(),
            file: RealFs.open_sync(
                &self.root.path().join(path.try_normalize()?),
                options,
                access_check,
            )?,
        }))
    }

    async fn open_async<'a>(
        &'a self,
        path: PathBuf,
        options: deno_fs::OpenOptions,
        access_check: Option<AccessCheckCb<'a>>,
    ) -> FsResult<Rc<dyn File>> {
        Ok(Rc::new(TmpObject {
            fs: self.clone(),
            file: RealFs
                .open_async(
                    self.root.path().join(path.try_normalize()?),
                    options,
                    access_check,
                )
                .await?,
        }))
    }

    fn mkdir_sync(&self, path: &Path, recursive: bool, mode: u32) -> FsResult<()> {
        RealFs.mkdir_sync(
            &self.root.path().join(path.try_normalize()?),
            recursive,
            mode,
        )
    }

    async fn mkdir_async(&self, path: PathBuf, recursive: bool, mode: u32) -> FsResult<()> {
        RealFs
            .mkdir_async(
                self.root.path().join(path.try_normalize()?),
                recursive,
                mode,
            )
            .await
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

    fn remove_sync(&self, path: &Path, recursive: bool) -> FsResult<()> {
        let result = RealFs.remove_sync(&self.root.path().join(path.try_normalize()?), recursive);

        self.quota.blocking_sync();
        result
    }

    async fn remove_async(&self, path: PathBuf, recursive: bool) -> FsResult<()> {
        let result = RealFs
            .remove_async(self.root.path().join(path.try_normalize()?), recursive)
            .await;

        self.quota.sync().await;
        result
    }

    fn copy_file_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        let result = RealFs.copy_file_sync(
            &self.root.path().join(oldpath.try_normalize()?),
            &self.root.path().join(newpath.try_normalize()?),
        );

        self.quota.blocking_sync();
        result
    }

    async fn copy_file_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        let result = RealFs
            .copy_file_async(
                self.root.path().join(oldpath.try_normalize()?),
                self.root.path().join(newpath.try_normalize()?),
            )
            .await;

        self.quota.sync().await;
        result
    }

    fn cp_sync(&self, path: &Path, new_path: &Path) -> FsResult<()> {
        let result = RealFs.cp_sync(
            &self.root.path().join(path.try_normalize()?),
            &self.root.path().join(new_path.try_normalize()?),
        );

        self.quota.blocking_sync();
        result
    }

    async fn cp_async(&self, path: PathBuf, new_path: PathBuf) -> FsResult<()> {
        let result = RealFs
            .cp_async(
                self.root.path().join(path),
                self.root.path().join(new_path.try_normalize()?),
            )
            .await;

        self.quota.sync().await;
        result
    }

    fn stat_sync(&self, path: &Path) -> FsResult<FsStat> {
        RealFs.stat_sync(&self.root.path().join(path.try_normalize()?))
    }

    async fn stat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        RealFs
            .stat_async(self.root.path().join(path.try_normalize()?))
            .await
    }

    fn lstat_sync(&self, path: &Path) -> FsResult<FsStat> {
        RealFs.lstat_sync(&self.root.path().join(path.try_normalize()?))
    }

    async fn lstat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        RealFs
            .lstat_async(self.root.path().join(path.try_normalize()?))
            .await
    }

    fn realpath_sync(&self, path: &Path) -> FsResult<PathBuf> {
        RealFs.realpath_sync(&self.root.path().join(path.try_normalize()?))
    }

    async fn realpath_async(&self, path: PathBuf) -> FsResult<PathBuf> {
        RealFs
            .realpath_async(self.root.path().join(path.try_normalize()?))
            .await
    }

    fn read_dir_sync(&self, path: &Path) -> FsResult<Vec<FsDirEntry>> {
        RealFs.read_dir_sync(&self.root.path().join(path.try_normalize()?))
    }

    async fn read_dir_async(&self, path: PathBuf) -> FsResult<Vec<FsDirEntry>> {
        RealFs
            .read_dir_async(self.root.path().join(path.try_normalize()?))
            .await
    }

    fn rename_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        RealFs.rename_sync(
            &self.root.path().join(oldpath.try_normalize()?),
            &self.root.path().join(newpath.try_normalize()?),
        )
    }

    async fn rename_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        RealFs
            .rename_async(
                self.root.path().join(oldpath.try_normalize()?),
                self.root.path().join(newpath),
            )
            .await
    }

    fn link_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        RealFs.link_sync(
            &self.root.path().join(oldpath.try_normalize()?),
            &self.root.path().join(newpath.try_normalize()?),
        )
    }

    async fn link_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        RealFs
            .link_async(
                self.root.path().join(oldpath.try_normalize()?),
                self.root.path().join(newpath.try_normalize()?),
            )
            .await
    }

    fn symlink_sync(
        &self,
        oldpath: &Path,
        newpath: &Path,
        file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        RealFs.symlink_sync(
            &self.root.path().join(oldpath.try_normalize()?),
            &self.root.path().join(newpath.try_normalize()?),
            file_type,
        )
    }

    async fn symlink_async(
        &self,
        oldpath: PathBuf,
        newpath: PathBuf,
        file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        RealFs
            .symlink_async(
                self.root.path().join(oldpath.try_normalize()?),
                self.root.path().join(newpath.try_normalize()?),
                file_type,
            )
            .await
    }

    fn read_link_sync(&self, path: &Path) -> FsResult<PathBuf> {
        RealFs.read_link_sync(&self.root.path().join(path.try_normalize()?))
    }

    async fn read_link_async(&self, path: PathBuf) -> FsResult<PathBuf> {
        RealFs
            .read_link_async(self.root.path().join(path.try_normalize()?))
            .await
    }

    fn truncate_sync(&self, path: &Path, len: u64) -> FsResult<()> {
        let path = self.root.path().join(path.try_normalize()?);
        let size = self.stat_sync(&path)?.size;

        RealFs.truncate_sync(&path, len).inspect(|_| {
            self.quota
                .fetch_delta((len as i64) - (size as i64), Ordering::Release);
        })
    }

    async fn truncate_async(&self, path: PathBuf, len: u64) -> FsResult<()> {
        let path = self.root.path().join(path.try_normalize()?);
        let size = self.stat_async(path.clone()).await?.size;

        RealFs.truncate_async(path, len).await.inspect(|_| {
            self.quota
                .fetch_delta((len as i64) - (size as i64), Ordering::Release);
        })
    }

    fn utime_sync(
        &self,
        path: &Path,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        RealFs.utime_sync(
            &self.root.path().join(path.try_normalize()?),
            atime_secs,
            atime_nanos,
            mtime_secs,
            mtime_nanos,
        )
    }

    async fn utime_async(
        &self,
        path: PathBuf,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        RealFs
            .utime_async(
                self.root.path().join(path.try_normalize()?),
                atime_secs,
                atime_nanos,
                mtime_secs,
                mtime_nanos,
            )
            .await
    }

    fn lutime_sync(
        &self,
        path: &Path,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        RealFs.lutime_sync(
            &self.root.path().join(path.try_normalize()?),
            atime_secs,
            atime_nanos,
            mtime_secs,
            mtime_nanos,
        )
    }

    async fn lutime_async(
        &self,
        path: PathBuf,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        RealFs
            .lutime_async(
                self.root.path().join(path.try_normalize()?),
                atime_secs,
                atime_nanos,
                mtime_secs,
                mtime_nanos,
            )
            .await
    }
}

pub struct TmpObject {
    fs: TmpFs,
    file: Rc<dyn File>,
}

#[async_trait::async_trait(?Send)]
impl deno_io::fs::File for TmpObject {
    fn read_sync(self: Rc<Self>, buf: &mut [u8]) -> FsResult<usize> {
        self.file.clone().read_sync(buf)
    }

    async fn read_byob(self: Rc<Self>, buf: BufMutView) -> FsResult<(usize, BufMutView)> {
        self.file.clone().read_byob(buf).await
    }

    fn write_sync(self: Rc<Self>, buf: &[u8]) -> FsResult<usize> {
        self.fs.quota.blocking_check(buf)?;
        self.file.clone().write_sync(buf).inspect(|it| {
            self.fs.quota.fetch_add(*it, Ordering::Release);
        })
    }

    async fn write(self: Rc<Self>, buf: BufView) -> FsResult<WriteOutcome> {
        self.fs.quota.check(&*buf).await?;
        self.file.clone().write(buf).await.inspect(|it| match it {
            WriteOutcome::Partial { nwritten, .. } | WriteOutcome::Full { nwritten } => {
                self.fs.quota.fetch_add(*nwritten, Ordering::Release);
            }
        })
    }

    fn write_all_sync(self: Rc<Self>, buf: &[u8]) -> FsResult<()> {
        self.fs.quota.blocking_check(buf)?;
        self.file.clone().write_all_sync(buf).inspect(|_| {
            self.fs.quota.fetch_add(buf.len(), Ordering::Release);
        })
    }

    async fn write_all(self: Rc<Self>, buf: BufView) -> FsResult<()> {
        let len = buf.len();

        self.fs.quota.check(&*buf).await?;
        self.file.clone().write_all(buf).await.inspect(|_| {
            self.fs.quota.fetch_add(len, Ordering::Release);
        })
    }

    fn read_all_sync(self: Rc<Self>) -> FsResult<Vec<u8>> {
        self.file.clone().read_all_sync()
    }

    async fn read_all_async(self: Rc<Self>) -> FsResult<Vec<u8>> {
        self.file.clone().read_all_async().await
    }

    fn chmod_sync(self: Rc<Self>, _pathmode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chmod_async(self: Rc<Self>, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn seek_sync(self: Rc<Self>, pos: SeekFrom) -> FsResult<u64> {
        self.file.clone().seek_sync(pos)
    }

    async fn seek_async(self: Rc<Self>, pos: SeekFrom) -> FsResult<u64> {
        self.file.clone().seek_async(pos).await
    }

    fn datasync_sync(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().datasync_sync()
    }

    async fn datasync_async(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().datasync_async().await
    }

    fn sync_sync(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().sync_sync()
    }

    async fn sync_async(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().sync_async().await
    }

    fn stat_sync(self: Rc<Self>) -> FsResult<FsStat> {
        self.file.clone().stat_sync()
    }

    async fn stat_async(self: Rc<Self>) -> FsResult<FsStat> {
        self.file.clone().stat_async().await
    }

    fn lock_sync(self: Rc<Self>, exclusive: bool) -> FsResult<()> {
        self.file.clone().lock_sync(exclusive)
    }

    async fn lock_async(self: Rc<Self>, exclusive: bool) -> FsResult<()> {
        self.file.clone().lock_async(exclusive).await
    }

    fn unlock_sync(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().unlock_sync()
    }

    async fn unlock_async(self: Rc<Self>) -> FsResult<()> {
        self.file.clone().unlock_async().await
    }

    fn truncate_sync(self: Rc<Self>, len: u64) -> FsResult<()> {
        let size = self.file.clone().stat_sync()?.size;

        self.file.clone().truncate_sync(len).inspect(|_| {
            self.fs
                .quota
                .fetch_delta((len as i64) - (size as i64), Ordering::Release);
        })
    }

    async fn truncate_async(self: Rc<Self>, len: u64) -> FsResult<()> {
        let size = self.file.clone().stat_sync()?.size;

        self.file.clone().truncate_async(len).await.inspect(|_| {
            self.fs
                .quota
                .fetch_delta((len as i64) - (size as i64), Ordering::Release);
        })
    }

    fn utime_sync(
        self: Rc<Self>,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        self.file
            .clone()
            .utime_sync(atime_secs, atime_nanos, mtime_secs, mtime_nanos)
    }

    async fn utime_async(
        self: Rc<Self>,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        self.file
            .clone()
            .utime_async(atime_secs, atime_nanos, mtime_secs, mtime_nanos)
            .await
    }

    fn as_stdio(self: Rc<Self>) -> FsResult<Stdio> {
        Err(FsError::NotSupported)
    }

    fn backing_fd(self: Rc<Self>) -> Option<ResourceHandleFd> {
        self.file.clone().backing_fd()
    }

    fn try_clone_inner(self: Rc<Self>) -> FsResult<Rc<dyn File>> {
        self.file.clone().try_clone_inner()
    }
}

#[cfg(test)]
mod test {
    use std::{io, path::PathBuf};

    use deno_fs::{FileSystem, OpenOptions};
    use once_cell::sync::Lazy;
    use tokio::fs::read;

    use super::{TmpFs, TmpFsConfig};

    static OPEN_CREATE: Lazy<OpenOptions> =
        Lazy::new(|| OpenOptions::write(false, false, true, None));

    static DATA: &[u8] = b"meowmeow";

    fn get_tmp_fs() -> TmpFs {
        TmpFsConfig {
            prefix: Some("meowmeow".to_string()),
            ..Default::default()
        }
        .try_into()
        .unwrap()
    }

    #[tokio::test]
    async fn normalized_path() {
        let fs = get_tmp_fs();

        fs.write_file_async(PathBuf::from("meowmeow"), *OPEN_CREATE, None, DATA.to_vec())
            .await
            .unwrap();

        assert_eq!(read(fs.root.path().join("meowmeow")).await.unwrap(), DATA);
    }

    #[tokio::test]
    async fn non_normalized_path() {
        let fs = get_tmp_fs();

        fs.write_file_async(
            PathBuf::from("meowmeow/a/b/../.."),
            *OPEN_CREATE,
            None,
            DATA.to_vec(),
        )
        .await
        .unwrap();

        assert_eq!(read(fs.root.path().join("meowmeow")).await.unwrap(), DATA)
    }

    #[tokio::test]
    async fn non_normalized_path_2() {
        let fs = get_tmp_fs();

        assert_eq!(
            fs.write_file_async(
                PathBuf::from("../meowmeow"),
                *OPEN_CREATE,
                None,
                DATA.to_vec(),
            )
            .await
            .err()
            .unwrap()
            .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[tokio::test]
    async fn non_normalized_path_3() {
        let fs = get_tmp_fs();

        fs.mkdir_async(PathBuf::from("meowmeow/a/b/../c"), true, 0o777)
            .await
            .unwrap();

        assert!(fs.stat_async(PathBuf::from("meowmeow/a/c")).await.is_ok());
    }

    #[tokio::test]
    async fn abnormal_mkdir_mode() {
        let fs = get_tmp_fs();

        assert_eq!(
            fs.mkdir_async(PathBuf::from("meowmeow/a/b"), true, 0o100)
                .await
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
