use crate::js_worker;
use anyhow::{bail, Error};
use log::{debug, error, info};
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::str;
use std::str::FromStr;
use tokio::net::{TcpListener, TcpStream};
use url::Url;

async fn process_stream(
    stream: TcpStream,
    services_dir: String,
    mem_limit: u16,
    service_timeout: u16,
    no_module_cache: bool,
    import_map_path: Option<String>,
) -> Result<(), Error> {
    // peek into the HTTP header
    // find request path
    // try to find a matching function from services_dir
    // if no function found return a 404
    // if function found boot a new JS worker,
    // and pass the incoming tcp request
    // peek into the request in 1024 byte chunks
    let mut buf = [0; 1024];
    let _ = stream.peek(&mut buf).await?;

    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    let _ = req.parse(&buf).unwrap();

    if req.path.is_none() {
        // if the path isn't found in first 1024 bytes it must not be a valid http
        // request
        bail!("invalid request")
    }

    let host = req
        .headers
        .iter()
        .find(|&&h| h.name.to_lowercase() == "host")
        .map(|h| h.value)
        .unwrap_or_default();
    let host = str::from_utf8(host).unwrap_or("example.com"); // TODO: configure the default host
    let req_path = req.path.unwrap_or_default();

    let url = Url::parse(&format!("http://{}{}", &host, &req_path).as_str())?;
    let path_segments = url.path_segments();
    if path_segments.is_none() {
        bail!("need to provide a path")
        // send a 400 response
    }
    let service_name = path_segments.unwrap().next().unwrap_or_default(); // get the first path segement
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

    let memory_limit_mb = u64::from(mem_limit);
    let worker_timeout_ms = u64::from(service_timeout * 1000);
    let _ = js_worker::serve(
        service_path.to_path_buf(),
        memory_limit_mb,
        worker_timeout_ms,
        no_module_cache,
        import_map_path,
        stream,
    )
    .await?;

    Ok(())
}

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    services_dir: String,
    mem_limit: u16,
    service_timeout: u16,
    no_module_cache: bool,
    import_map_path: Option<String>,
}

impl Server {
    pub fn new(
        ip: &str,
        port: u16,
        services_dir: String,
        mem_limit: u16,
        service_timeout: u16,
        no_module_cache: bool,
        import_map_path: Option<String>,
    ) -> Result<Self, Error> {
        let ip = Ipv4Addr::from_str(ip)?;
        Ok(Self {
            ip,
            port,
            services_dir,
            mem_limit,
            service_timeout,
            no_module_cache,
            import_map_path,
        })
    }

    pub async fn listen(&self) -> Result<(), Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let listener = TcpListener::bind(&addr).await?;
        debug!("edge-runtime is listening on {:?}", listener.local_addr()?);

        loop {
            tokio::select! {
                msg = listener.accept() => {
                    match msg {
                       Ok((stream, _)) => {
                           let services_dir_clone = self.services_dir.clone();
                           let mem_limit = self.mem_limit;
                           let service_timeout = self.service_timeout;
                           let no_module_cache = self.no_module_cache;
                           let import_map_path_clone = self.import_map_path.clone();
                           tokio::task::spawn(async move {
                             let res = process_stream(stream, services_dir_clone, mem_limit, service_timeout, no_module_cache, import_map_path_clone).await;
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
