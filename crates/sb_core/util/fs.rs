use deno_ast::ModuleSpecifier;
use deno_core::anyhow::Context;
use deno_core::error::{uri_error, AnyError};
pub use deno_core::normalize_path;
use deno_core::unsync::spawn_blocking;
use deno_crypto::rand;
use deno_fs::FileSystem;
use log::debug;
use sb_node::PathClean;
use std::env::current_dir;
use std::fs::OpenOptions;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Writes the file to the file system at a temporary path, then
/// renames it to the destination in a single sys call in order
/// to never leave the file system in a corrupted state.
///
/// This also handles creating the directory if a NotFound error
/// occurs.
pub fn atomic_write_file_with_retries<T: AsRef<[u8]>>(
    file_path: &Path,
    data: T,
    mode: u32,
) -> std::io::Result<()> {
    let mut count = 0;
    loop {
        match atomic_write_file(file_path, data.as_ref(), mode) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if count >= 5 {
                    // too many retries, return the error
                    return Err(err);
                }
                count += 1;
                let sleep_ms = std::cmp::min(50, 10 * count);
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        }
    }
}

/// Writes the file to the file system at a temporary path, then
/// renames it to the destination in a single sys call in order
/// to never leave the file system in a corrupted state.
///
/// This also handles creating the directory if a NotFound error
/// occurs.
pub fn atomic_write_file<T: AsRef<[u8]>>(
    file_path: &Path,
    data: T,
    mode: u32,
) -> std::io::Result<()> {
    fn atomic_write_file_raw(
        temp_file_path: &Path,
        file_path: &Path,
        data: &[u8],
        mode: u32,
    ) -> std::io::Result<()> {
        write_file(temp_file_path, data, mode)?;
        std::fs::rename(temp_file_path, file_path)?;
        Ok(())
    }

    fn add_file_context(file_path: &Path, err: Error) -> Error {
        Error::new(
            err.kind(),
            format!("{:#} (for '{}')", err, file_path.display()),
        )
    }

    fn inner(file_path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
        let temp_file_path = {
            #[allow(clippy::format_collect)]
            let rand: String = (0..4)
                .map(|_| format!("{:02x}", rand::random::<u8>()))
                .collect();
            let extension = format!("{rand}.tmp");
            file_path.with_extension(extension)
        };

        if let Err(write_err) = atomic_write_file_raw(&temp_file_path, file_path, data, mode) {
            if write_err.kind() == ErrorKind::NotFound {
                let parent_dir_path = file_path.parent().unwrap();
                match std::fs::create_dir_all(parent_dir_path) {
                    Ok(()) => {
                        return atomic_write_file_raw(&temp_file_path, file_path, data, mode)
                            .map_err(|err| add_file_context(file_path, err));
                    }
                    Err(create_err) => {
                        if !parent_dir_path.exists() {
                            return Err(Error::new(
                                create_err.kind(),
                                format!(
                                    "{:#} (for '{}')\nCheck the permission of the directory.",
                                    create_err,
                                    parent_dir_path.display()
                                ),
                            ));
                        }
                    }
                }
            }
            return Err(add_file_context(file_path, write_err));
        }
        Ok(())
    }

    inner(file_path, data.as_ref(), mode)
}

pub fn write_file<T: AsRef<[u8]>>(filename: &Path, data: T, mode: u32) -> std::io::Result<()> {
    write_file_2(filename, data, true, mode, true, false)
}

pub fn write_file_2<T: AsRef<[u8]>>(
    filename: &Path,
    data: T,
    update_mode: bool,
    mode: u32,
    is_create: bool,
    is_append: bool,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .read(false)
        .write(true)
        .append(is_append)
        .truncate(!is_append)
        .create(is_create)
        .open(filename)?;

    if update_mode {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = mode & 0o777;
            let permissions = PermissionsExt::from_mode(mode);
            file.set_permissions(permissions)?;
        }
        #[cfg(not(unix))]
        let _ = mode;
    }

    file.write_all(data.as_ref())
}

/// Similar to `std::fs::canonicalize()` but strips UNC prefixes on Windows.
pub fn canonicalize_path(path: &Path) -> Result<PathBuf, Error> {
    Ok(deno_core::strip_unc_prefix(path.canonicalize()?))
}

/// Canonicalizes a path which might be non-existent by going up the
/// ancestors until it finds a directory that exists, canonicalizes
/// that path, then adds back the remaining path components.
///
/// Note: When using this, you should be aware that a symlink may
/// subsequently be created along this path by some other code.
pub fn canonicalize_path_maybe_not_exists(path: &Path) -> Result<PathBuf, Error> {
    canonicalize_path_maybe_not_exists_with_custom_fn(path, canonicalize_path)
}

pub fn canonicalize_path_maybe_not_exists_with_fs(
    path: &Path,
    fs: &dyn FileSystem,
) -> Result<PathBuf, Error> {
    canonicalize_path_maybe_not_exists_with_custom_fn(path, |path| {
        fs.realpath_sync(path).map_err(|err| err.into_io_error())
    })
}

pub fn canonicalize_path_maybe_not_exists_with_custom_fn(
    path: &Path,
    canonicalize: impl Fn(&Path) -> Result<PathBuf, Error>,
) -> Result<PathBuf, Error> {
    let path = path.to_path_buf().clean();
    let mut path = path.as_path();
    let mut names_stack = Vec::new();
    loop {
        match canonicalize(path) {
            Ok(mut canonicalized_path) => {
                for name in names_stack.into_iter().rev() {
                    canonicalized_path = canonicalized_path.join(name);
                }
                return Ok(canonicalized_path);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                names_stack.push(path.file_name().unwrap());
                path = path.parent().unwrap();
            }
            Err(err) => return Err(err),
        }
    }
}

pub fn resolve_from_cwd(path: &Path) -> Result<PathBuf, AnyError> {
    let resolved_path = if path.is_absolute() {
        path.to_owned()
    } else {
        let cwd = current_dir().context("Failed to get current working directory")?;
        cwd.join(path)
    };

    Ok(normalize_path(resolved_path))
}

mod clone_dir_imp {

    #[cfg(target_vendor = "apple")]
    mod apple {
        use super::super::copy_dir_recursive;
        use deno_core::error::AnyError;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;
        fn clonefile(from: &Path, to: &Path) -> std::io::Result<()> {
            let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
            let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
            // SAFETY: `from` and `to` are valid C strings.
            let ret = unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) };
            if ret != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn clone_dir_recursive(from: &Path, to: &Path) -> Result<(), AnyError> {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Try to clone the whole directory
            if let Err(err) = clonefile(from, to) {
                if err.kind() != std::io::ErrorKind::AlreadyExists {
                    log::warn!(
                        "Failed to clone dir {:?} to {:?} via clonefile: {}",
                        from,
                        to,
                        err
                    );
                }
                // clonefile won't overwrite existing files, so if the dir exists
                // we need to handle it recursively.
                copy_dir_recursive(from, to)?;
            }

            Ok(())
        }
    }

    #[cfg(target_vendor = "apple")]
    pub(super) use apple::clone_dir_recursive;

    #[cfg(not(target_vendor = "apple"))]
    pub(super) fn clone_dir_recursive(
        from: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<(), deno_core::error::AnyError> {
        if let Err(e) = super::hard_link_dir_recursive(from, to) {
            log::debug!("Failed to hard link dir {:?} to {:?}: {}", from, to, e);
            super::copy_dir_recursive(from, to)?;
        }

        Ok(())
    }
}

/// Clones a directory to another directory. The exact method
/// is not guaranteed - it may be a hardlink, copy, or other platform-specific
/// operation.
///
/// Note: Does not handle symlinks.
pub fn clone_dir_recursive(from: &Path, to: &Path) -> Result<(), AnyError> {
    clone_dir_imp::clone_dir_recursive(from, to)
}

/// Copies a directory to another directory.
///
/// Note: Does not handle symlinks.
pub fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), AnyError> {
    std::fs::create_dir_all(to).with_context(|| format!("Creating {}", to.display()))?;
    let read_dir =
        std::fs::read_dir(from).with_context(|| format!("Reading {}", from.display()))?;

    for entry in read_dir {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let new_from = from.join(entry.file_name());
        let new_to = to.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&new_from, &new_to)
                .with_context(|| format!("Dir {} to {}", new_from.display(), new_to.display()))?;
        } else if file_type.is_file() {
            std::fs::copy(&new_from, &new_to).with_context(|| {
                format!("Copying {} to {}", new_from.display(), new_to.display())
            })?;
        }
    }

    Ok(())
}

/// Hardlinks the files in one directory to another directory.
///
/// Note: Does not handle symlinks.
pub fn hard_link_dir_recursive(from: &Path, to: &Path) -> Result<(), AnyError> {
    std::fs::create_dir_all(to).with_context(|| format!("Creating {}", to.display()))?;
    let read_dir =
        std::fs::read_dir(from).with_context(|| format!("Reading {}", from.display()))?;

    for entry in read_dir {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let new_from = from.join(entry.file_name());
        let new_to = to.join(entry.file_name());

        if file_type.is_dir() {
            hard_link_dir_recursive(&new_from, &new_to)
                .with_context(|| format!("Dir {} to {}", new_from.display(), new_to.display()))?;
        } else if file_type.is_file() {
            // note: chance for race conditions here between attempting to create,
            // then removing, then attempting to create. There doesn't seem to be
            // a way to hard link with overwriting in Rust, but maybe there is some
            // way with platform specific code. The workaround here is to handle
            // scenarios where something else might create or remove files.
            if let Err(err) = std::fs::hard_link(&new_from, &new_to) {
                if err.kind() == ErrorKind::AlreadyExists {
                    if let Err(err) = std::fs::remove_file(&new_to) {
                        if err.kind() == ErrorKind::NotFound {
                            // Assume another process/thread created this hard link to the file we are wanting
                            // to remove then sleep a little bit to let the other process/thread move ahead
                            // faster to reduce contention.
                            std::thread::sleep(Duration::from_millis(10));
                        } else {
                            return Err(err).with_context(|| {
                                format!(
                                    "Removing file to hard link {} to {}",
                                    new_from.display(),
                                    new_to.display()
                                )
                            });
                        }
                    }

                    // Always attempt to recreate the hardlink. In contention scenarios, the other process
                    // might have been killed or exited after removing the file, but before creating the hardlink
                    if let Err(err) = std::fs::hard_link(&new_from, &new_to) {
                        // Assume another process/thread created this hard link to the file we are wanting
                        // to now create then sleep a little bit to let the other process/thread move ahead
                        // faster to reduce contention.
                        if err.kind() == ErrorKind::AlreadyExists {
                            std::thread::sleep(Duration::from_millis(10));
                        } else {
                            return Err(err).with_context(|| {
                                format!(
                                    "Hard linking {} to {}",
                                    new_from.display(),
                                    new_to.display()
                                )
                            });
                        }
                    }
                } else {
                    return Err(err).with_context(|| {
                        format!(
                            "Hard linking {} to {}",
                            new_from.display(),
                            new_to.display()
                        )
                    });
                }
            }
        }
    }

    Ok(())
}

pub fn symlink_dir(oldpath: &Path, newpath: &Path) -> Result<(), AnyError> {
    let err_mapper = |err: Error| {
        Error::new(
            err.kind(),
            format!(
                "{}, symlink '{}' -> '{}'",
                err,
                oldpath.display(),
                newpath.display()
            ),
        )
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(oldpath, newpath).map_err(err_mapper)?;
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::fs::symlink_dir;
        symlink_dir(oldpath, newpath).map_err(err_mapper)?;
    }
    Ok(())
}

/// Gets the total size (in bytes) of a directory.
pub fn dir_size(path: &Path) -> std::io::Result<u64> {
    let entries = std::fs::read_dir(path)?;
    let mut total = 0;
    for entry in entries {
        let entry = entry?;
        total += match entry.metadata()? {
            data if data.is_dir() => dir_size(&entry.path())?,
            data => data.len(),
        };
    }
    Ok(total)
}

struct LaxSingleProcessFsFlagInner {
    file_path: PathBuf,
    fs_file: std::fs::File,
    finished_token: Arc<tokio_util::sync::CancellationToken>,
}

impl Drop for LaxSingleProcessFsFlagInner {
    fn drop(&mut self) {
        use fs3::FileExt;
        // kill the poll thread
        self.finished_token.cancel();
        // release the file lock
        if let Err(err) = self.fs_file.unlock() {
            log::debug!(
                "Failed releasing lock for {}. {:#}",
                self.file_path.display(),
                err
            );
        }
    }
}

/// A file system based flag that will attempt to synchronize multiple
/// processes so they go one after the other. In scenarios where
/// synchronization cannot be achieved, it will allow the current process
/// to proceed.
///
/// This should only be used in places where it's ideal for multiple
/// processes to not update something on the file system at the same time,
/// but it's not that big of a deal.
#[allow(dead_code)]
pub struct LaxSingleProcessFsFlag(Option<LaxSingleProcessFsFlagInner>);

impl LaxSingleProcessFsFlag {
    #[allow(clippy::suspicious_open_options)]
    pub async fn lock(file_path: PathBuf, _long_wait_message: &str) -> Self {
        debug!("Acquiring file lock at {}", file_path.display());
        use fs3::FileExt;
        let last_updated_path = file_path.with_extension("lock.poll");
        let _start_instant = std::time::Instant::now();
        let open_result = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&file_path);

        match open_result {
            Ok(fs_file) => {
                let mut error_count = 0;
                while error_count < 10 {
                    let lock_result = fs_file.try_lock_exclusive();
                    let poll_file_update_ms = 100;
                    match lock_result {
                        Ok(_) => {
                            log::debug!("Acquired file lock at {}", file_path.display());
                            let _ignore = std::fs::write(&last_updated_path, "");
                            let token = Arc::new(tokio_util::sync::CancellationToken::new());

                            // Spawn a blocking task that will continually update a file
                            // signalling the lock is alive. This is a fail safe for when
                            // a file lock is never released. For example, on some operating
                            // systems, if a process does not release the lock (say it's
                            // killed), then the OS may release it at an indeterminate time
                            //
                            // This uses a blocking task because we use a single threaded
                            // runtime and this is time sensitive so we don't want it to update
                            // at the whims of of whatever is occurring on the runtime thread.
                            spawn_blocking({
                                let token = token.clone();
                                let last_updated_path = last_updated_path.clone();
                                move || {
                                    let mut i = 0;
                                    while !token.is_cancelled() {
                                        i += 1;
                                        let _ignore =
                                            std::fs::write(&last_updated_path, i.to_string());
                                        std::thread::sleep(Duration::from_millis(
                                            poll_file_update_ms,
                                        ));
                                    }
                                }
                            });

                            return Self(Some(LaxSingleProcessFsFlagInner {
                                file_path,
                                fs_file,
                                finished_token: token,
                            }));
                        }
                        Err(_) => {
                            // sleep for a little bit
                            tokio::time::sleep(Duration::from_millis(20)).await;

                            // Poll the last updated path to check if it's stopped updating,
                            // which is an indication that the file lock is claimed, but
                            // was never properly released.
                            match std::fs::metadata(&last_updated_path).and_then(|p| p.modified()) {
                                Ok(last_updated_time) => {
                                    let current_time = std::time::SystemTime::now();
                                    match current_time.duration_since(last_updated_time) {
                                        Ok(duration) => {
                                            if duration.as_millis()
                                                > (poll_file_update_ms * 2) as u128
                                            {
                                                // the other process hasn't updated this file in a long time
                                                // so maybe it was killed and the operating system hasn't
                                                // released the file lock yet
                                                return Self(None);
                                            } else {
                                                error_count = 0; // reset
                                            }
                                        }
                                        Err(_) => {
                                            error_count += 1;
                                        }
                                    }
                                }
                                Err(_) => {
                                    error_count += 1;
                                }
                            }
                        }
                    }
                }

                Self(None)
            }
            Err(err) => {
                log::debug!(
                    "Failed to open file lock at {}. {:#}",
                    file_path.display(),
                    err
                );
                Self(None) // let the process through
            }
        }
    }
}

/// Attempts to convert a specifier to a file path. By default, uses the Url
/// crate's `to_file_path()` method, but falls back to try and resolve unix-style
/// paths on Windows.
pub fn specifier_to_file_path(specifier: &ModuleSpecifier) -> Result<PathBuf, AnyError> {
    let result = if specifier.scheme() != "file" {
        Err(())
    } else if cfg!(windows) {
        match specifier.to_file_path() {
            Ok(path) => Ok(path),
            Err(()) => {
                // This might be a unix-style path which is used in the tests even on Windows.
                // Attempt to see if we can convert it to a `PathBuf`. This code should be removed
                // once/if https://github.com/servo/rust-url/issues/730 is implemented.
                if specifier.scheme() == "file"
                    && specifier.host().is_none()
                    && specifier.port().is_none()
                    && specifier.path_segments().is_some()
                {
                    let path_str = specifier.path();
                    match String::from_utf8(
                        percent_encoding::percent_decode(path_str.as_bytes()).collect(),
                    ) {
                        Ok(path_str) => Ok(PathBuf::from(path_str)),
                        Err(_) => Err(()),
                    }
                } else {
                    Err(())
                }
            }
        }
    } else {
        specifier.to_file_path()
    };
    match result {
        Ok(path) => Ok(path),
        Err(()) => Err(uri_error(format!(
            "Invalid file path.\n  Specifier: {specifier}"
        ))),
    }
}
