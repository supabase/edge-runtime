use anyhow::Error;
use enum_as_inner::EnumAsInner;
use event_manager::events::WorkerEventWithMetadata;
use hyper::{Body, Request, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct UserWorkerRuntimeOpts {
    pub service_path: Option<String>,
    pub key: Option<u64>, // unique worker key based on service path hash
    pub execution_id: Option<Uuid>,

    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub events_msg_tx: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,

    pub memory_limit_mb: u64,
    pub low_memory_multiplier: u64,

    pub worker_timeout_ms: u64, // wall clock limit

    pub cpu_time_threshold_ms: u64,
    pub cpu_burst_interval_ms: u64,
    pub max_cpu_bursts: u64,

    pub force_create: bool,
    pub net_access_disabled: bool,
    pub custom_module_root: Option<String>,
}

impl Default for UserWorkerRuntimeOpts {
    fn default() -> UserWorkerRuntimeOpts {
        UserWorkerRuntimeOpts {
            memory_limit_mb: 512,
            worker_timeout_ms: 5 * 60 * 1000,
            low_memory_multiplier: 5,
            max_cpu_bursts: 10,
            cpu_burst_interval_ms: 100,
            cpu_time_threshold_ms: 50,

            force_create: false,
            key: None,
            pool_msg_tx: None,
            events_msg_tx: None,
            net_access_disabled: false,
            custom_module_root: None,
            service_path: None,
            execution_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MainWorkerRuntimeOpts {
    pub worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
}

#[derive(Debug, Clone)]
pub struct EventWorkerRuntimeOpts {}

#[derive(Debug, Clone, EnumAsInner)]
pub enum WorkerRuntimeOpts {
    UserWorker(UserWorkerRuntimeOpts),
    MainWorker(MainWorkerRuntimeOpts),
    EventsWorker(EventWorkerRuntimeOpts),
}

#[derive(Debug)]
pub struct WorkerContextInitOpts {
    pub service_path: PathBuf,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub events_rx: Option<mpsc::UnboundedReceiver<WorkerEventWithMetadata>>,
    pub conf: WorkerRuntimeOpts,
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
