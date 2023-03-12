use crate::worker_ctx::{CreateWorkerOptions, WorkerRec};
use anyhow::{Context, Error};
use hyper::{server::conn::Http, service::service_fn, Body, Request};
use log::{debug, error, info};
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::str;
use std::str::FromStr;
use tokio::net::{TcpListener, TcpStream};
use url::Url;

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    services_dir: String,
    mem_limit: u16,
    service_timeout: u16,
    no_module_cache: bool,
    import_map_path: Option<String>,
    env_vars: HashMap<String, String>,
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
        env_vars: HashMap<String, String>,
    ) -> Result<Self, Error> {
        // create the main worker

        let ip = Ipv4Addr::from_str(ip)?;
        Ok(Self {
            ip,
            port,
            services_dir,
            mem_limit,
            service_timeout,
            no_module_cache,
            import_map_path,
            env_vars,
        })
    }

    async fn handle_conn(conn: TcpStream) -> Result<(), Error> {
        let service = service_fn(|req: Request<Body>| async move {
            // create a base worker and send the request
            // get the response from the worker
            // check if it contains header 'x-invoke-worker': 'deno'
            // if so, parse the response body and start a worker with provided config
            // pass the modified request to it

            let host = req
                .headers()
                .get("host")
                .map(|v| v.to_str().unwrap())
                .unwrap_or("example.com");
            let req_path = req.uri().path();

            let url = Url::parse(&format!("http://{}{}", host, req_path).as_str())?;
            let path_segments = url.path_segments();
            if path_segments.is_none() {
                error!("need to provide a path");
                // send a 400 response
                //Ok(Response::new(Body::from("need to provide a path")))
            }

            let service_name = path_segments.unwrap().next().unwrap_or_default(); // get the first path segement
            if service_name == "" {
                error!("service name cannot be empty");
                //Ok(Response::new(Body::from("service name cannot be empty")))
            }

            //let service_path = Path::new(&services_dir_clone).join(service_name);
            let service_path = Path::new(&"./examples".to_string()).join(service_name);
            if !service_path.exists() {
                error!("service does not exist");
                // send a 404 response
                //Ok(Response::new(Body::from("service does not exist")))
            }

            info!("serving function {}", service_name);

            //let memory_limit_mb = u64::from(mem_limit);
            let memory_limit_mb = 150 as u64;
            //let worker_timeout_ms = u64::from(service_timeout * 1000);
            let worker_timeout_ms = (60 * 1000) as u64;

            let import_map_path = None;
            let env_vars: HashMap<String, String> = HashMap::new();
            let no_module_cache = false;

            let worker_id = service_path.display().to_string();

            // check if the worker already available

            let mut worker_rec = WorkerRec::new(CreateWorkerOptions {
                service_path: service_path.to_path_buf(),
                memory_limit_mb,
                worker_timeout_ms,
                import_map_path,
                env_vars,
                no_module_cache,
            })
            .await?;

            let response = worker_rec.send_request(req).await?;

            // add the worker to the pool, to resue later

            Ok::<_, Error>(response)
        });

        Http::new()
            .serve_connection(conn, service)
            .await
            .context("Failed to process the incoming request")?;

        Ok(())
    }

    pub async fn listen(&mut self) -> Result<(), Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let listener = TcpListener::bind(&addr).await?;
        debug!("edge-runtime is listening on {:?}", listener.local_addr()?);

        loop {
            tokio::select! {
                msg = listener.accept() => {
                    match msg {
                       Ok((conn, _)) => {
                           tokio::task::spawn(async move {
                             let res = Self::handle_conn(conn).await;
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
