use crate::worker_ctx::{
    create_events_worker, create_user_worker_pool, create_worker, WorkerRequestMsg,
};
use anyhow::Error;
use hyper::{server::conn::Http, service::Service, Body, Request, Response};
use log::{debug, error, info};
use sb_worker_context::essentials::{
    MainWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRuntimeOpts,
};
use sb_worker_context::events::WorkerEventWithMetadata;
use std::future::Future;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::str;
use std::str::FromStr;
use std::task::Poll;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, oneshot};

pub enum ServerCodes {
    Listening,
    Failure,
}

struct WorkerService {
    worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
}

impl WorkerService {
    fn new(worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>) -> Self {
        Self { worker_req_tx }
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
        let worker_req_tx = self.worker_req_tx.clone();
        let fut = async move {
            let req_uri = req.uri().clone();

            let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
            let msg = WorkerRequestMsg { req, res_tx };

            worker_req_tx.send(msg)?;
            let result = res_rx.await?;
            match result {
                Ok(res) => Ok(res),
                Err(e) => {
                    error!(
                        "request failed (uri: {:?} reason: {:?})",
                        req_uri.to_string(),
                        e
                    );
                    // FIXME: add an error body
                    Ok(Response::builder().status(500).body(Body::empty()).unwrap())
                }
            }
        };

        // Return the response as an immediate future
        Box::pin(fut)
    }
}

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    main_worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    callback_tx: Option<Sender<ServerCodes>>,
}

impl Server {
    pub async fn new(
        ip: &str,
        port: u16,
        main_service_path: String,
        maybe_events_service_path: Option<String>,
        import_map_path: Option<String>,
        no_module_cache: bool,
        callback_tx: Option<Sender<ServerCodes>>,
    ) -> Result<Self, Error> {
        let mut worker_events_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>> = None;

        // Create Event Worker
        if let Some(events_service_path) = maybe_events_service_path {
            let events_path = Path::new(&events_service_path);
            let events_path_buf = events_path.to_path_buf();

            let events_worker =
                create_events_worker(events_path_buf, import_map_path.clone(), no_module_cache)
                    .await
                    .expect("Event worker could not be created");

            worker_events_sender = Some(events_worker);
        }

        // Create a user worker pool
        let user_worker_msgs_tx = create_user_worker_pool(worker_events_sender).await?;

        // create main worker
        let main_path = Path::new(&main_service_path);
        let main_worker_req_tx = create_worker(WorkerContextInitOpts {
            service_path: main_path.to_path_buf(),
            import_map_path,
            no_module_cache,
            events_rx: None,
            conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                worker_pool_tx: user_worker_msgs_tx,
            }),
            env_vars: std::env::vars().collect(),
        })
        .await?;

        // register alarm signal handler
        cpu_timer::register_alarm()?;

        let ip = Ipv4Addr::from_str(ip)?;
        Ok(Self {
            ip,
            port,
            main_worker_req_tx,
            callback_tx,
        })
    }

    pub async fn listen(&mut self) -> Result<(), Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let listener = TcpListener::bind(&addr).await?;
        debug!("edge-runtime is listening on {:?}", listener.local_addr()?);

        if let Some(callback) = self.callback_tx.clone() {
            let _ = callback.send(ServerCodes::Listening).await;
        }

        loop {
            let main_worker_req_tx = self.main_worker_req_tx.clone();

            tokio::select! {
                msg = listener.accept() => {
                    match msg {
                       Ok((conn, _)) => {
                           tokio::task::spawn(async move {
                             let service = WorkerService::new(main_worker_req_tx);

                             let conn_fut = Http::new()
                                .serve_connection(conn, service);

                             if let Err(e) = conn_fut.await {
                                 // Most common cause for these errors are when the client closes the connection before
                                 // we could send a response
                                 error!("client connection error ({:?})", e);
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
