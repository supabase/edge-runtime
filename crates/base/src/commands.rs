use crate::{
    inspector_server::Inspector,
    rt_worker::{worker_ctx::TerminationToken, worker_pool::WorkerPoolPolicy},
    server::{Server, ServerFlags, ServerHealth, WorkerEntrypoints},
    InspectorOption,
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
    flags: ServerFlags,
    callback_tx: Option<Sender<ServerHealth>>,
    entrypoints: WorkerEntrypoints,
    termination_token: Option<TerminationToken>,
    inspector_option: Option<InspectorOption>,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        event_worker_path,
        user_worker_policy,
        import_map_path,
        flags,
        callback_tx,
        entrypoints,
        termination_token,
        inspector_option.map(Inspector::from_option),
    )
    .await?;

    server.listen().await
}
