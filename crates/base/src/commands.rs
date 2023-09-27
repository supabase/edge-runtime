use crate::server::{Server, ServerCodes};
use anyhow::Error;
use tokio::sync::mpsc::Sender;

pub async fn start_server(
    ip: &str,
    port: u16,
    main_service_path: String,
    event_worker_path: Option<String>,
    import_map_path: Option<String>,
    no_module_cache: bool,
    callback_tx: Option<Sender<ServerCodes>>,
    maybe_main_entrypoint: Option<String>,
    maybe_events_entrypoint: Option<String>,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        event_worker_path,
        import_map_path,
        no_module_cache,
        callback_tx,
        maybe_main_entrypoint,
        maybe_events_entrypoint,
    )
    .await?;
    server.listen().await
}
