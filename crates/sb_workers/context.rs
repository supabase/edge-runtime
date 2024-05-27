use anyhow::{anyhow, Error};
use deno_config::JsxImportSourceConfig;
use deno_core::FastString;
use enum_as_inner::EnumAsInner;
use event_worker::events::{UncaughtExceptionEvent, WorkerEventWithMetadata};
use hyper::{Body, Request, Response};
use sb_core::util::sync::AtomicFlag;
use sb_core::{MetricSource, SharedMetricSource};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::{mpsc, oneshot, Mutex, Notify, OwnedSemaphorePermit};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use sb_graph::{DecoratorType, EszipPayloadKind};

#[derive(Debug, Clone)]
pub enum WorkerExitStatus {
    Normal,
    WithUncaughtException(UncaughtExceptionEvent),
}

impl Default for WorkerExitStatus {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkerExit(Arc<Mutex<WorkerExitStatus>>);

impl WorkerExit {
    pub async fn error(&self) -> Option<anyhow::Error> {
        match &*self.0.lock().await {
            WorkerExitStatus::Normal => None,
            WorkerExitStatus::WithUncaughtException(UncaughtExceptionEvent {
                exception, ..
            }) => Some(anyhow!("{exception}")),
        }
    }

    pub async fn set(&self, exit_status: WorkerExitStatus) {
        *self.0.lock().await = exit_status;
    }
}

#[derive(Debug, Clone)]
pub struct UserWorkerRuntimeOpts {
    pub service_path: Option<String>,
    pub key: Option<Uuid>,

    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub events_msg_tx: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    pub cancel: Option<CancellationToken>,

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
    pub timing_tx_pair: (
        mpsc::UnboundedSender<Arc<Notify>>,
        mpsc::UnboundedSender<()>,
    ),
    pub service_path: String,
    pub permit: Option<Arc<OwnedSemaphorePermit>>,
    pub cancel: CancellationToken,
    pub status: TimingStatus,
    pub exit: WorkerExit,
}

#[derive(Debug, Clone)]
pub struct MainWorkerRuntimeOpts {
    pub worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    pub shared_metric_src: Option<SharedMetricSource>,
    pub event_worker_metric_src: Option<MetricSource>,
}

#[derive(Debug, Clone)]
pub struct EventWorkerRuntimeOpts {}

#[derive(Debug, Clone, EnumAsInner)]
pub enum WorkerRuntimeOpts {
    UserWorker(UserWorkerRuntimeOpts),
    MainWorker(MainWorkerRuntimeOpts),
    EventsWorker(EventWorkerRuntimeOpts),
}

impl WorkerRuntimeOpts {
    pub fn to_worker_kind(&self) -> WorkerKind {
        match self {
            Self::UserWorker(_) => WorkerKind::UserWorker,
            Self::MainWorker(_) => WorkerKind::MainWorker,
            Self::EventsWorker(_) => WorkerKind::EventsWorker,
        }
    }
}

#[derive(Debug, Clone, Copy, EnumAsInner, PartialEq, Eq)]
pub enum WorkerKind {
    UserWorker,
    MainWorker,
    EventsWorker,
}

impl std::fmt::Display for WorkerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerKind::UserWorker => write!(f, "user"),
            WorkerKind::MainWorker => write!(f, "main"),
            WorkerKind::EventsWorker => write!(f, "event"),
        }
    }
}

impl From<&WorkerRuntimeOpts> for WorkerKind {
    fn from(value: &WorkerRuntimeOpts) -> Self {
        value.to_worker_kind()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TimingStatus {
    pub demand: Arc<AtomicUsize>,
    pub is_retired: Arc<AtomicFlag>,
}

#[derive(Debug)]
pub struct Timing {
    pub status: TimingStatus,
    pub req: (
        mpsc::UnboundedReceiver<Arc<Notify>>,
        mpsc::UnboundedReceiver<()>,
    ),
}

impl Default for Timing {
    fn default() -> Self {
        let (_, dumb_start_rx) = unbounded_channel::<Arc<Notify>>();
        let (_, dumb_end_rx) = unbounded_channel::<()>();

        Self {
            status: TimingStatus::default(),
            req: (dumb_start_rx, dumb_end_rx),
        }
    }
}

#[derive(Debug)]
pub struct WorkerContextInitOpts {
    pub service_path: PathBuf,
    pub no_module_cache: bool,
    pub import_map_path: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub events_rx: Option<mpsc::UnboundedReceiver<WorkerEventWithMetadata>>,
    pub timing: Option<Timing>,
    pub conf: WorkerRuntimeOpts,
    pub maybe_eszip: Option<EszipPayloadKind>,
    pub maybe_module_code: Option<FastString>,
    pub maybe_entrypoint: Option<String>,
    pub maybe_decorator: Option<DecoratorType>,
    pub static_patterns: Vec<String>,
    pub maybe_jsx_import_source_config: Option<JsxImportSourceConfig>,
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
        Option<CancellationToken>,
    ),
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
    pub conn_token: Option<CancellationToken>,
}
