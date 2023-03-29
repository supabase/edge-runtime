use crate::server::Server;
use anyhow::Error;

#[tokio::main]
pub async fn start_server(ip: &str, port: u16) -> Result<(), Error> {
    let mut server = Server::new(ip, port).await?;
    server.listen().await
}
