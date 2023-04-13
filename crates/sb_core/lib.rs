pub mod http_start;
pub mod net;
pub mod permissions;
pub mod runtime;

deno_core::extension!(
    sb_core_main_js,
    esm = [
        "js/user_runtime_loader.js",
        "js/bootstrap.js",
        "js/main_worker.js"
    ]
);
