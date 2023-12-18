use anyhow::Error;
use deno_core::FastString;
use enum_as_inner::EnumAsInner;
use event_worker::events::WorkerEventWithMetadata;
use hyper::{Body, Request, Response};
use std::path::PathBuf;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, oneshot, Notify, OwnedSemaphorePermit};
use uuid::Uuid;

use sb_graph::EszipPayloadKind;

#[derive(Debug, Clone)]
pub struct UserWorkerRuntimeOpts {
    pub service_path: Option<String>,
    pub key: Option<Uuid>,

    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub events_msg_tx: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    pub cancel: Option<Arc<Notify>>,

    pub memory_limit_mb: u64,
    pub low_memory_multiplier: u64,

    pub worker_timeout_ms: u64, // wall clock limit

    pub cpu_time_soft_limit_ms: u64,
    pub cpu_time_hard_limit_ms: u64,

    pub force_create: bool,
    pub net_access_disabled: bool,
    pub custom_module_root: Option<String>,
    pub allow_remote_modules: bool,
}

impl Default for UserWorkerRuntimeOpts {
    fn default() -> UserWorkerRuntimeOpts {
        UserWorkerRuntimeOpts {
            memory_limit_mb: 512,
            worker_timeout_ms: 5 * 60 * 1000,
            low_memory_multiplier: 5,
            cpu_time_soft_limit_ms: 50,
            cpu_time_hard_limit_ms: 100,

            force_create: false,
            key: None,
            pool_msg_tx: None,
            events_msg_tx: None,
            cancel: None,
            net_access_disabled: false,
            allow_remote_modules: true,
            custom_module_root: None,
            service_path: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserWorkerProfile {
    pub worker_request_msg_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    pub timing_tx_pair: (mpsc::UnboundedSender<()>, mpsc::UnboundedSender<()>),
    pub service_path: String,
    pub permit: Arc<OwnedSemaphorePermit>,
    pub cancel: Arc<Notify>,
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
    pub timing_rx_pair: Option<(mpsc::UnboundedReceiver<()>, mpsc::UnboundedReceiver<()>)>,
    pub conf: WorkerRuntimeOpts,
    pub maybe_eszip: Option<EszipPayloadKind>,
    pub maybe_module_code: Option<FastString>,
    pub maybe_entrypoint: Option<String>,
}

#[derive(Debug)]
pub enum UserWorkerMsgs {
    Create(
        WorkerContextInitOpts,
        oneshot::Sender<Result<CreateUserWorkerResult, Error>>,
    ),
    Created(Uuid, UserWorkerProfile),
    SendRequest(
        Uuid,
        Request<Body>,
        oneshot::Sender<Result<SendRequestResult, Error>>,
    ),
    Retire(Uuid),
    Idle(Uuid),
    Shutdown(Uuid),
}

pub type SendRequestResult = (Response<Body>, mpsc::UnboundedSender<()>);

#[derive(Debug)]
pub struct CreateUserWorkerResult {
    pub key: Uuid,
}

#[derive(Debug)]
pub struct WorkerRequestMsg {
    pub req: Request<Body>,
    pub res_tx: oneshot::Sender<Result<Response<Body>, hyper::Error>>,
}
