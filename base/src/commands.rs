use crate::server::Server;
use anyhow::Error;

#[tokio::main]
pub async fn start_server(ip: &str, port: u16, main_service_path: String) -> Result<(), Error> {
    let mut server = Server::new(ip, port, main_service_path).await?;
    server.listen().await
}
