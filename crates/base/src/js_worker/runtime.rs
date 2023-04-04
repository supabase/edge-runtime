use crate::js_worker::permissions::Permissions;
use std::path::Path;

use anyhow::Context;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::Extension;
use deno_core::ModuleSpecifier;
use deno_core::OpState;

pub fn init(main_module: ModuleSpecifier) -> Extension {
    Extension::builder("custom:runtime")
        .ops(vec![op_main_module::decl()])
        .state(move |state| {
            state.put::<ModuleSpecifier>(main_module.clone());
            ()
        })
        .build()
}

#[op]
fn op_main_module(state: &mut OpState) -> Result<String, AnyError> {
    let main = state.borrow::<ModuleSpecifier>().to_string();
    let main_url = deno_core::resolve_url_or_path(&main, Path::new("./steve-jobs"))?;
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
