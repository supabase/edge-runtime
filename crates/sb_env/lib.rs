use deno_core::error::AnyError;
use deno_core::error::{not_supported, type_error};
use deno_core::include_js_files;
use deno_core::op;
use deno_core::Extension;
use deno_core::OpState;
use deno_node::NODE_ENV_VAR_ALLOWLIST;
use sb_core::permissions::Permissions;
use std::collections::HashMap;
use std::path::Path;

pub type EnvVars = HashMap<String, String>;

deno_core::extension!(
    sb_env,
    ops = [op_set_env, op_env, op_get_env, op_delete_env],
    esm = ["env.js"]
);

#[op]
fn op_set_env(_state: &mut OpState, _key: String, _value: String) -> Result<(), AnyError> {
    Err(not_supported())
}

#[op]
fn op_env(state: &mut OpState) -> Result<HashMap<String, String>, AnyError> {
    state.borrow_mut::<Permissions>().check_env_all()?;
    let env_vars = state.borrow::<EnvVars>();
    Ok(env_vars.clone())
}

#[op]
fn op_get_env(state: &mut OpState, key: String) -> Result<Option<String>, AnyError> {
    let skip_permission_check = NODE_ENV_VAR_ALLOWLIST.contains(&key);

    if !skip_permission_check {
        state.borrow_mut::<Permissions>().check_env(&key)?;
    }

    if key.is_empty() {
        return Err(type_error("Key is an empty string."));
    }

    if key.contains(&['=', '\0'] as &[char]) {
        return Err(type_error(format!(
            "Key contains invalid characters: {:?}",
            key
        )));
    }

    let env_vars = state.borrow::<EnvVars>();
    let r = env_vars.get(&key).map(|k| k.clone());
    Ok(r)
}

#[op]
fn op_delete_env(_state: &mut OpState, _key: String) -> Result<(), AnyError> {
    Err(not_supported())
}
