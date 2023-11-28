pub mod cert;
pub mod errors_rt;
pub mod http_start;
pub mod net;
pub mod permissions;
pub mod runtime;
pub mod transpiler;

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
