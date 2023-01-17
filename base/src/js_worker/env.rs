use crate::js_worker::permissions::Permissions;

use deno_core::error::type_error;
use deno_core::error::AnyError;
use deno_core::include_js_files;
use deno_core::op;
use deno_core::Extension;
use deno_core::OpState;
use deno_node::NODE_ENV_VAR_ALLOWLIST;
use std::collections::HashMap;
use std::env;

pub fn init() -> Extension {
    Extension::builder()
        .js(include_js_files!(
          prefix "custom:ext/env",
          "js/env.js",
        ))
        .ops(vec![
            op_env::decl(),
            op_delete_env::decl(),
            op_get_env::decl(),
            op_set_env::decl(),
        ])
        .build()
}

#[op]
fn op_set_env(state: &mut OpState, key: String, value: String) -> Result<(), AnyError> {
    state.borrow_mut::<Permissions>().check_env(&key)?;
    if key.is_empty() {
        return Err(type_error("Key is an empty string."));
    }
    if key.contains(&['=', '\0'] as &[char]) {
        return Err(type_error(format!(
            "Key contains invalid characters: {:?}",
            key
        )));
    }
    if value.contains('\0') {
        return Err(type_error(format!(
            "Value contains invalid characters: {:?}",
            value
        )));
    }
    env::set_var(key, value);
    Ok(())
}

#[op]
fn op_env(state: &mut OpState) -> Result<HashMap<String, String>, AnyError> {
    state.borrow_mut::<Permissions>().check_env_all()?;
    Ok(env::vars().collect())
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

    let r = match env::var(key) {
        Err(env::VarError::NotPresent) => None,
        v => Some(v?),
    };
    Ok(r)
}

#[op]
fn op_delete_env(state: &mut OpState, key: String) -> Result<(), AnyError> {
    state.borrow_mut::<Permissions>().check_env(&key)?;
    if key.is_empty() || key.contains(&['=', '\0'] as &[char]) {
        return Err(type_error("Key contains invalid characters."));
    }
    env::remove_var(key);
    Ok(())
}
