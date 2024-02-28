// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::anyhow::Context;
use deno_core::error::AnyError;
use deno_core::parking_lot::Mutex;
use deno_core::BufMutView;
use deno_core::BufView;
use deno_core::ResourceHandleFd;
use deno_fs::FsDirEntry;
use deno_io;
use deno_io::fs::FsError;
use deno_io::fs::FsResult;
use deno_io::fs::FsStat;
use sb_core::util::checksum;
use sb_core::util::fs::canonicalize_path;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
#[error(
"Failed to strip prefix '{}' from '{}'", root_path.display(), target.display()
)]
pub struct StripRootError {
    root_path: PathBuf,
    target: PathBuf,
}

pub struct VfsBuilder {
    root_path: PathBuf,
    root_dir: VirtualDirectory,
    files: Vec<Vec<u8>>,
    current_offset: u64,
    file_offsets: HashMap<String, u64>,
}

impl VfsBuilder {
    pub fn new(root_path: PathBuf) -> Result<Self, AnyError> {
        let root_path = canonicalize_path(&root_path)?;
        log::debug!("Building vfs with root '{}'", root_path.display());
        Ok(Self {
            root_dir: VirtualDirectory {
                name: root_path
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                entries: Vec::new(),
            },
            root_path,
            files: Vec::new(),
            current_offset: 0,
            file_offsets: Default::default(),
        })
    }

    pub fn set_root_dir_name(&mut self, name: String) {
        self.root_dir.name = name;
    }

    pub fn add_dir_recursive(&mut self, path: &Path) -> Result<(), AnyError> {
        let path = canonicalize_path(path)?;
        self.add_dir_recursive_internal(&path)
    }

    fn add_dir_recursive_internal(&mut self, path: &Path) -> Result<(), AnyError> {
        self.add_dir(path)?;
        let read_dir =
            std::fs::read_dir(path).with_context(|| format!("Reading {}", path.display()))?;

        for entry in read_dir {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();

            if file_type.is_dir() {
                self.add_dir_recursive_internal(&path)?;
            } else if file_type.is_file() {
                let file_bytes =
                    std::fs::read(&path).with_context(|| format!("Reading {}", path.display()))?;
                self.add_file(&path, file_bytes)?;
            } else if file_type.is_symlink() {
                let target = canonicalize_path(&path)
                    .with_context(|| format!("Reading symlink {}", path.display()))?;
                if let Err(StripRootError { .. }) = self.add_symlink(&path, &target) {
                    if target.is_file() {
                        // this may change behavior, so warn the user about it
                        log::warn!(
              "Symlink target is outside '{}'. Inlining symlink at '{}' to '{}' as file.",
              self.root_path.display(),
              path.display(),
              target.display(),
            );
                        // inline the symlink and make the target file
                        let file_bytes = std::fs::read(&target)
                            .with_context(|| format!("Reading {}", path.display()))?;
                        self.add_file(&path, file_bytes)?;
                    } else {
                        log::warn!(
              "Symlink target is outside '{}'. Excluding symlink at '{}' with target '{}'.",
              self.root_path.display(),
              path.display(),
              target.display(),
            );
                    }
                }
            }
        }

        Ok(())
    }

    fn add_dir(&mut self, path: &Path) -> Result<&mut VirtualDirectory, StripRootError> {
        log::debug!("Ensuring directory '{}'", path.display());
        let path = self.path_relative_root(path)?;
        let mut current_dir = &mut self.root_dir;

        for component in path.components() {
            let name = component.as_os_str().to_string_lossy();
            let index = match current_dir
                .entries
                .binary_search_by(|e| e.name().cmp(&name))
            {
                Ok(index) => index,
                Err(insert_index) => {
                    current_dir.entries.insert(
                        insert_index,
                        VfsEntry::Dir(VirtualDirectory {
                            name: name.to_string(),
                            entries: Vec::new(),
                        }),
                    );
                    insert_index
                }
            };
            match &mut current_dir.entries[index] {
                VfsEntry::Dir(dir) => {
                    current_dir = dir;
                }
                _ => unreachable!(),
            };
        }

        Ok(current_dir)
    }

    fn add_file(&mut self, path: &Path, data: Vec<u8>) -> Result<(), AnyError> {
        log::debug!("Adding file '{}'", path.display());
        let checksum = checksum::gen(&[&data]);
        let offset = if let Some(offset) = self.file_offsets.get(&checksum) {
            // duplicate file, reuse an old offset
            *offset
        } else {
            self.file_offsets.insert(checksum, self.current_offset);
            self.current_offset
        };

        let dir = self.add_dir(path.parent().unwrap())?;
        let name = path.file_name().unwrap().to_string_lossy();
        let data_len = data.len();
        match dir.entries.binary_search_by(|e| e.name().cmp(&name)) {
            Ok(_) => unreachable!(),
            Err(insert_index) => {
                dir.entries.insert(
                    insert_index,
                    VfsEntry::File(VirtualFile {
                        name: name.to_string(),
                        offset,
                        len: data.len() as u64,
                        content: Some(data),
                    }),
                );
            }
        }

        // new file, update the list of files
        if self.current_offset == offset {
            // self.files.push(data);
            self.current_offset += data_len as u64;
        }

        Ok(())
    }

    fn add_symlink(&mut self, path: &Path, target: &Path) -> Result<(), StripRootError> {
        log::debug!(
            "Adding symlink '{}' to '{}'",
            path.display(),
            target.display()
        );
        let dest = self.path_relative_root(target)?;
        let dir = self.add_dir(path.parent().unwrap())?;
        let name = path.file_name().unwrap().to_string_lossy();
        match dir.entries.binary_search_by(|e| e.name().cmp(&name)) {
            Ok(_) => unreachable!(),
            Err(insert_index) => {
                dir.entries.insert(
                    insert_index,
                    VfsEntry::Symlink(VirtualSymlink {
                        name: name.to_string(),
                        dest_parts: dest
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy().to_string())
                            .collect::<Vec<_>>(),
                    }),
                );
            }
        }
        Ok(())
    }

    pub fn into_dir_and_files(self) -> (VirtualDirectory, Vec<Vec<u8>>) {
        (self.root_dir, self.files)
    }

    fn path_relative_root(&self, path: &Path) -> Result<PathBuf, StripRootError> {
        match path.strip_prefix(&self.root_path) {
            Ok(p) => Ok(p.to_path_buf()),
            Err(_) => Err(StripRootError {
                root_path: self.root_path.clone(),
                target: path.to_path_buf(),
            }),
        }
    }
}

#[derive(Debug)]
enum VfsEntryRef<'a> {
    Dir(&'a VirtualDirectory),
    File(&'a VirtualFile),
    Symlink(&'a VirtualSymlink),
}

impl<'a> VfsEntryRef<'a> {
    pub fn as_fs_stat(&self) -> FsStat {
        match self {
            VfsEntryRef::Dir(_) => FsStat {
                is_directory: true,
                is_file: false,
                is_symlink: false,
                atime: None,
                birthtime: None,
                mtime: None,
                blksize: 0,
                size: 0,
                dev: 0,
                ino: 0,
                mode: 0,
                nlink: 0,
                uid: 0,
                gid: 0,
                rdev: 0,
                blocks: 0,
                is_block_device: false,
                is_char_device: false,
                is_fifo: false,
                is_socket: false,
            },
            VfsEntryRef::File(file) => FsStat {
                is_directory: false,
                is_file: true,
                is_symlink: false,
                atime: None,
                birthtime: None,
                mtime: None,
                blksize: 0,
                size: file.len,
                dev: 0,
                ino: 0,
                mode: 0,
                nlink: 0,
                uid: 0,
                gid: 0,
                rdev: 0,
                blocks: 0,
                is_block_device: false,
                is_char_device: false,
                is_fifo: false,
                is_socket: false,
            },
            VfsEntryRef::Symlink(_) => FsStat {
                is_directory: false,
                is_file: false,
                is_symlink: true,
                atime: None,
                birthtime: None,
                mtime: None,
                blksize: 0,
                size: 0,
                dev: 0,
                ino: 0,
                mode: 0,
                nlink: 0,
                uid: 0,
                gid: 0,
                rdev: 0,
                blocks: 0,
                is_block_device: false,
                is_char_device: false,
                is_fifo: false,
                is_socket: false,
            },
        }
    }
}

// todo(dsherret): we should store this more efficiently in the binary
#[derive(Debug, Serialize, Deserialize)]
pub enum VfsEntry {
    Dir(VirtualDirectory),
    File(VirtualFile),
    Symlink(VirtualSymlink),
}

impl VfsEntry {
    pub fn name(&self) -> &str {
        match self {
            VfsEntry::Dir(dir) => &dir.name,
            VfsEntry::File(file) => &file.name,
            VfsEntry::Symlink(symlink) => &symlink.name,
        }
    }

    fn as_ref(&self) -> VfsEntryRef {
        match self {
            VfsEntry::Dir(dir) => VfsEntryRef::Dir(dir),
            VfsEntry::File(file) => VfsEntryRef::File(file),
            VfsEntry::Symlink(symlink) => VfsEntryRef::Symlink(symlink),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VirtualDirectory {
    pub name: String,
    // should be sorted by name
    pub entries: Vec<VfsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualFile {
    pub name: String,
    pub offset: u64,
    pub len: u64,
    pub content: Option<Vec<u8>>, // Not Deno Original, but it's the best way to store it in the ESZIP.
}

impl VirtualFile {
    pub fn read_file(&self, _pos: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        match &self.content {
            Some(content) => {
                let read_length = buf.len().min(content.len());
                buf[..read_length].copy_from_slice(&content[..read_length]);
                Ok(read_length)
            }
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No content available",
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VirtualSymlink {
    pub name: String,
    pub dest_parts: Vec<String>,
}

impl VirtualSymlink {
    pub fn resolve_dest_from_root(&self, root: &Path) -> PathBuf {
        let mut dest = root.to_path_buf();
        for part in &self.dest_parts {
            dest.push(part);
        }
        dest
    }
}

#[derive(Debug)]
pub struct VfsRoot {
    pub dir: VirtualDirectory,
    pub root_path: PathBuf,
}

impl VfsRoot {
    fn find_entry<'a>(&'a self, path: &Path) -> std::io::Result<(PathBuf, VfsEntryRef<'a>)> {
        self.find_entry_inner(path, &mut HashSet::new())
    }

    fn find_entry_inner<'a>(
        &'a self,
        path: &Path,
        seen: &mut HashSet<PathBuf>,
    ) -> std::io::Result<(PathBuf, VfsEntryRef<'a>)> {
        let mut path = Cow::Borrowed(path);
        loop {
            let (resolved_path, entry) = self.find_entry_no_follow_inner(&path, seen)?;
            match entry {
                VfsEntryRef::Symlink(symlink) => {
                    if !seen.insert(path.to_path_buf()) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "circular symlinks",
                        ));
                    }
                    path = Cow::Owned(symlink.resolve_dest_from_root(&self.root_path));
                }
                _ => {
                    return Ok((resolved_path, entry));
                }
            }
        }
    }

    fn find_entry_no_follow(&self, path: &Path) -> std::io::Result<(PathBuf, VfsEntryRef)> {
        self.find_entry_no_follow_inner(path, &mut HashSet::new())
    }

    fn find_entry_no_follow_inner<'a>(
        &'a self,
        path: &Path,
        seen: &mut HashSet<PathBuf>,
    ) -> std::io::Result<(PathBuf, VfsEntryRef<'a>)> {
        let relative_path = match path.strip_prefix(&self.root_path) {
            Ok(p) => p,
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "path not found",
                ));
            }
        };
        let mut final_path = self.root_path.clone();
        let mut current_entry = VfsEntryRef::Dir(&self.dir);
        for component in relative_path.components() {
            let component = component.as_os_str().to_string_lossy();
            let current_dir = match current_entry {
                VfsEntryRef::Dir(dir) => {
                    final_path.push(component.as_ref());
                    dir
                }
                VfsEntryRef::Symlink(symlink) => {
                    let dest = symlink.resolve_dest_from_root(&self.root_path);
                    let (resolved_path, entry) = self.find_entry_inner(&dest, seen)?;
                    final_path = resolved_path; // overwrite with the new resolved path
                    match entry {
                        VfsEntryRef::Dir(dir) => {
                            final_path.push(component.as_ref());
                            dir
                        }
                        _ => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "path not found",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "path not found",
                    ));
                }
            };
            match current_dir
                .entries
                .binary_search_by(|e| e.name().cmp(&component))
            {
                Ok(index) => {
                    current_entry = current_dir.entries[index].as_ref();
                }
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "path not found",
                    ));
                }
            }
        }

        Ok((final_path, current_entry))
    }
}

#[derive(Clone)]
struct FileBackedVfsFile {
    file: VirtualFile,
    pos: Arc<Mutex<u64>>,
    vfs: Arc<FileBackedVfs>,
}

impl FileBackedVfsFile {
    fn seek(&self, pos: SeekFrom) -> FsResult<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                *self.pos.lock() = pos;
                Ok(pos)
            }
            SeekFrom::End(offset) => {
                if offset < 0 && -offset as u64 > self.file.len {
                    let msg = "An attempt was made to move the file pointer before the beginning of the file.";
                    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, msg).into())
                } else {
                    let mut current_pos = self.pos.lock();
                    *current_pos = if offset >= 0 {
                        self.file.len - (offset as u64)
                    } else {
                        self.file.len + (-offset as u64)
                    };
                    Ok(*current_pos)
                }
            }
            SeekFrom::Current(offset) => {
                let mut current_pos = self.pos.lock();
                if offset >= 0 {
                    *current_pos += offset as u64;
                } else if -offset as u64 > *current_pos {
                    return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "An attempt was made to move the file pointer before the beginning of the file.").into());
                } else {
                    *current_pos -= -offset as u64;
                }
                Ok(*current_pos)
            }
        }
    }

    fn read_to_buf(&self, buf: &mut [u8]) -> FsResult<usize> {
        let pos = {
            let mut pos = self.pos.lock();
            let read_pos = *pos;
            // advance the position due to the read
            *pos = std::cmp::min(self.file.len, *pos + buf.len() as u64);
            read_pos
        };
        self.vfs
            .read_file(&self.file, pos, buf)
            .map_err(|err| err.into())
    }

    fn read_to_end(&self) -> FsResult<Vec<u8>> {
        let pos = {
            let mut pos = self.pos.lock();
            let read_pos = *pos;
            // todo(dsherret): should this always set it to the end of the file?
            if *pos < self.file.len {
                // advance the position due to the read
                *pos = self.file.len;
            }
            read_pos
        };
        if pos > self.file.len {
            return Ok(Vec::new());
        }
        let size = (self.file.len - pos) as usize;
        let mut buf = vec![0; size];
        self.vfs.read_file(&self.file, pos, &mut buf)?;
        Ok(buf)
    }
}

#[async_trait::async_trait(?Send)]
impl deno_io::fs::File for FileBackedVfsFile {
    fn read_sync(self: Rc<Self>, buf: &mut [u8]) -> FsResult<usize> {
        self.read_to_buf(buf)
    }
    async fn read_byob(self: Rc<Self>, mut buf: BufMutView) -> FsResult<(usize, BufMutView)> {
        let inner = (*self).clone();
        tokio::task::spawn(async move {
            let nread = inner.read_to_buf(&mut buf)?;
            Ok((nread, buf))
        })
        .await?
    }

    fn write_sync(self: Rc<Self>, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }
    async fn write(self: Rc<Self>, _buf: BufView) -> FsResult<deno_core::WriteOutcome> {
        Err(FsError::NotSupported)
    }

    fn write_all_sync(self: Rc<Self>, _buf: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    async fn write_all(self: Rc<Self>, _buf: BufView) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn read_all_sync(self: Rc<Self>) -> FsResult<Vec<u8>> {
        self.read_to_end()
    }
    async fn read_all_async(self: Rc<Self>) -> FsResult<Vec<u8>> {
        let inner = (*self).clone();
        tokio::task::spawn_blocking(move || inner.read_to_end()).await?
    }

    fn chmod_sync(self: Rc<Self>, _pathmode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    async fn chmod_async(self: Rc<Self>, _mode: u32) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn seek_sync(self: Rc<Self>, pos: SeekFrom) -> FsResult<u64> {
        self.seek(pos)
    }
    async fn seek_async(self: Rc<Self>, pos: SeekFrom) -> FsResult<u64> {
        self.seek(pos)
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
        Err(FsError::NotSupported)
    }

    fn stat_sync(self: Rc<Self>) -> FsResult<FsStat> {
        Err(FsError::NotSupported)
    }
    async fn stat_async(self: Rc<Self>) -> FsResult<FsStat> {
        Err(FsError::NotSupported)
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

    // lower level functionality
    fn as_stdio(self: Rc<Self>) -> FsResult<std::process::Stdio> {
        Err(FsError::NotSupported)
    }
    fn backing_fd(self: Rc<Self>) -> Option<ResourceHandleFd> {
        None
    }
    fn try_clone_inner(self: Rc<Self>) -> FsResult<Rc<dyn deno_io::fs::File>> {
        Ok(self)
    }
}

#[derive(Debug)]
pub struct FileBackedVfs {
    fs_root: VfsRoot,
}

impl FileBackedVfs {
    pub fn new(fs_root: VfsRoot) -> Self {
        Self { fs_root }
    }

    pub fn root(&self) -> &Path {
        &self.fs_root.root_path
    }

    pub fn is_path_within(&self, path: &Path) -> bool {
        path.starts_with(&self.fs_root.root_path)
    }

    pub fn open_file(self: &Arc<Self>, path: &Path) -> std::io::Result<Rc<dyn deno_io::fs::File>> {
        let file = self.file_entry(path)?;
        Ok(Rc::new(FileBackedVfsFile {
            file: file.clone(),
            vfs: self.clone(),
            pos: Default::default(),
        }))
    }

    pub fn read_dir(&self, path: &Path) -> std::io::Result<Vec<FsDirEntry>> {
        let dir = self.dir_entry(path)?;
        Ok(dir
            .entries
            .iter()
            .map(|entry| FsDirEntry {
                name: entry.name().to_string(),
                is_file: matches!(entry, VfsEntry::File(_)),
                is_directory: matches!(entry, VfsEntry::Dir(_)),
                is_symlink: matches!(entry, VfsEntry::Symlink(_)),
            })
            .collect())
    }

    pub fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        let (_, entry) = self.fs_root.find_entry_no_follow(path)?;
        match entry {
            VfsEntryRef::Symlink(symlink) => {
                Ok(symlink.resolve_dest_from_root(&self.fs_root.root_path))
            }
            VfsEntryRef::Dir(_) | VfsEntryRef::File(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "not a symlink",
            )),
        }
    }

    pub fn lstat(&self, path: &Path) -> std::io::Result<FsStat> {
        let (_, entry) = self.fs_root.find_entry_no_follow(path)?;
        Ok(entry.as_fs_stat())
    }

    pub fn stat(&self, path: &Path) -> std::io::Result<FsStat> {
        let (_, entry) = self.fs_root.find_entry(path)?;
        Ok(entry.as_fs_stat())
    }

    pub fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let (path, _) = self.fs_root.find_entry(path)?;
        Ok(path)
    }

    pub fn read_file_all(&self, file: &VirtualFile) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0; file.len as usize];
        self.read_file(file, 0, &mut buf)?;
        Ok(buf)
    }

    pub fn read_file(
        &self,
        file: &VirtualFile,
        pos: u64,
        buf: &mut [u8],
    ) -> std::io::Result<usize> {
        file.read_file(pos, buf)
    }

    pub fn dir_entry(&self, path: &Path) -> std::io::Result<&VirtualDirectory> {
        let (_, entry) = self.fs_root.find_entry(path)?;
        match entry {
            VfsEntryRef::Dir(dir) => Ok(dir),
            VfsEntryRef::Symlink(_) => unreachable!(),
            VfsEntryRef::File(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "path is a file",
            )),
        }
    }

    pub fn file_entry(&self, path: &Path) -> std::io::Result<&VirtualFile> {
        let (_, entry) = self.fs_root.find_entry(path)?;
        match entry {
            VfsEntryRef::Dir(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "path is a directory",
            )),
            VfsEntryRef::Symlink(_) => unreachable!(),
            VfsEntryRef::File(file) => Ok(file),
        }
    }
}
