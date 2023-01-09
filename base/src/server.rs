use crate::js_worker::JsWorker;
use anyhow::{bail, Error};
use log::{error, info};
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use tokio::net::{TcpListener, TcpStream};

async fn process_stream(stream: TcpStream, services_dir: String) -> Result<(), Error> {
    // parse HTTP header
    // find request path
    // try to find a matching function from services_dir
    // if no function found return a 404
    // if function found boot a new JS worker,
    // and pass the incoming tcp request
    // peek into the request in 128 byte chunks
    let mut buf = [0; 128];
    let _ = stream.peek(&mut buf).await?;

    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    let _ = req.parse(&buf).unwrap();

    if req.path.is_none() {
        // if the path isn't found in first 128 bytes it must not be a valid http
        // request
        bail!("invalid request")
    }

    let req_path = String::from(req.path.unwrap());
    let service_name = req_path.split("/").nth(1).unwrap_or_default();

    if service_name == "" {
        bail!("service name cannot be empty")
        // send a 400 response
    }

    let service_path = Path::new(&services_dir).join(service_name);
    if !service_path.exists() {
        bail!("service does not exist")
        // send a 404 response
    }

    info!("serving function {}", service_name);

    let _ = JsWorker::serve(service_path.to_path_buf(), stream).await?;

    Ok(())
}

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    services_dir: String,
}

impl Server {
    pub fn new(ip: &str, port: u16, services_dir: String) -> Result<Self, Error> {
        let ip = Ipv4Addr::from_str(ip)?;
        Ok(Self {
            ip,
            port,
            services_dir,
        })
    }

    pub async fn listen(&self) -> Result<(), Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let listener = TcpListener::bind(&addr).await?;
        info!("listening on {:?}", listener.local_addr()?);

        loop {
            tokio::select! {
                msg = listener.accept() => {
                    match msg {
                       Ok((stream, _)) => {
                           let services_dir_clone = self.services_dir.clone();
                           tokio::task::spawn(async move {
                             let res = process_stream(stream, services_dir_clone).await;
                             if res.is_err() {
                                 error!("{:?}", res.err().unwrap());
                             }
                           });
                       }
                       Err(e) => error!("socket error: {}", e)
                    }
                }
                // wait for shutdown signal...
                _ = tokio::signal::ctrl_c() => {
                    info!("shutdown signal received");
                    break;
                }
            }
        }
        Ok(())
    }
}
