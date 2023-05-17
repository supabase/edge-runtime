pub mod http_start;
pub mod net;
pub mod permissions;
pub mod runtime;

deno_core::extension!(
    sb_core_main_js,
    esm = [
        "js/permissions.js",
        "js/errors.js",
        "js/fieldUtils.js",
        "js/promises.js",
        "js/http.js",
        "js/denoOverrides.js",
        "js/user_runtime_loader.js",
        "js/internal_events.js",
        "js/bootstrap.js",
        "js/main_worker.js",
    ]
);
