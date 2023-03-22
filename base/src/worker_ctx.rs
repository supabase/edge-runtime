use crate::js_worker::JsWorker;

use anyhow::Error;
use hyper::{Body, Request, Response};
use log::{debug, error};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::sync::RwLock;

pub struct WorkerContext {
    handle: thread::JoinHandle<Result<(), Error>>,
    request_sender: hyper::client::conn::SendRequest<Body>,
}

pub struct CreateWorkerOptions {
    pub main_worker: bool,
    pub service_path: PathBuf,
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
}

impl WorkerContext {
    pub async fn new(options: CreateWorkerOptions) -> Result<Self, Error> {
        let service_path = options.service_path;
        let memory_limit_mb = options.memory_limit_mb;
        let worker_timeout_ms = options.worker_timeout_ms;
        let no_module_cache = options.no_module_cache;
        let import_map_path = options.import_map_path;
        let env_vars = options.env_vars;

        // create a unix socket pair
        let (sender_stream, recv_stream) = UnixStream::pair()?;

        let handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
            let worker = JsWorker::new(
                service_path.clone(),
                memory_limit_mb,
                worker_timeout_ms,
                no_module_cache,
                import_map_path,
                env_vars,
            )?;

            worker.accept(recv_stream)?;

            // start the worker
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            worker.run(shutdown_tx)?;

            // wait for shutdown signal
            let _ = shutdown_rx.blocking_recv();

            debug!("js worker for {:?} stopped", service_path);

            Ok(())
        });

        // send the HTTP request to the worker over Unix stream
        let (request_sender, connection) = hyper::client::conn::handshake(sender_stream).await?;

        // spawn a task to poll the connection and drive the HTTP state
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("Error in connection: {}", e);
            }
        });

        Ok(Self {
            handle,
            request_sender,
        })
    }

    pub async fn send_request(
        &mut self,
        req: Request<Body>,
    ) -> Result<Response<Body>, hyper::Error> {
        self.request_sender.send_request(req).await
    }
}

pub struct WorkerPool {
    pub main_worker: Arc<RwLock<WorkerContext>>,
    user_workers: HashMap<String, Arc<RwLock<WorkerContext>>>,
}

impl WorkerPool {
    pub async fn new(
        main_path: String,
        import_map_path: Option<String>,
        no_module_cache: bool,
    ) -> Result<Self, Error> {
        let main_path = Path::new(&main_path);
        let main_worker_ctx = WorkerContext::new(CreateWorkerOptions {
            main_worker: true,
            service_path: main_path.to_path_buf(),
            memory_limit_mb: 150,
            worker_timeout_ms: 60 * 60 * 1000,
            import_map_path,
            env_vars: HashMap::new(),
            no_module_cache,
        })
        .await?;
        let main_worker = Arc::new(RwLock::new(main_worker_ctx));

        let user_workers: HashMap<String, Arc<RwLock<WorkerContext>>> = HashMap::new();

        Ok(Self {
            main_worker,
            user_workers,
        })
    }

    pub fn add_user_worker() {}

    pub fn get_user_worker() {}

    pub fn stats() {}
}
