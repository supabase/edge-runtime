// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use deno_fs::FileSystemRc;
use serde::Serialize;

use crate::NodePermissions;

#[op2(fast)]
pub fn op_node_fs_exists_sync<P>(
    state: &mut OpState,
    #[string] path: String,
) -> Result<bool, AnyError>
where
    P: NodePermissions + 'static,
{
    let path = PathBuf::from(path);
    state
        .borrow_mut::<P>()
        .check_read_with_api_name(&path, Some("node:fs.existsSync()"))?;
    let fs = state.borrow::<FileSystemRc>();
    Ok(fs.lstat_sync(&path).is_ok())
}

#[op2(fast)]
pub fn op_node_cp_sync<P>(
    state: &mut OpState,
    #[string] path: &str,
    #[string] new_path: &str,
) -> Result<(), AnyError>
where
    P: NodePermissions + 'static,
{
    let path = Path::new(path);
    let new_path = Path::new(new_path);

    state
        .borrow_mut::<P>()
        .check_read_with_api_name(path, Some("node:fs.cpSync"))?;
    state
        .borrow_mut::<P>()
        .check_write_with_api_name(new_path, Some("node:fs.cpSync"))?;

    let fs = state.borrow::<FileSystemRc>();
    fs.cp_sync(path, new_path)?;
    Ok(())
}

#[op2(async)]
pub async fn op_node_cp<P>(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] new_path: String,
) -> Result<(), AnyError>
where
    P: NodePermissions + 'static,
{
    let path = PathBuf::from(path);
    let new_path = PathBuf::from(new_path);

    let fs = {
        let mut state = state.borrow_mut();
        state
            .borrow_mut::<P>()
            .check_read_with_api_name(&path, Some("node:fs.cpSync"))?;
        state
            .borrow_mut::<P>()
            .check_write_with_api_name(&new_path, Some("node:fs.cpSync"))?;
        state.borrow::<FileSystemRc>().clone()
    };

    fs.cp_async(path, new_path).await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct StatFs {
    #[serde(rename = "type")]
    pub typ: u64,
    pub bsize: u64,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
}

#[op2]
#[serde]
pub fn op_node_statfs<P>(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    bigint: bool,
) -> Result<StatFs, AnyError>
where
    P: NodePermissions + 'static,
{
    {
        let mut state = state.borrow_mut();
        state
            .borrow_mut::<P>()
            .check_read_with_api_name(Path::new(&path), Some("node:fs.statfs"))?;
        state
            .borrow_mut::<P>()
            .check_sys("statfs", "node:fs.statfs")?;
    }
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = OsStr::new(&path);
        let mut cpath = path.as_bytes().to_vec();
        cpath.push(0);
        if bigint {
            #[cfg(not(target_os = "macos"))]
            // SAFETY: `cpath` is NUL-terminated and result is pointer to valid statfs memory.
            let (code, result) = unsafe {
                let mut result: libc::statfs64 = std::mem::zeroed();
                (libc::statfs64(cpath.as_ptr() as _, &mut result), result)
            };
            #[cfg(target_os = "macos")]
            // SAFETY: `cpath` is NUL-terminated and result is pointer to valid statfs memory.
            let (code, result) = unsafe {
                let mut result: libc::statfs = std::mem::zeroed();
                (libc::statfs(cpath.as_ptr() as _, &mut result), result)
            };
            if code == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(StatFs {
                typ: result.f_type as _,
                bsize: result.f_bsize as _,
                blocks: result.f_blocks as _,
                bfree: result.f_bfree as _,
                bavail: result.f_bavail as _,
                files: result.f_files as _,
                ffree: result.f_ffree as _,
            })
        } else {
            // SAFETY: `cpath` is NUL-terminated and result is pointer to valid statfs memory.
            let (code, result) = unsafe {
                let mut result: libc::statfs = std::mem::zeroed();
                (libc::statfs(cpath.as_ptr() as _, &mut result), result)
            };
            if code == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(StatFs {
                typ: result.f_type as _,
                bsize: result.f_bsize as _,
                blocks: result.f_blocks as _,
                bfree: result.f_bfree as _,
                bavail: result.f_bavail as _,
                files: result.f_files as _,
                ffree: result.f_ffree as _,
            })
        }
    }
    #[cfg(windows)]
    {
        use deno_core::anyhow::anyhow;
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;

        let _ = bigint;
        // Using a vfs here doesn't make sense, it won't align with the windows API
        // call below.
        #[allow(clippy::disallowed_methods)]
        let path = Path::new(&path).canonicalize()?;
        let root = path
            .ancestors()
            .last()
            .ok_or(anyhow!("Path has no root."))?;
        let mut root = OsStr::new(root).encode_wide().collect::<Vec<_>>();
        root.push(0);
        let mut sectors_per_cluster = 0;
        let mut bytes_per_sector = 0;
        let mut available_clusters = 0;
        let mut total_clusters = 0;
        let mut code = 0;
        let mut retries = 0;
        // We retry here because libuv does: https://github.com/libuv/libuv/blob/fa6745b4f26470dae5ee4fcbb1ee082f780277e0/src/win/fs.c#L2705
        while code == 0 && retries < 2 {
            // SAFETY: Normal GetDiskFreeSpaceW usage.
            code = unsafe {
                GetDiskFreeSpaceW(
                    root.as_ptr(),
                    &mut sectors_per_cluster,
                    &mut bytes_per_sector,
                    &mut available_clusters,
                    &mut total_clusters,
                )
            };
            retries += 1;
        }
        if code == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(StatFs {
            typ: 0,
            bsize: (bytes_per_sector * sectors_per_cluster) as _,
            blocks: total_clusters as _,
            bfree: available_clusters as _,
            bavail: available_clusters as _,
            files: 0,
            ffree: 0,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        let _ = bigint;
        Err(anyhow!("Unsupported platform."))
    }
}
