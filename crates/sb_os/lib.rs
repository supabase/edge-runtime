mod sys_info;

use crate::sys_info::MemInfo;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::OpState;
use std::collections::HashMap;

pub type EnvVars = HashMap<String, String>;

deno_core::extension!(
    sb_os,
    ops = [
        op_gid,
        op_uid,
        op_system_memory_info,
        op_hostname,
        op_loadavg,
        op_os_release,
        op_os_uptime
    ],
    esm = ["os.js"]
);

#[cfg(not(windows))]
#[op]
fn op_gid(_state: &mut OpState) -> Result<Option<u32>, AnyError> {
    Ok(Some(1000_u32))
}

#[cfg(windows)]
#[op]
fn op_gid(_state: &mut OpState) -> Result<Option<u32>, AnyError> {
    Ok(None)
}

#[cfg(not(windows))]
#[op]
fn op_uid(_state: &mut OpState) -> Result<Option<u32>, AnyError> {
    Ok(Some(1000_u32))
}

#[op]
fn op_system_memory_info(_state: &mut OpState) -> Result<Option<sys_info::MemInfo>, AnyError> {
    Ok(Some(MemInfo {
        total: 0,
        free: 0,
        available: 0,
        buffers: 0,
        cached: 0,
        swap_total: 0,
        swap_free: 0,
    }))
}

#[cfg(windows)]
#[op]
fn op_uid(state: &mut OpState) -> Result<Option<u32>, AnyError> {
    Ok(None)
}

#[op]
fn op_hostname(_state: &mut OpState) -> Result<String, AnyError> {
    Ok(String::from("localhost"))
}

#[op]
fn op_loadavg(_state: &mut OpState) -> Result<(f64, f64, f64), AnyError> {
    Ok(sys_info::DEFAULT_LOADAVG)
}

#[op]
fn op_os_release(_state: &mut OpState) -> Result<String, AnyError> {
    Ok(String::from("0.0.0-00000000-generic"))
}

#[op]
fn op_os_uptime(_state: &mut OpState) -> Result<u64, AnyError> {
    Ok(sys_info::os_uptime())
}
