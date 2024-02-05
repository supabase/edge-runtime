use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;

pub mod auth_tokens;
pub mod cache;
pub mod cert;
pub mod conn_sync;
pub mod emit;
pub mod errors_rt;
pub mod external_memory;
pub mod file_fetcher;
pub mod http_start;
pub mod net;
pub mod permissions;
pub mod runtime;
pub mod transpiler;
pub mod util;

#[op2(fast)]
fn op_is_terminal(state: &mut OpState, rid: u32) -> Result<bool, AnyError> {
    let handle = state.resource_table.get_handle(rid)?;
    Ok(handle.is_terminal())
}

#[op2(fast)]
fn op_stdin_set_raw(_state: &mut OpState, _is_raw: bool, _cbreak: bool) -> Result<(), AnyError> {
    Ok(())
}

#[op2(fast)]
fn op_console_size(_state: &mut OpState, #[buffer] _result: &mut [u32]) -> Result<(), AnyError> {
    Ok(())
}

#[op2]
#[string]
pub fn op_read_line_prompt(
    #[string] _prompt_text: &str,
    #[string] _default_value: &str,
) -> Result<Option<String>, AnyError> {
    Ok(None)
}

#[op2(fast)]
fn op_set_exit_code(_state: &mut OpState, #[smi] _code: i32) -> Result<(), AnyError> {
    Ok(())
}

deno_core::extension!(
    sb_core_main_js,
    ops = [
        op_is_terminal,
        op_stdin_set_raw,
        op_console_size,
        op_read_line_prompt,
        op_set_exit_code
    ],
    esm_entry_point = "ext:sb_core_main_js/js/bootstrap.js",
    esm = [
        "js/permissions.js",
        "js/errors.js",
        "js/fieldUtils.js",
        "js/promises.js",
        "js/http.js",
        "js/denoOverrides.js",
        "js/navigator.js",
        "js/bootstrap.js",
        "js/main_worker.js",
    ]
);
