use crate::server::Server;
use anyhow::Error;

pub async fn start_server(
    ip: &str,
    port: u16,
    main_service_path: String,
    import_map_path: Option<String>,
    no_module_cache: bool,
) -> Result<(), Error> {
    let mut server = Server::new(
        ip,
        port,
        main_service_path,
        import_map_path,
        no_module_cache,
    )
    .await?;
    server.listen().await
}
