use crate::permissions::Permissions;
use anyhow::Context;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::ModuleSpecifier;
use deno_core::OpState;

#[op]
fn op_main_module(state: &mut OpState) -> Result<String, AnyError> {
    let main = state.borrow::<ModuleSpecifier>().to_string();
    let main_url = deno_core::resolve_url_or_path(&main, std::env::current_dir()?.as_path())?;
    if main_url.scheme() == "file" {
        let main_path = std::env::current_dir()
            .context("Failed to get current working directory")?
            .join(main_url.to_string());
        state.borrow_mut::<Permissions>().check_read_blind(
            &main_path,
            "main_module",
            "Deno.mainModule",
        )?;
    }
    Ok(main)
}

#[op]
fn op_build_target(_state: &mut OpState) -> String {
    let target = env!("TARGET").to_string();
    target
}

deno_core::extension!(sb_core_runtime,
    ops = [op_main_module, op_build_target],
    options = {
        main_module: Option<ModuleSpecifier>
    },
    state = |state, options| {
        if let Some(module_init) = options.main_module {
            state.put::<ModuleSpecifier>(module_init.clone());
        }
    },
);
