#![allow(dead_code)]

use std::{
    marker::PhantomPinned,
    sync::Arc,
    task::{ready, Poll},
};

use anyhow::Error;
use base::rt_worker::{
    worker_ctx::{create_worker, CreateWorkerArgs, TerminationToken},
    worker_pool::{SupervisorPolicy, WorkerPoolPolicy},
};
use futures_util::{future::BoxFuture, Future, FutureExt};
use pin_project::pin_project;
use sb_core::conn_sync::ConnSync;
use sb_workers::context::{Timing, UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRequestMsg};
use scopeguard::ScopeGuard;
use tokio::sync::{mpsc, watch, Notify};

pub fn create_conn_watch() -> (
    ScopeGuard<watch::Sender<ConnSync>, impl FnOnce(watch::Sender<ConnSync>)>,
    watch::Receiver<ConnSync>,
) {
    let (conn_watch_tx, conn_watch_rx) = watch::channel(ConnSync::Want);
    let conn_watch_tx = scopeguard::guard(conn_watch_tx, |tx| tx.send(ConnSync::Recv).unwrap());

    (conn_watch_tx, conn_watch_rx)
}

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
