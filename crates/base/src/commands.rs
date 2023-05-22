use crate::server::{Server, ServerCodes};
use anyhow::Error;
use tokio::sync::mpsc::Sender;

pub async fn start_server(
    ip: &str,
    port: u16,
    main_service_path: String,
    import_map_path: Option<String>,
    no_module_cache: bool,
    callback_tx: Option<Sender<ServerCodes>>,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        import_map_path,
        no_module_cache,
        callback_tx,
    )
    .await?;
    server.listen().await
}
