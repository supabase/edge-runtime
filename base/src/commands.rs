use crate::server::Server;
use anyhow::Error;

#[tokio::main]
pub async fn start_server(ip: &str, port: u16, services_dir: String) -> Result<(), Error> {
    let server = Server::new(ip, port, services_dir)?;
    server.listen().await
}
