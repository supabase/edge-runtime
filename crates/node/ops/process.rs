// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use deno_core::error::not_supported;
use deno_core::error::AnyError;
use deno_core::op2;

// #[cfg(unix)]
// fn kill(pid: i32, sig: i32) -> i32 {
//     // SAFETY: FFI call to libc
//     if unsafe { libc::kill(pid, sig) } < 0 {
//         std::io::Error::last_os_error().raw_os_error().unwrap()
//     } else {
//         0
//     }
// }

// #[cfg(not(unix))]
// fn kill(pid: i32, _sig: i32) -> i32 {
//     use winapi::shared::minwindef::DWORD;
//     use winapi::shared::minwindef::FALSE;
//     use winapi::shared::minwindef::TRUE;
//     use winapi::um::errhandlingapi::GetLastError;
//     use winapi::um::processthreadsapi::GetCurrentProcess;
//     use winapi::um::processthreadsapi::OpenProcess;
//     use winapi::um::processthreadsapi::TerminateProcess;
//     use winapi::um::winnt::PROCESS_TERMINATE;

//     // SAFETY: FFI call to winapi
//     unsafe {
//         let p_hnd = if pid == 0 {
//             GetCurrentProcess()
//         } else {
//             OpenProcess(PROCESS_TERMINATE, FALSE, pid as DWORD)
//         };

//         if p_hnd.is_null() {
//             return GetLastError() as i32;
//         }

//         if TerminateProcess(p_hnd, 1) == TRUE {
//             return 0;
//         }

//         GetLastError() as i32
//     }
// }

#[op2(fast)]
pub fn op_node_process_kill() -> Result<i32, AnyError> {
    Err(not_supported())
}

#[op2(fast)]
pub fn op_process_abort() {}
