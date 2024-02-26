use crate::{EszipStaticFiles, FileBackedVfs};
use deno_fs::{FsDirEntry, FsFileType, OpenOptions, RealFs};
use deno_io::fs::{File, FsError, FsResult, FsStat};
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use deno_semver::Version;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct StaticFs {
    files: EszipStaticFiles,
    vfs_path: PathBuf,
    snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    vfs: Arc<FileBackedVfs>,
}

impl StaticFs {
    pub fn new(
        static_files: EszipStaticFiles,
        vfs_path: PathBuf,
        vfs: Arc<FileBackedVfs>,
        snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
    ) -> Self {
        Self {
            vfs,
            files: static_files,
            vfs_path,
            snapshot,
        }
    }

    pub fn is_valid_npm_package(&self, path: &Path) -> bool {
        if let Some(npm_snapshot) = &self.snapshot {
            let serialized_snapshot = npm_snapshot.as_serialized();
            let vfs_path = self.vfs_path.clone();
            if path.starts_with(vfs_path) {
                let gen_path_str = path.to_str().unwrap();
                let main_path = self.vfs_path.to_str().unwrap();

                let package_info = gen_path_str.replace(main_path, "");
                let package_info = {
                    if let Some(res) = package_info.strip_prefix("/") {
                        res.to_string()
                    } else {
                        package_info
                    }
                };

                let package_info: Vec<&str> = package_info.split("/").collect();

                let (name, ver) = {
                    let name = package_info.get(0).unwrap().to_string();
                    let ver = package_info.get(1).unwrap().to_string();
                    (name, ver)
                };

                let does_package_exist = serialized_snapshot.packages.iter().any(|data| {
                    data.id.nv.name == name
                        && data.id.nv.version == Version::parse_standard(ver.as_str()).unwrap()
                });

                does_package_exist
            } else {
                false
            }
        } else {
            false
        }
    }
}

#[async_trait::async_trait(?Send)]
impl deno_fs::FileSystem for StaticFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Ok(PathBuf::new())
    }

    fn tmp_dir(&self) -> FsResult<PathBuf> {
        Err(FsError::NotSupported)
    }

    fn chdir(&self, path: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn umask(&self, mask: Option<u32>) -> FsResult<u32> {
        Err(FsError::NotSupported)
    }

    fn open_sync(&self, path: &Path, options: OpenOptions) -> FsResult<Rc<dyn File>> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.open_file(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn open_async(&self, path: PathBuf, options: OpenOptions) -> FsResult<Rc<dyn File>> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.open_file(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn mkdir_sync(&self, path: &Path, recursive: bool, mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn mkdir_async(&self, path: PathBuf, recursive: bool, mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn chmod_sync(&self, path: &Path, mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chmod_async(&self, path: PathBuf, mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn chown_sync(&self, path: &Path, uid: Option<u32>, gid: Option<u32>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn chown_async(&self, path: PathBuf, uid: Option<u32>, gid: Option<u32>) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn remove_sync(&self, path: &Path, recursive: bool) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn remove_async(&self, path: PathBuf, recursive: bool) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn copy_file_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn copy_file_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn cp_sync(&self, path: &Path, new_path: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn cp_async(&self, path: PathBuf, new_path: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn stat_sync(&self, path: &Path) -> FsResult<FsStat> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.stat(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn stat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.stat(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn lstat_sync(&self, path: &Path) -> FsResult<FsStat> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.lstat(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn lstat_async(&self, path: PathBuf) -> FsResult<FsStat> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.lstat(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn realpath_sync(&self, path: &Path) -> FsResult<PathBuf> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.canonicalize(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn realpath_async(&self, path: PathBuf) -> FsResult<PathBuf> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.canonicalize(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn read_dir_sync(&self, path: &Path) -> FsResult<Vec<FsDirEntry>> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.read_dir(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn read_dir_async(&self, path: PathBuf) -> FsResult<Vec<FsDirEntry>> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.read_dir(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn rename_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn rename_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn link_sync(&self, oldpath: &Path, newpath: &Path) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn link_async(&self, oldpath: PathBuf, newpath: PathBuf) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn symlink_sync(
        &self,
        oldpath: &Path,
        newpath: &Path,
        file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn symlink_async(
        &self,
        oldpath: PathBuf,
        newpath: PathBuf,
        file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn read_link_sync(&self, path: &Path) -> FsResult<PathBuf> {
        if self.vfs.is_path_within(path) {
            Ok(self.vfs.read_link(path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    async fn read_link_async(&self, path: PathBuf) -> FsResult<PathBuf> {
        if self.vfs.is_path_within(&path) {
            Ok(self.vfs.read_link(&path)?)
        } else {
            Err(FsError::NotSupported)
        }
    }

    fn truncate_sync(&self, path: &Path, len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn truncate_async(&self, path: PathBuf, len: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn utime_sync(
        &self,
        path: &Path,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    async fn utime_async(
        &self,
        path: PathBuf,
        atime_secs: i64,
        atime_nanos: u32,
        mtime_secs: i64,
        mtime_nanos: u32,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
}
