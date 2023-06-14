mod sys_info;

use deno_core::error::AnyError;
use deno_core::op;
use deno_core::OpState;
use std::collections::HashMap;

pub type EnvVars = HashMap<String, String>;

deno_core::extension!(sb_os, ops = [op_os_uptime], esm = ["os.js"]);

#[op]
fn op_os_uptime(_state: &mut OpState) -> Result<u64, AnyError> {
    Ok(sys_info::os_uptime())
}
