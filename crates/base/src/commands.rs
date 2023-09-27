use crate::server::{Server, ServerCodes, WorkerEntrypoints};
use anyhow::Error;
use tokio::sync::mpsc::Sender;

#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    ip: &str,
    port: u16,
    main_service_path: String,
    event_worker_path: Option<String>,
    import_map_path: Option<String>,
    no_module_cache: bool,
    callback_tx: Option<Sender<ServerCodes>>,
    entrypoints: WorkerEntrypoints,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        event_worker_path,
        import_map_path,
        no_module_cache,
        callback_tx,
        entrypoints,
    )
    .await?;
    server.listen().await
}
