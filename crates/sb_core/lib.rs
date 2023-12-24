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

deno_core::extension!(
    sb_core_main_js,
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
