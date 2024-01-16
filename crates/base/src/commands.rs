use crate::{
    rt_worker::{worker_ctx::TerminationToken, worker_pool::WorkerPoolPolicy},
    server::{Server, ServerHealth, WorkerEntrypoints},
};
use anyhow::Error;
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
    callback_tx: Option<Sender<ServerHealth>>,
    entrypoints: WorkerEntrypoints,
    termination_token: Option<TerminationToken>,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        event_worker_path,
        user_worker_policy,
        import_map_path,
        no_module_cache,
        callback_tx,
        entrypoints,
        termination_token,
    )
    .await?;

    server.listen().await
}
