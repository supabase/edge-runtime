use anyhow::Error;
use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EdgeUserRuntimeOpts {
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub id: String,
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
            id: String::from("Unknown"),
        }
    }
}

#[derive(Debug)]
pub struct UserWorkerOptions {
    pub service_path: PathBuf,
    pub memory_limit_mb: u64,
    pub worker_timeout_ms: u64,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
}

#[derive(Debug)]
pub enum UserWorkerMsgs {
    Create(
        EdgeContextInitOpts,
        oneshot::Sender<Result<CreateUserWorkerResult, Error>>,
    ),
    SendRequest(Uuid, Request<Body>, oneshot::Sender<Response<Body>>),
}

#[derive(Debug)]
pub struct CreateUserWorkerResult {
    pub key: Uuid,
}
