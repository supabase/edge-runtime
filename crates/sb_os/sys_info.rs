// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.
#[cfg(target_family = "windows")]
use std::sync::Once;

pub fn os_uptime() -> u64 {
    let uptime: u64;

    #[cfg(target_os = "linux")]
    {
        let mut info = std::mem::MaybeUninit::uninit();
        // SAFETY: `info` is a valid pointer to a `libc::sysinfo` struct.
        let res = unsafe { libc::sysinfo(info.as_mut_ptr()) };
        uptime = if res == 0 {
            // SAFETY: `sysinfo` initializes the struct.
            let info = unsafe { info.assume_init() };
            info.uptime as u64
        } else {
            0
        }
    }

    #[cfg(any(target_vendor = "apple", target_os = "freebsd", target_os = "openbsd"))]
    {
        use std::mem;
        use std::time::Duration;
        use std::time::SystemTime;
        let mut request = [libc::CTL_KERN, libc::KERN_BOOTTIME];
        // SAFETY: `boottime` is only accessed if sysctl() succeeds
        // and agrees with the `size` set by sysctl().
        let mut boottime: libc::timeval = unsafe { mem::zeroed() };
        let mut size: libc::size_t = mem::size_of_val(&boottime) as libc::size_t;
        // SAFETY: `sysctl` is thread-safe.
        let res = unsafe {
            libc::sysctl(
                &mut request[0],
                2,
                &mut boottime as *mut libc::timeval as *mut libc::c_void,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        uptime = if res == 0 {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| {
                    (d - Duration::new(boottime.tv_sec as u64, boottime.tv_usec as u32 * 1000))
                        .as_secs()
                })
                .unwrap_or_default()
        } else {
            0
        }
    }

    #[cfg(target_family = "windows")]
    // SAFETY: windows API usage
    unsafe {
        // Windows is the only one that returns `uptime` in milisecond precision,
        // so we need to get the seconds out of it to be in sync with other envs.
        uptime = winapi::um::sysinfoapi::GetTickCount64() / 1000;
    }

    uptime
}
