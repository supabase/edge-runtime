use crate::edge_runtime::EdgeRuntime;
use anyhow::{bail, Error};
use hyper::{Body, Request, Response};
use log::error;
use sb_worker_context::essentials::{
    CreateUserWorkerResult, EdgeContextInitOpts, EdgeContextOpts, EdgeMainRuntimeOpts,
    UserWorkerMsgs, UserWorkerOptions,
};
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

impl WorkerContext {
    pub async fn new(conf: EdgeContextInitOpts) -> Result<Self, Error> {
        let service_path = conf.service_path.clone();

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
                let worker = EdgeRuntime::new(conf)?;

                // start the worker
                let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
                worker.run(recv_stream, shutdown_tx).await?;

                // wait for shutdown signal
                let _ = shutdown_rx.await;

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

        let main_worker_ctx = WorkerContext::new(EdgeContextInitOpts {
            service_path: main_path.to_path_buf(),
            import_map_path,
            no_module_cache,
            conf: EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
                worker_pool_tx: user_worker_msgs_tx,
            }),
            env_vars: std::env::vars().collect(),
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
                        let user_worker_ctx = WorkerContext::new(worker_options).await;
                        if !user_worker_ctx.is_err() {
                            user_workers.insert(
                                key.clone(),
                                Arc::new(RwLock::new(user_worker_ctx.unwrap())),
                            );

                            let _ = tx.send(Ok(CreateUserWorkerResult { key }));
                        } else {
                            let _ = tx.send(Err(user_worker_ctx.unwrap_err()));
                        }
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
