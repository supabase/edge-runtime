use crate::js_worker::{MainWorker, UserWorker};

use anyhow::{bail, Error};
use hyper::{Body, Request, Response};
use log::error;
use sb_worker_context::essentials::{CreateUserWorkerResult, UserWorkerMsgs, UserWorkerOptions};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug)]
pub struct WorkerContext {
    request_sender: hyper::client::conn::SendRequest<Body>,
}

pub struct MainWorkerOptions {
    pub service_path: PathBuf,
    pub user_worker_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
}

impl WorkerContext {
    pub async fn new_main_worker(options: MainWorkerOptions) -> Result<Self, Error> {
        let service_path = options.service_path;
        let no_module_cache = options.no_module_cache;
        let import_map_path = options.import_map_path;
        let user_worker_msgs_tx = options.user_worker_msgs_tx;

        if !service_path.exists() {
            bail!("main function does not exist {:?}", &service_path)
        }

        // create a unix socket pair
        let (sender_stream, recv_stream) = UnixStream::pair()?;

        let _handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();

            let _handle: Result<(), Error> = local.block_on(&runtime, async {
                let worker = MainWorker::new(
                    service_path.clone(),
                    no_module_cache,
                    import_map_path,
                    user_worker_msgs_tx.clone(),
                )?;

                // start the worker
                let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
                worker.run(recv_stream, shutdown_tx).await?;

                // wait for shutdown signal
                let _ = shutdown_rx.blocking_recv();

                Ok(())
            });

            Ok(())
        });

        // send the HTTP request to the worker over Unix stream
        let (request_sender, connection) = hyper::client::conn::handshake(sender_stream).await?;

        // spawn a task to poll the connection and drive the HTTP state
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("Error in main worker connection: {}", e);
            }
        });

        Ok(Self { request_sender })
    }

    pub async fn new_user_worker(options: UserWorkerOptions) -> Result<Self, Error> {
        let service_path = options.service_path;
        let memory_limit_mb = options.memory_limit_mb;
        let worker_timeout_ms = options.worker_timeout_ms;
        let no_module_cache = options.no_module_cache;
        let import_map_path = options.import_map_path;
        let env_vars = options.env_vars;

        if !service_path.exists() {
            bail!("user function does not exist {:?}", &service_path)
        }

        // create a unix socket pair
        let (sender_stream, recv_stream) = UnixStream::pair()?;

        let _handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();

            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

            let _handle: Result<(), Error> = local.block_on(&runtime, async {
                let worker = UserWorker::new(
                    service_path.clone(),
                    memory_limit_mb,
                    worker_timeout_ms,
                    no_module_cache,
                    import_map_path,
                    env_vars,
                )?;

                // start the worker
                worker.run(recv_stream, shutdown_tx).await?;

                Ok(())
            });

            // wait for shutdown signal
            let _ = shutdown_rx.blocking_recv();

            Ok(())
        });

        // send the HTTP request to the worker over Unix stream
        let (request_sender, connection) = hyper::client::conn::handshake(sender_stream).await?;

        // spawn a task to poll the connection and drive the HTTP state
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("Error in user worker connection: {}", e);
            }
        });

        Ok(Self { request_sender })
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
}

impl WorkerPool {
    pub async fn new(
        main_path: String,
        import_map_path: Option<String>,
        no_module_cache: bool,
    ) -> Result<Self, Error> {
        let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
            mpsc::unbounded_channel::<UserWorkerMsgs>();

        let main_path = Path::new(&main_path);
        let main_worker_ctx = WorkerContext::new_main_worker(MainWorkerOptions {
            service_path: main_path.to_path_buf(),
            import_map_path,
            no_module_cache,
            user_worker_msgs_tx,
        })
        .await?;
        let main_worker = Arc::new(RwLock::new(main_worker_ctx));
        tokio::spawn(async move {
            let mut user_workers: HashMap<Uuid, Arc<RwLock<WorkerContext>>> = HashMap::new();

            loop {
                match user_worker_msgs_rx.recv().await {
                    None => break,
                    Some(UserWorkerMsgs::Create(worker_options, tx)) => {
                        let key = Uuid::new_v4();
                        let user_worker_ctx = WorkerContext::new_user_worker(worker_options).await;
                        let _ = match user_worker_ctx {
                            Ok(ctx) => {
                                user_workers.insert(key, Arc::new(RwLock::new(ctx)));
                                tx.send(Ok(CreateUserWorkerResult { key }))
                            }
                            Err(e) => tx.send(Err(e)),
                        };
                    }
                    Some(UserWorkerMsgs::SendRequest(key, req, tx)) => {
                        // TODO: handle errors
                        let worker = user_workers.get(&key).unwrap();
                        let mut worker = worker.write().await;
                        // TODO: Json format
                        // TODO: Ability to attach hook
                        let res = worker.send_request(req).await.unwrap_or_else(|_e| Response::builder().status(408).body(Body::from(deno_core::serde_json::json!({
                            "msg": "Request could not be processed by the server because it timed out or an error was thrown."
                        }).to_string())).unwrap());

                        let _ = tx.send(res);
                    }
                }
            }
        });

        Ok(Self { main_worker })
    }
}
