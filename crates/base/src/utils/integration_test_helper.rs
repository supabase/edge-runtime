#![allow(dead_code)]

use std::{
    collections::HashMap,
    marker::PhantomPinned,
    path::PathBuf,
    sync::Arc,
    task::{ready, Poll},
    time::Duration,
};

use anyhow::{bail, Context, Error};
use base::{
    rt_worker::{
        worker_ctx::{create_user_worker_pool, create_worker, CreateWorkerArgs, TerminationToken},
        worker_pool::{SupervisorPolicy, WorkerPoolPolicy},
    },
    server::ServerFlags,
};
use futures_util::{future::BoxFuture, Future, FutureExt};
use http::{Request, Response};
use hyper::Body;
use pin_project::pin_project;

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

pub trait WorkerContextInitOptsForTesting {
    fn with_policy(self, policy: SupervisorPolicy) -> CreateWorkerArgs
    where
        Self: Sized;
}

impl WorkerContextInitOptsForTesting for WorkerContextInitOpts {
    fn with_policy(self, policy: SupervisorPolicy) -> CreateWorkerArgs {
        (self, policy).into()
    }
}

pub struct TestBedBuilder {
    main_service_path: PathBuf,
    worker_pool_policy: Option<WorkerPoolPolicy>,
    main_worker_init_opts: Option<WorkerContextInitOpts>,
    request_idle_timeout: Option<u64>,
}

impl TestBedBuilder {
    pub fn new<T>(main_service_path: T) -> Self
    where
        T: Into<PathBuf>,
    {
        Self {
            main_service_path: main_service_path.into(),
            worker_pool_policy: None,
            main_worker_init_opts: None,
            request_idle_timeout: None,
        }
    }

    pub fn with_worker_pool_policy(mut self, worker_pool_policy: WorkerPoolPolicy) -> Self {
        self.worker_pool_policy = Some(worker_pool_policy);
        self
    }

    pub fn with_oneshot_policy(mut self, request_wait_timeout_ms: u64) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::oneshot(),
            1,
            ServerFlags {
                request_wait_timeout_ms: Some(request_wait_timeout_ms),
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_per_worker_policy(mut self, request_wait_timeout_ms: u64) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerWorker,
            1,
            ServerFlags {
                request_wait_timeout_ms: Some(request_wait_timeout_ms),
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_per_request_policy(mut self, request_wait_timeout_ms: u64) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerRequest { oneshot: false },
            1,
            ServerFlags {
                request_wait_timeout_ms: Some(request_wait_timeout_ms),
                ..Default::default()
            },
        ));

        self
    }

    pub fn with_main_worker_init_opts(
        mut self,
        main_worker_init_opts: WorkerContextInitOpts,
    ) -> Self {
        self.main_worker_init_opts = Some(main_worker_init_opts);
        self
    }

    pub fn with_request_idle_timeout(mut self, request_idle_timeout: u64) -> Self {
        self.request_idle_timeout = Some(request_idle_timeout);
        self
    }

    pub async fn build(self) -> TestBed {
        let ((_, worker_pool_tx), pool_termination_token) = {
            let token = TerminationToken::new();
            (
                create_user_worker_pool(
                    self.worker_pool_policy
                        .unwrap_or_else(test_user_worker_pool_policy),
                    None,
                    Some(token.clone()),
                    vec![],
                    None,
                    None,
                    self.request_idle_timeout,
                )
                .await
                .unwrap(),
                token,
            )
        };

        let main_worker_init_opts = WorkerContextInitOpts {
            service_path: self.main_service_path,
            no_module_cache: false,
            import_map_path: None,
            env_vars: HashMap::new(),
            events_rx: None,
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
        };

        let main_termination_token = TerminationToken::new();
        let ctx = create_worker(
            (main_worker_init_opts, main_termination_token.clone()),
            None,
            None,
        )
        .await
        .unwrap();

        TestBed {
            pool_termination_token,
            main_termination_token,
            main_worker_msg_tx: ctx.msg_tx,
        }
    }
}

pub struct TestBed {
    pool_termination_token: TerminationToken,
    main_termination_token: TerminationToken,
    main_worker_msg_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
}

impl TestBed {
    pub async fn request<F>(
        &self,
        request_factory_fn: F,
    ) -> Result<ScopeGuard<Response<Body>, impl FnOnce(Response<Body>)>, Error>
    where
        F: FnOnce() -> Result<Request<Body>, Error>,
    {
        let conn_token = CancellationToken::new();
        let (res_tx, res_rx) = oneshot::channel();

        let req: Request<Body> = request_factory_fn()?;

        let _ = self.main_worker_msg_tx.send(WorkerRequestMsg {
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
) -> Result<(mpsc::UnboundedSender<WorkerRequestMsg>, RequestScope), Error> {
    let CreateTestUserWorkerArgs(mut opts, maybe_policy) = opts.into();
    let (req_start_tx, req_start_rx) = mpsc::unbounded_channel();
    let (req_end_tx, req_end_rx) = mpsc::unbounded_channel();

    let policy = maybe_policy.unwrap_or_else(SupervisorPolicy::oneshot);
    let termination_token = TerminationToken::new();

    opts.timing = Some(Timing {
        req: (req_start_rx, req_end_rx),
        ..Default::default()
    });

    Ok({
        let ctx = create_worker(
            opts.with_policy(policy)
                .with_termination_token(termination_token.clone()),
            None,
            None,
        )
        .await?;

        (
            ctx.msg_tx,
            RequestScope {
                policy,
                req_start_tx,
                req_end_tx,
                termination_token,
                conn_token: CancellationToken::new(),
            },
        )
    })
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
