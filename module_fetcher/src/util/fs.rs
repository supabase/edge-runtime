// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_crypto::rand;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub fn atomic_write_file<T: AsRef<[u8]>>(
    filename: &Path,
    data: T,
    mode: u32,
) -> std::io::Result<()> {
    let rand: String = (0..4)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let extension = format!("{}.tmp", rand);
    let tmp_file = filename.with_extension(extension);
    write_file(&tmp_file, data, mode)?;
    std::fs::rename(tmp_file, filename)?;
    Ok(())
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
