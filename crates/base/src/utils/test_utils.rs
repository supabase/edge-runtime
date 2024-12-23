#![allow(dead_code)]

use std::{
    marker::PhantomPinned,
    path::PathBuf,
    sync::Arc,
    task::{ready, Poll},
    time::Duration,
};

use crate::{
    server::ServerFlags,
    worker::{
        self,
        pool::{SupervisorPolicy, WorkerPoolPolicy},
        TerminationToken,
    },
};

use anyhow::{bail, Context, Error};
use either::Either::Right;
use futures_util::{future::BoxFuture, Future, FutureExt};
use http_v02::{Request, Response};
use hyper_v014::Body;
use pin_project::pin_project;
use sb_event_worker::events::WorkerEventWithMetadata;

use sb_workers::context::{
    MainWorkerRuntimeOpts, Timing, UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg,
    WorkerRuntimeOpts,
};
use scopeguard::ScopeGuard;
use tokio::{
    sync::{mpsc, oneshot, Notify},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

pub struct CreateTestUserWorkerArgs(WorkerContextInitOpts, Option<SupervisorPolicy>);

impl From<WorkerContextInitOpts> for CreateTestUserWorkerArgs {
    fn from(val: WorkerContextInitOpts) -> Self {
        Self(val, None)
    }
}

impl From<(WorkerContextInitOpts, SupervisorPolicy)> for CreateTestUserWorkerArgs {
    fn from(val: (WorkerContextInitOpts, SupervisorPolicy)) -> Self {
        Self(val.0, Some(val.1))
    }
}

#[derive(Debug)]
pub struct RequestScope {
    policy: SupervisorPolicy,
    req_start_tx: mpsc::UnboundedSender<Arc<Notify>>,
    req_end_tx: mpsc::UnboundedSender<()>,
    termination_token: TerminationToken,
    conn_token: CancellationToken,
}

impl RequestScope {
    pub fn conn_token(&self) -> CancellationToken {
        self.conn_token.clone()
    }

    pub async fn start_request(self) -> RequestScopeGuard {
        if self.policy.is_per_request() {
            let fence = Arc::<Notify>::default();

            self.req_start_tx.send(fence.clone()).unwrap();
            fence.notified().await;
        }

        RequestScopeGuard {
            cancelled: false,
            req_end_tx: self.req_end_tx.clone(),
            termination_token: Some(self.termination_token.clone()),
            conn_token: self.conn_token.clone(),
            inner: None,
            _pinned: PhantomPinned,
        }
    }
}

#[pin_project]
pub struct RequestScopeGuard {
    cancelled: bool,
    req_end_tx: mpsc::UnboundedSender<()>,
    termination_token: Option<TerminationToken>,
    conn_token: CancellationToken,
    inner: Option<BoxFuture<'static, ()>>,
    _pinned: PhantomPinned,
}

impl Future for RequestScopeGuard {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();

        if !(*this.cancelled) {
            *this.cancelled = true;
            this.req_end_tx.send(()).unwrap();
            this.termination_token.as_ref().unwrap().inbound.cancel();
        }

        let inner = this.inner.get_or_insert_with(|| {
            wait_termination(this.termination_token.take().unwrap()).boxed()
        });

        ready!(inner.as_mut().poll_unpin(cx));
        this.conn_token.cancel();

        Poll::Ready(())
    }
}

pub struct TestBedBuilder {
    main_service_path: PathBuf,
    worker_pool_policy: Option<WorkerPoolPolicy>,
    worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    main_worker_init_opts: Option<WorkerContextInitOpts>,
    flags: ServerFlags,
}

impl TestBedBuilder {
    pub fn new<T>(main_service_path: T) -> Self
    where
        T: Into<PathBuf>,
    {
        Self {
            main_service_path: main_service_path.into(),
            worker_pool_policy: None,
            worker_event_sender: None,
            main_worker_init_opts: None,
            flags: ServerFlags::default(),
        }
    }

    pub fn with_worker_pool_policy(mut self, value: WorkerPoolPolicy) -> Self {
        self.worker_pool_policy = Some(value);
        self
    }

    pub fn with_worker_event_sender(
        mut self,
        value: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    ) -> Self {
        self.worker_event_sender = value;
        self
    }

    pub fn with_oneshot_policy(mut self, value: Option<u64>) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::oneshot(),
            1,
            ServerFlags {
                request_wait_timeout_ms: value,
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_per_worker_policy(mut self, value: Option<u64>) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerWorker,
            1,
            ServerFlags {
                request_wait_timeout_ms: value,
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_per_request_policy(mut self, value: Option<u64>) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerRequest { oneshot: false },
            1,
            ServerFlags {
                request_wait_timeout_ms: value,
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_main_worker_init_opts(mut self, value: WorkerContextInitOpts) -> Self {
        self.main_worker_init_opts = Some(value);
        self
    }

    pub fn with_server_flags(mut self, value: ServerFlags) -> Self {
        self.flags = value;
        self
    }

    pub async fn build(self) -> TestBed {
        let ((_, worker_pool_tx), pool_termination_token) = {
            let token = TerminationToken::new();
            (
                worker::create_user_worker_pool(
                    Arc::new(self.flags),
                    self.worker_pool_policy
                        .unwrap_or_else(test_user_worker_pool_policy),
                    self.worker_event_sender,
                    Some(token.clone()),
                    vec![],
                    None,
                    None,
                )
                .await
                .unwrap(),
                token,
            )
        };

        let main_termination_token = TerminationToken::new();
        let main_worker_surface = worker::WorkerSurfaceBuilder::new()
            .sever_flags(Right(self.flags))
            .termination_token(main_termination_token.clone())
            .init_opts(WorkerContextInitOpts {
                service_path: self.main_service_path,
                no_module_cache: false,
                import_map_path: None,
                env_vars: std::env::vars().collect(),
                timing: None,
                maybe_eszip: None,
                maybe_entrypoint: None,
                maybe_decorator: None,
                maybe_module_code: None,
                conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                    worker_pool_tx,
                    shared_metric_src: None,
                    event_worker_metric_src: None,
                }),
                static_patterns: vec![],

                maybe_jsx_import_source_config: None,
                maybe_s3_fs_config: None,
                maybe_tmp_fs_config: None,
            })
            .build()
            .await
            .unwrap();

        TestBed {
            pool_termination_token,
            main_termination_token,
            main_worker_surface,
        }
    }
}

pub struct TestBed {
    pool_termination_token: TerminationToken,
    main_termination_token: TerminationToken,
    main_worker_surface: worker::WorkerSurface,
}

impl TestBed {
    pub async fn request<F>(
        &self,
        request_factory_fn: F,
    ) -> Result<ScopeGuard<Response<Body>, impl FnOnce(Response<Body>)>, Error>
    where
        F: FnOnce(http_v02::request::Builder) -> Result<Request<Body>, Error>,
    {
        let conn_token = CancellationToken::new();
        let (res_tx, res_rx) = oneshot::channel();

        let req: Request<Body> = request_factory_fn(http_v02::request::Builder::new())?;

        let _ = self.main_worker_surface.msg_tx.send(WorkerRequestMsg {
            req,
            res_tx,
            conn_token: Some(conn_token.clone()),
        });

        let Ok(res) = res_rx.await else {
            bail!("can't send request to the main worker");
        };

        Ok(scopeguard::guard(
            res.context("request failure")?,
            move |_| {
                conn_token.cancel();
            },
        ))
    }

    pub async fn exit(self, wait_dur: Duration) {
        let wait_fut = async move {
            self.pool_termination_token.cancel_and_wait().await;
            self.main_termination_token.cancel_and_wait().await;
        };

        if timeout(wait_dur, wait_fut).await.is_err() {
            panic!("failed to exit `TestBed` in the given time");
        }
    }
}

pub async fn create_test_user_worker<Opt: Into<CreateTestUserWorkerArgs>>(
    opts: Opt,
) -> Result<(worker::WorkerSurface, RequestScope), Error> {
    let CreateTestUserWorkerArgs(mut opts, maybe_policy) = opts.into();
    let (req_start_tx, req_start_rx) = mpsc::unbounded_channel();
    let (req_end_tx, req_end_rx) = mpsc::unbounded_channel();

    let policy = maybe_policy.unwrap_or_else(SupervisorPolicy::oneshot);
    let termination_token = TerminationToken::new();

    opts.timing = Some(Timing {
        req: (req_start_rx, req_end_rx),
        ..Default::default()
    });

    let worker_surface = worker::WorkerSurfaceBuilder::new()
        .init_opts(opts)
        .policy(policy)
        .termination_token(termination_token.clone())
        .build()
        .await?;

    Ok((
        worker_surface,
        RequestScope {
            policy,
            req_start_tx,
            req_end_tx,
            termination_token,
            conn_token: CancellationToken::new(),
        },
    ))
}

pub fn test_user_worker_pool_policy() -> WorkerPoolPolicy {
    WorkerPoolPolicy::new(
        SupervisorPolicy::oneshot(),
        1,
        ServerFlags {
            request_wait_timeout_ms: Some(4 * 1000 * 3600),
            ..Default::default()
        },
    )
}

pub fn test_user_runtime_opts() -> UserWorkerRuntimeOpts {
    UserWorkerRuntimeOpts {
        worker_timeout_ms: 4 * 1000 * 3600,
        cpu_time_soft_limit_ms: 4 * 1000 * 3600,
        cpu_time_hard_limit_ms: 4 * 1000 * 3600,
        ..Default::default()
    }
}

async fn wait_termination(token: TerminationToken) {
    token.outbound.cancelled().await;
}
