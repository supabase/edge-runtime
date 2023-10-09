use crate::permissions::Permissions;
use anyhow::Context;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::ModuleSpecifier;
use deno_core::OpState;

#[op2]
#[string]
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

deno_core::extension!(sb_core_runtime,
    ops = [op_main_module],
    options = {
        main_module: Option<ModuleSpecifier>
    },
    state = |state, options| {
        if let Some(module_init) = options.main_module {
            state.put::<ModuleSpecifier>(module_init);
        }
    },
);
