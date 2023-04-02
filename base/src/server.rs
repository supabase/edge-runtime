use crate::worker_ctx::{WorkerContext, WorkerPool};
use anyhow::Error;
use hyper::{server::conn::Http, service::Service, Body, Request, Response};
use log::{debug, error, info};
use std::future::Future;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use url::Url;

struct WorkerService {
    worker_ctx: Arc<RwLock<WorkerContext>>,
}

impl WorkerService {
    fn new(worker_ctx: Arc<RwLock<WorkerContext>>) -> Self {
        Self { worker_ctx }
    }
}

impl Service<Request<Body>> for WorkerService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // create a response in a future.
        let worker_ctx = self.worker_ctx.clone();
        let fut = async move {
            let req_path = req.uri().path();

            // if the request is for the health endpoint return a 200 OK response
            if req_path == "/_internal/health" {
                return Ok(Response::new(Body::empty()));
            }

            let mut worker_ctx_writer = worker_ctx.write().await;
            let response = worker_ctx_writer.send_request(req).await?;
            Ok(response)
        };

        // Return the response as an immediate future
        Box::pin(fut)
    }
}

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    worker_pool: WorkerPool,
}

impl Server {
    pub async fn new(ip: &str, port: u16, main_service_path: String) -> Result<Self, Error> {
        // create a worker pool
        let worker_pool = WorkerPool::new(main_service_path, None, false).await?;

        let ip = Ipv4Addr::from_str(ip)?;
        Ok(Self {
            ip,
            port,
            worker_pool,
        })
    }

    pub async fn listen(&mut self) -> Result<(), Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let listener = TcpListener::bind(&addr).await?;
        debug!("edge-runtime is listening on {:?}", listener.local_addr()?);

        let main_worker = &self.worker_pool.main_worker;

        loop {
            tokio::select! {
                msg = listener.accept() => {
                    match msg {
                       Ok((conn, _)) => {
                           let main_worker = main_worker.clone();
                           tokio::task::spawn(async move {
                             let service = WorkerService::new(main_worker);

                             let conn_fut = Http::new()
                                .serve_connection(conn, service);

                             if let Err(e) = conn_fut.await {
                                 error!("{:?}", e);
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
