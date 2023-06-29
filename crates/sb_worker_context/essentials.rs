use crate::events::WorkerEvents;
use anyhow::Error;
use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct UserWorkerRuntimeOpts {
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub force_create: bool,
    pub key: Option<u64>,
    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub events_msg_tx: Option<mpsc::UnboundedSender<WorkerEvents>>,
}

#[derive(Debug, Clone)]
pub struct MainWorkerRuntimeOpts {
    pub worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
}

#[derive(Debug)]
pub struct EventWorkerRuntimeOpts {
    pub event_rx: mpsc::UnboundedReceiver<WorkerEvents>,
}

#[derive(Debug, Clone)]
pub enum WorkerRuntimeOpts {
    UserWorker(UserWorkerRuntimeOpts),
    MainWorker(MainWorkerRuntimeOpts),
    EventsWorker,
}

#[derive(Debug, Clone)]
pub struct WorkerContextInitOpts {
    pub service_path: PathBuf,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub conf: WorkerRuntimeOpts,
}

impl Default for UserWorkerRuntimeOpts {
    fn default() -> UserWorkerRuntimeOpts {
        UserWorkerRuntimeOpts {
            memory_limit_mb: 512,
            worker_timeout_ms: 5 * 60 * 1000,
            force_create: false,
            key: None,
            pool_msg_tx: None,
            events_msg_tx: None,
        }
    }
}

#[derive(Debug)]
pub enum UserWorkerMsgs {
    Create(
        WorkerContextInitOpts,
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
