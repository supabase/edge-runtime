use crate::server::Server;
use anyhow::Error;

#[tokio::main]
pub async fn start_server(
    ip: &str,
    port: u16,
    services_dir: String,
    mem_limit: u16,
    service_timeout: u16,
    no_module_cache: bool,
    import_map_path: Option<String>,
) -> Result<(), Error> {
    let server = Server::new(
        ip,
        port,
        services_dir,
        mem_limit,
        service_timeout,
        no_module_cache,
        import_map_path,
    )?;
    server.listen().await
}