use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::oneshot;

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
    Create(UserWorkerOptions, oneshot::Sender<CreateUserWorkerResult>),
    SendRequest(String, Request<Body>, oneshot::Sender<Response<Body>>),
}

#[derive(Debug)]
pub struct CreateUserWorkerResult {
    pub key: String,
}
