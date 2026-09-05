use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::anyhow;
use anyhow::Error;
use base_mem_check::MemCheckState;
use base_mem_check::WorkerHeapStatisticsWithServicePath;
use deno::deno_permissions::PermissionsOptions;
use deno_core::unsync::sync::AtomicFlag;
use deno_core::FastString;
use deno_facade::EszipPayloadKind;
use deno_telemetry::OtelConfig;
use enum_as_inner::EnumAsInner;
use ext_event_worker::events::UncaughtExceptionEvent;
use ext_event_worker::events::WorkerEventWithMetadata;
use ext_runtime::MetricSource;
use ext_runtime::RateLimiterOpts;
use ext_runtime::SharedMetricSource;
use fs::s3_fs::S3FsConfig;
use fs::tmp_fs::TmpFsConfig;
use hyper_v014::Body;
use hyper_v014::Request;
use hyper_v014::Response;
use sha2::Digest as _;
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub enum WorkerExitStatus {
  #[default]
  Normal,
  WithUncaughtException(UncaughtExceptionEvent),
}

#[derive(Debug, Clone, Default)]
pub struct WorkerExit(Arc<Mutex<WorkerExitStatus>>);

impl WorkerExit {
  pub async fn error(&self) -> Option<anyhow::Error> {
    match &*self.0.lock().await {
      WorkerExitStatus::Normal => None,
      WorkerExitStatus::WithUncaughtException(UncaughtExceptionEvent {
        exception,
        ..
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

  /// Wall clock limit
  pub worker_timeout_ms: u64,

  pub cpu_time_max_budget_per_task_ms: Option<u64>,
  pub cpu_time_soft_limit_ms: u64,
  pub cpu_time_hard_limit_ms: u64,

  pub beforeunload_wall_clock_pct: Option<u8>,
  pub beforeunload_cpu_pct: Option<u8>,
  pub beforeunload_memory_pct: Option<u8>,

  pub force_create: bool,
  pub allow_remote_modules: bool,
  pub custom_module_root: Option<String>,
  pub permissions: Option<PermissionsOptions>,

  pub context: Option<crate::JsonMap>,
  pub rate_limiter: RateLimiterOpts,
}

impl Default for UserWorkerRuntimeOpts {
  fn default() -> UserWorkerRuntimeOpts {
    UserWorkerRuntimeOpts {
      service_path: None,
      key: None,

      pool_msg_tx: None,
      events_msg_tx: None,
      cancel: None,

      memory_limit_mb: env!("SUPABASE_RESOURCE_LIMIT_MEM_MB").parse().unwrap(),
      low_memory_multiplier: env!("SUPABASE_RESOURCE_LIMIT_LOW_MEM_MULTIPLIER")
        .parse()
        .unwrap(),

      worker_timeout_ms: env!("SUPABASE_RESOURCE_LIMIT_TIMEOUT_MS")
        .parse()
        .unwrap(),

      cpu_time_max_budget_per_task_ms: if cfg!(debug_assertions) {
        Some(100)
      } else {
        Some(1)
      },
      cpu_time_soft_limit_ms: env!("SUPABASE_RESOURCE_LIMIT_CPU_SOFT_MS")
        .parse()
        .unwrap(),
      cpu_time_hard_limit_ms: env!("SUPABASE_RESOURCE_LIMIT_CPU_HARD_MS")
        .parse()
        .unwrap(),

      beforeunload_wall_clock_pct: None,
      beforeunload_cpu_pct: None,
      beforeunload_memory_pct: None,

      force_create: false,
      allow_remote_modules: true,
      custom_module_root: None,
      permissions: None,

      context: None,
      rate_limiter: RateLimiterOpts::Disabled,
    }
  }
}

/// Identity of the executable artifact a user worker was created from.
///
/// The pool keys warm-worker reuse by `service_path`, but the same
/// `service_path` can be handed completely different executable code across
/// `EdgeRuntime.userWorkers.create()` calls — a redeployed bundle, or a host
/// that funnels several functions through one path. Reusing an already-active
/// worker in that situation silently runs code that was never deployed for the
/// incoming request (supabase/edge-runtime#721). The pool therefore also
/// compares this identity and only reuses a worker whose artifact matches.
///
/// The digest is deterministic (content-derived, no pointer or process-random
/// input) so the same bytes always map to the same identity, and it is a full
/// SHA-256 (`[u8; 32]`, never truncated) so two different executable artifacts
/// cannot compare equal through a hash collision — worker identity is a
/// correctness boundary, not a cache hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCodeIdentity {
  /// A SHA-256 digest over the executable artifact (inline eszip bundle or
  /// inline module code) together with the service path, any explicit
  /// entrypoint override, and the effective import map path.
  Digest([u8; 32]),
  /// The artifact could not be reduced to a stable digest (e.g. a pre-parsed
  /// eszip handed straight to the pool). Such a request is never considered
  /// compatible with an existing worker, so it always gets a fresh one.
  Opaque,
}

impl WorkerCodeIdentity {
  /// Whether a worker created with `self` may serve a request carrying
  /// `incoming`. Reuse is only sound when both sides resolve to the same
  /// deterministic digest.
  pub fn can_serve(&self, incoming: &WorkerCodeIdentity) -> bool {
    matches!(
      (self, incoming),
      (Self::Digest(a), Self::Digest(b)) if a == b
    )
  }
}

#[derive(Debug, Clone)]
pub struct UserWorkerProfile {
  pub worker_request_msg_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
  pub early_drop_tx: mpsc::UnboundedSender<oneshot::Sender<bool>>,
  pub timing_tx_pair: (
    mpsc::UnboundedSender<Arc<Notify>>,
    mpsc::UnboundedSender<()>,
  ),
  pub service_path: String,
  pub code_identity: WorkerCodeIdentity,
  pub permit: Option<Arc<OwnedSemaphorePermit>>,
  pub cancel: CancellationToken,
  pub status: TimingStatus,
  pub exit: WorkerExit,
  pub mem_check: Arc<RwLock<MemCheckState>>,
}

#[derive(Debug, Clone)]
pub struct MainWorkerRuntimeOpts {
  pub worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
  pub shared_metric_src: Option<SharedMetricSource>,
  pub event_worker_metric_src: Option<MetricSource>,
  pub context: Option<crate::JsonMap>,
}

#[derive(Debug)]
pub struct EventWorkerRuntimeOpts {
  pub events_msg_rx: Option<mpsc::UnboundedReceiver<WorkerEventWithMetadata>>,
  pub event_worker_exit_deadline_sec: Option<u64>,
  pub context: Option<crate::JsonMap>,
}

#[derive(Debug, EnumAsInner)]
#[allow(clippy::large_enum_variant)] // Boxing would change the worker construction API.
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

  pub fn context(&self) -> Option<&crate::JsonMap> {
    match self {
      Self::UserWorker(user_worker_runtime_opts) => {
        user_worker_runtime_opts.context.as_ref()
      }
      Self::MainWorker(main_worker_runtime_opts) => {
        main_worker_runtime_opts.context.as_ref()
      }
      Self::EventsWorker(event_worker_runtime_opts) => {
        event_worker_runtime_opts.context.as_ref()
      }
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
  pub early_drop_rx: mpsc::UnboundedReceiver<oneshot::Sender<bool>>,
  pub status: TimingStatus,
  pub req: (
    mpsc::UnboundedReceiver<Arc<Notify>>,
    mpsc::UnboundedReceiver<()>,
  ),
}

impl Default for Timing {
  fn default() -> Self {
    let (_, dumb_early_drop_rx) = unbounded_channel();
    let (_, dumb_start_rx) = unbounded_channel::<Arc<Notify>>();
    let (_, dumb_end_rx) = unbounded_channel::<()>();

    Self {
      early_drop_rx: dumb_early_drop_rx,
      status: TimingStatus::default(),
      req: (dumb_start_rx, dumb_end_rx),
    }
  }
}

// TODO: Refactor this. Some members remove the `Default` trait bounds,
// increasing complexity.
#[derive(Debug)]
pub struct WorkerContextInitOpts {
  pub service_path: PathBuf,
  pub no_module_cache: bool,
  pub no_npm: Option<bool>,
  pub env_vars: HashMap<String, String>,
  pub conf: WorkerRuntimeOpts,
  pub static_patterns: Vec<String>,
  pub timing: Option<Timing>,
  pub maybe_eszip: Option<EszipPayloadKind>,
  pub maybe_module_code: Option<FastString>,
  pub maybe_entrypoint: Option<String>,
  pub maybe_s3_fs_config: Option<S3FsConfig>,
  pub maybe_tmp_fs_config: Option<TmpFsConfig>,
  pub maybe_otel_config: Option<OtelConfig>,
}

impl WorkerContextInitOpts {
  /// Derive the [`WorkerCodeIdentity`] for this creation request.
  ///
  /// This folds in every input that determines *which code* the worker will
  /// run: the inline eszip bundle bytes, the inline module code, the service
  /// path, any explicit entrypoint override, and the effective import map path
  /// (which steers module resolution both when the runtime builds the eszip and
  /// when it loads a pre-built one). It intentionally does not depend on runtime
  /// knobs (memory/CPU limits, env vars, timing) — those may legitimately differ
  /// between two requests that should still share a warm worker.
  ///
  /// Filesystem source read from `service_path` is *not* hashed: a plain
  /// file-backed worker keeps the pre-existing reuse semantics (a caller that
  /// wants an on-disk edit picked up must pass `force_create`, exactly as
  /// before this change). What the digest adds for that case is that a changed
  /// entrypoint or import map path now correctly forces a fresh worker.
  ///
  /// The digest is SHA-256 and is never truncated, so two different artifacts
  /// cannot be treated as compatible through a hash collision.
  pub fn code_identity(&self) -> WorkerCodeIdentity {
    let mut hasher = Sha256::new();

    // The service path is the pool's reuse key already, but include it so a
    // path-backed worker can never collide with an inline-artifact worker that
    // happens to resolve to the same key.
    hasher.update(self.service_path.to_string_lossy().as_bytes());

    if let Some(entrypoint) = self.maybe_entrypoint.as_deref() {
      hasher.update(b"\0entrypoint\0");
      hasher.update(entrypoint.as_bytes());
    }

    // The import map path is read out of the worker creation context
    // (`context.importMapPath`) in `crates/base/src/runtime`. It changes how
    // bare specifiers resolve, so the same source can produce a different
    // executable under a different import map — it must be part of identity.
    if let Some(import_map_path) = self
      .conf
      .context()
      .and_then(|it| it.get("importMapPath"))
      .and_then(|it| it.as_str())
    {
      hasher.update(b"\0import_map\0");
      hasher.update(import_map_path.as_bytes());
    }

    match self.maybe_eszip.as_ref() {
      Some(EszipPayloadKind::JsBufferKind(buf)) => {
        hasher.update(b"\0eszip\0");
        hasher.update(&buf[..]);
      }
      Some(EszipPayloadKind::VecKind(buf)) => {
        hasher.update(b"\0eszip\0");
        hasher.update(&buf[..]);
      }
      // A pre-parsed eszip does not expose its original bytes cheaply. This
      // shape is not produced for pooled user workers, but stay conservative
      // rather than risk treating two different bundles as equal.
      Some(EszipPayloadKind::Eszip(_)) => return WorkerCodeIdentity::Opaque,
      None => {}
    }

    if let Some(code) = self.maybe_module_code.as_ref() {
      hasher.update(b"\0module\0");
      hasher.update(code.as_str().as_bytes());
    }

    WorkerCodeIdentity::Digest(hasher.finalize().into())
  }
}

#[cfg(test)]
mod code_identity_tests {
  use super::*;

  #[derive(Default)]
  struct Artifact<'a> {
    eszip: Option<Vec<u8>>,
    module_code: Option<&'a str>,
    entrypoint: Option<&'a str>,
    import_map_path: Option<&'a str>,
  }

  fn opts(service_path: &str, artifact: Artifact<'_>) -> WorkerContextInitOpts {
    let Artifact {
      eszip,
      module_code,
      entrypoint,
      import_map_path,
    } = artifact;

    let context = import_map_path.map(|path| {
      let mut map = crate::JsonMap::new();
      map.insert("importMapPath".to_string(), path.into());
      map
    });

    WorkerContextInitOpts {
      service_path: std::path::PathBuf::from(service_path),
      no_module_cache: false,
      no_npm: None,
      env_vars: HashMap::new(),
      conf: WorkerRuntimeOpts::UserWorker(UserWorkerRuntimeOpts {
        context,
        ..Default::default()
      }),
      static_patterns: vec![],
      timing: None,
      maybe_eszip: eszip.map(EszipPayloadKind::VecKind),
      maybe_module_code: module_code.map(|it| it.to_string().into()),
      maybe_entrypoint: entrypoint.map(str::to_string),
      maybe_s3_fs_config: None,
      maybe_tmp_fs_config: None,
      maybe_otel_config: None,
    }
  }

  fn eszip(service_path: &str, bytes: &[u8]) -> WorkerContextInitOpts {
    opts(
      service_path,
      Artifact {
        eszip: Some(bytes.to_vec()),
        ..Default::default()
      },
    )
  }

  #[test]
  fn identity_is_deterministic_and_content_addressed() {
    // Inline eszip: identical bytes may reuse, different bytes may not.
    let a = eszip("svc", b"bundle-A");
    let a_again = eszip("svc", b"bundle-A");
    let b = eszip("svc", b"bundle-B");
    assert!(a.code_identity().can_serve(&a_again.code_identity()));
    assert!(!a.code_identity().can_serve(&b.code_identity()));

    // Inline module code behaves the same way.
    let m = opts(
      "svc",
      Artifact {
        module_code: Some("export default 1"),
        ..Default::default()
      },
    );
    let m_again = opts(
      "svc",
      Artifact {
        module_code: Some("export default 1"),
        ..Default::default()
      },
    );
    let m_changed = opts(
      "svc",
      Artifact {
        module_code: Some("export default 2"),
        ..Default::default()
      },
    );
    assert!(m.code_identity().can_serve(&m_again.code_identity()));
    assert!(!m.code_identity().can_serve(&m_changed.code_identity()));

    // Different artifact kinds for one service path never look equivalent.
    assert!(!a.code_identity().can_serve(&m.code_identity()));

    // An explicit entrypoint override is part of the executable identity.
    let e1 = opts(
      "svc",
      Artifact {
        entrypoint: Some("a.ts"),
        ..Default::default()
      },
    );
    let e2 = opts(
      "svc",
      Artifact {
        entrypoint: Some("b.ts"),
        ..Default::default()
      },
    );
    assert!(!e1.code_identity().can_serve(&e2.code_identity()));

    // A pre-parsed eszip cannot be digested, so it is never reusable.
    assert!(!WorkerCodeIdentity::Opaque.can_serve(&WorkerCodeIdentity::Opaque));
  }

  #[test]
  fn import_map_path_is_part_of_identity() {
    // `context.importMapPath` steers module resolution, so the same source
    // under a different import map is a different executable and must not
    // silently reuse a warm worker.
    let base = eszip("svc", b"bundle-A");

    let map_a = opts(
      "svc",
      Artifact {
        eszip: Some(b"bundle-A".to_vec()),
        import_map_path: Some("/etc/import_map_a.json"),
        ..Default::default()
      },
    );
    let map_a_again = opts(
      "svc",
      Artifact {
        eszip: Some(b"bundle-A".to_vec()),
        import_map_path: Some("/etc/import_map_a.json"),
        ..Default::default()
      },
    );
    let map_b = opts(
      "svc",
      Artifact {
        eszip: Some(b"bundle-A".to_vec()),
        import_map_path: Some("/etc/import_map_b.json"),
        ..Default::default()
      },
    );

    assert!(map_a
      .code_identity()
      .can_serve(&map_a_again.code_identity()));
    assert!(!map_a.code_identity().can_serve(&map_b.code_identity()));
    // Adding an import map to an otherwise identical request also changes it.
    assert!(!base.code_identity().can_serve(&map_a.code_identity()));
  }

  #[test]
  fn digest_uses_full_sha256() {
    // The strong identity is the full 32-byte SHA-256, not a truncated hash.
    let WorkerCodeIdentity::Digest(bytes) =
      eszip("svc", b"bundle-A").code_identity()
    else {
      panic!("inline eszip must produce a digest");
    };
    assert_eq!(bytes.len(), 32);

    // Known-answer: SHA-256 of the exact byte stream the hasher folds in for
    // this request (service path, then the framed eszip bytes).
    let mut expected = Sha256::new();
    expected.update(b"svc");
    expected.update(b"\0eszip\0");
    expected.update(b"bundle-A");
    let expected: [u8; 32] = expected.finalize().into();
    assert_eq!(bytes, expected);
  }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // This is a low-frequency control channel; avoid API churn.
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
  TryCleanupIdleWorkers(usize, oneshot::Sender<usize>),
  InqueryMemoryUsage(
    oneshot::Sender<HashMap<Uuid, WorkerHeapStatisticsWithServicePath>>,
  ),
}

pub type SendRequestResult = (Response<Body>, mpsc::UnboundedSender<()>);

#[derive(Debug)]
pub struct CreateUserWorkerResult {
  pub key: Uuid,
  pub reused: bool,
}

#[derive(Debug)]
pub struct WorkerRequestMsg {
  pub req: Request<Body>,
  pub res_tx: oneshot::Sender<Result<Response<Body>, hyper_v014::Error>>,
  pub conn_token: Option<CancellationToken>,
  pub idle_timed_out: Arc<AtomicBool>,
}
