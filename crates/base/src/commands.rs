use crate::{
    rt_worker::worker_pool::WorkerPoolPolicy,
    server::{Server, ServerCodes, WorkerEntrypoints},
};
use anyhow::Error;
use deno_core::JsRuntime;
use log::error;
use tokio::sync::mpsc::Sender;

#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    ip: &str,
    port: u16,
    main_service_path: String,
    event_worker_path: Option<String>,
    user_worker_policy: Option<WorkerPoolPolicy>,
    import_map_path: Option<String>,
    no_module_cache: bool,
    no_signal_handler: bool,
    callback_tx: Option<Sender<ServerCodes>>,
    entrypoints: WorkerEntrypoints,
) -> Result<(), Error> {
    set_v8_flags();

    // NOTE(denoland/deno/20495): Due to the new PKU (Memory Protection Keys)
    // feature introduced in V8 11.6, We need to initialize the V8 platform on
    // the main thread that spawns V8 isolates.
    JsRuntime::init_platform(None);

    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        event_worker_path,
        user_worker_policy,
        import_map_path,
        no_module_cache,
        no_signal_handler,
        callback_tx,
        entrypoints,
    )
    .await?;
    server.listen().await
}

fn set_v8_flags() {
    let v8_flags = std::env::var("V8_FLAGS").unwrap_or("".to_string());
    let mut vec = vec![""];

    if v8_flags.is_empty() {
        return;
    }

    vec.append(&mut v8_flags.split(' ').collect());

    let ignored = deno_core::v8_set_flags(vec.iter().map(|v| v.to_string()).collect());

    if *ignored.as_slice() != [""] {
        error!("v8 flags unrecognized {:?}", ignored);
    }
}
