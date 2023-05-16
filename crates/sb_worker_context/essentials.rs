use anyhow::Error;
use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct EdgeUserRuntimeOpts {
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub force_create: bool,
    pub key: Option<u64>,
    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
}

#[derive(Debug, Clone)]
pub struct EdgeMainRuntimeOpts {
    pub worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
}

#[derive(Debug, Clone)]
pub enum EdgeContextOpts {
    UserWorker(EdgeUserRuntimeOpts),
    MainWorker(EdgeMainRuntimeOpts),
}

#[derive(Debug, Clone)]
pub struct EdgeContextInitOpts {
    pub service_path: PathBuf,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub conf: EdgeContextOpts,
}

impl Default for EdgeUserRuntimeOpts {
    fn default() -> EdgeUserRuntimeOpts {
        EdgeUserRuntimeOpts {
            memory_limit_mb: 150,
            worker_timeout_ms: 60000,
            force_create: false,
            key: None,
            pool_msg_tx: None,
        }
    }
}

#[derive(Debug)]
pub enum UserWorkerMsgs {
    Create(
        EdgeContextInitOpts,
        oneshot::Sender<Result<CreateUserWorkerResult, Error>>,
    ),
    SendRequest(
        u64,
        Request<Body>,
        oneshot::Sender<Result<Response<Body>, Error>>,
    ),
    Shutdown(u64),
}

#[derive(Debug)]
pub struct CreateUserWorkerResult {
    pub key: u64,
}
