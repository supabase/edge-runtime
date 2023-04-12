use crate::edge_runtime::{
    EdgeMainRuntimeOpts, EdgeRuntime, EdgeRuntimeOpts, EdgeRuntimeTypes, EdgeUserRuntimeOpts,
};
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

pub struct MainWorkerOpts {
    pub user_worker_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
}

pub struct UserWorkerOpts {
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub id: String,
}

pub enum WorkerContextOpts {
    UserWorker(UserWorkerOpts),
    MainWorker(MainWorkerOpts),
}

pub struct WorkerContextInitOpts {
    pub service_path: PathBuf,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub conf: WorkerContextOpts,
}

impl WorkerContext {
    pub async fn new(conf: WorkerContextInitOpts) -> Result<Self, Error> {
        let service_path = conf.service_path;

        if !service_path.exists() {
            bail!("main function does not exist {:?}", &service_path)
        }

        // create a unix socket pair
        let (sender_stream, recv_stream) = UnixStream::pair()?;

        let handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();

            let handle: Result<(), Error> = local.block_on(&runtime, async {
                let worker = EdgeRuntime::new(EdgeRuntimeOpts {
                    service_path: service_path.clone(),
                    no_module_cache: conf.no_module_cache,
                    import_map_path: conf.import_map_path,
                    env_vars: conf.env_vars,
                    conf: match &conf.conf {
                        WorkerContextOpts::UserWorker(user_conf) => {
                            EdgeRuntimeTypes::User(EdgeUserRuntimeOpts {
                                memory_limit_mb: user_conf.memory_limit_mb,
                                worker_timeout_ms: user_conf.worker_timeout_ms,
                                id: String::from(&user_conf.id),
                            })
                        }
                        WorkerContextOpts::MainWorker(main_conf) => {
                            EdgeRuntimeTypes::Main(EdgeMainRuntimeOpts {
                                worker_pool_tx: main_conf.user_worker_msgs_tx.clone(),
                            })
                        }
                    },
                })?;

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

        let main_worker_ctx = WorkerContext::new(WorkerContextInitOpts {
            service_path: main_path.to_path_buf(),
            import_map_path,
            no_module_cache,
            conf: WorkerContextOpts::MainWorker(MainWorkerOpts {
                user_worker_msgs_tx,
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
                        let user_worker_ctx = WorkerContext::new(WorkerContextInitOpts {
                            service_path: worker_options.service_path,
                            no_module_cache: worker_options.no_module_cache,
                            import_map_path: worker_options.import_map_path,
                            env_vars: worker_options.env_vars,
                            conf: WorkerContextOpts::UserWorker(UserWorkerOpts {
                                memory_limit_mb: worker_options.memory_limit_mb,
                                worker_timeout_ms: worker_options.worker_timeout_ms,
                                id: "".to_string(),
                            }),
                        })
                        .await;
                        if !user_worker_ctx.is_err() {
                            user_workers.insert(
                                key.clone(),
                                Arc::new(RwLock::new(user_worker_ctx.unwrap())),
                            );

                            tx.send(Ok(CreateUserWorkerResult { key }));
                        } else {
                            tx.send(Err(user_worker_ctx.unwrap_err()));
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

                        tx.send(res);
                    }
                }
            }
        });

        Ok(Self { main_worker })
    }
}
