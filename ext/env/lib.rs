use deno_core::error::AnyError;
use deno_core::error::{not_supported, type_error};
use deno_core::op2;
use deno_core::OpState;
use sb_core::permissions::Permissions;
use sb_node::NODE_ENV_VAR_ALLOWLIST;
use std::collections::HashMap;

#[derive(Default)]
pub struct EnvVars(pub HashMap<String, String>);

impl std::ops::Deref for EnvVars {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for EnvVars {
    fn deref_mut(&mut self) -> &mut Self::Target {
        todo!()
    }
}

deno_core::extension!(
    sb_env,
    ops = [op_set_env, op_env, op_get_env, op_delete_env],
    esm_entry_point = "ext:sb_env/env.js",
    esm = ["env.js"]
);

#[op2(fast)]
fn op_set_env(
    _state: &mut OpState,
    #[string] _key: String,
    #[string] _value: String,
) -> Result<(), AnyError> {
    Err(not_supported())
}

#[op2]
#[serde]
fn op_env(state: &mut OpState) -> Result<HashMap<String, String>, AnyError> {
    state.borrow_mut::<Permissions>().check_env_all()?;
    let env_vars = state.borrow::<EnvVars>();
    Ok(env_vars.0.clone())
}

#[op2]
#[string]
fn op_get_env(state: &mut OpState, #[string] key: String) -> Result<Option<String>, AnyError> {
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
    let r = env_vars.get(&key).cloned();
    Ok(r)
}

#[op2(fast)]
fn op_delete_env(_state: &mut OpState, #[string] _key: String) {}
