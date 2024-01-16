#![allow(dead_code)]

use std::{
    collections::HashMap,
    marker::PhantomPinned,
    path::PathBuf,
    sync::Arc,
    task::{ready, Poll},
};

use anyhow::{bail, Context, Error};
use base::rt_worker::{
    worker_ctx::{create_user_worker_pool, create_worker, CreateWorkerArgs, TerminationToken},
    worker_pool::{SupervisorPolicy, WorkerPoolPolicy},
};
use futures_util::{future::BoxFuture, Future, FutureExt};
use http::{Request, Response};
use hyper::Body;
use pin_project::pin_project;
use sb_core::conn_sync::ConnSync;
use sb_workers::context::{
    MainWorkerRuntimeOpts, Timing, UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg,
    WorkerRuntimeOpts,
};
use scopeguard::ScopeGuard;
use tokio::sync::{mpsc, oneshot, watch, Notify};

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

pub fn create_conn_watch() -> (
    ScopeGuard<watch::Sender<ConnSync>, impl FnOnce(watch::Sender<ConnSync>)>,
    watch::Receiver<ConnSync>,
) {
    let (conn_watch_tx, conn_watch_rx) = watch::channel(ConnSync::Want);
    let conn_watch_tx = scopeguard::guard(conn_watch_tx, |tx| tx.send(ConnSync::Recv).unwrap());

    (conn_watch_tx, conn_watch_rx)
}

#[derive(Debug)]
pub struct RequestScope {
    policy: SupervisorPolicy,
    req_start_tx: mpsc::UnboundedSender<Arc<Notify>>,
    req_end_tx: mpsc::UnboundedSender<()>,
    termination_token: TerminationToken,
    conn: (Option<watch::Sender<ConnSync>>, watch::Receiver<ConnSync>),
}

impl RequestScope {
    pub fn conn_rx(&self) -> Option<watch::Receiver<ConnSync>> {
        Some(self.conn.1.clone())
    }

    pub async fn start_request(mut self) -> RequestScopeGuard {
        if self.policy.is_per_request() {
            let fence = Arc::<Notify>::default();

            self.req_start_tx.send(fence.clone()).unwrap();
            fence.notified().await;
        }

        RequestScopeGuard {
            cancelled: false,
            req_end_tx: self.req_end_tx.clone(),
            termination_token: Some(self.termination_token.clone()),
            conn_tx: self.conn.0.take().unwrap(),
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
    conn_tx: watch::Sender<ConnSync>,
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
        this.conn_tx.send(ConnSync::Recv).unwrap();

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
            Some(request_wait_timeout_ms),
        ));

        self
    }

    pub fn with_per_worker_policy(mut self, request_wait_timeout_ms: u64) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerWorker,
            1,
            Some(request_wait_timeout_ms),
        ));

        self
    }

    pub fn with_per_request_policy(mut self, request_wait_timeout_ms: u64) -> Self {
        self.worker_pool_policy = Some(WorkerPoolPolicy::new(
            SupervisorPolicy::PerRequest { oneshot: false },
            1,
            Some(request_wait_timeout_ms),
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

    pub async fn build(self) -> TestBed {
        let (worker_pool_msg_tx, pool_termination_token) = {
            let token = TerminationToken::new();
            (
                create_user_worker_pool(
                    self.worker_pool_policy
                        .unwrap_or_else(test_user_worker_pool_policy),
                    None,
                    Some(token.clone()),
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
            maybe_module_code: None,
            conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                worker_pool_tx: worker_pool_msg_tx,
            }),
        };

        let main_termination_token = TerminationToken::new();
        let main_worker_msg_tx =
            create_worker((main_worker_init_opts, main_termination_token.clone()))
                .await
                .unwrap();

        TestBed {
            pool_termination_token,
            main_termination_token,
            main_worker_msg_tx,
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
        let (conn_tx, conn_rx) = create_conn_watch();
        let (res_tx, res_rx) = oneshot::channel();

        let req: Request<Body> = request_factory_fn()?;

        let _ = self.main_worker_msg_tx.send(WorkerRequestMsg {
            req,
            res_tx,
            conn_watch: Some(conn_rx),
        });

        let Ok(res) = res_rx.await else {
            bail!("can't send request to the main worker");
        };

        Ok(scopeguard::guard(
            res.context("request failure")?,
            move |_| drop(conn_tx),
        ))
    }

    pub async fn exit(self) {
        self.pool_termination_token.cancel_and_wait().await;
        self.main_termination_token.cancel_and_wait().await;
    }
}

pub async fn create_test_user_worker<Opt: Into<CreateTestUserWorkerArgs>>(
    opts: Opt,
) -> Result<(mpsc::UnboundedSender<WorkerRequestMsg>, RequestScope), Error> {
    let CreateTestUserWorkerArgs(mut opts, maybe_policy) = opts.into();
    let (req_start_tx, req_start_rx) = mpsc::unbounded_channel();
    let (req_end_tx, req_end_rx) = mpsc::unbounded_channel();
    let (conn_tx, conn_rx) = watch::channel(ConnSync::Want);

    let policy = maybe_policy.unwrap_or_else(SupervisorPolicy::oneshot);
    let termination_token = TerminationToken::new();

    opts.timing = Some(Timing {
        req: (req_start_rx, req_end_rx),
        ..Default::default()
    });

    Ok((
        create_worker(
            opts.with_policy(policy)
                .with_termination_token(termination_token.clone()),
        )
        .await?,
        RequestScope {
            policy,
            req_start_tx,
            req_end_tx,
            termination_token,
            conn: (Some(conn_tx), conn_rx),
        },
    ))
}

pub fn test_user_worker_pool_policy() -> WorkerPoolPolicy {
    WorkerPoolPolicy::new(SupervisorPolicy::oneshot(), 1, 4 * 1000 * 3600)
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
