use crate::deno_runtime::DenoRuntime;
use crate::inspector_server::Inspector;
use crate::rt_worker::supervisor;
use crate::rt_worker::utils::{get_event_metadata, parse_worker_conf};
use crate::rt_worker::worker_ctx::create_supervisor;
use crate::utils::send_event_if_event_worker_available;
use anyhow::{anyhow, Error};
use event_worker::events::{
    EventMetadata, ShutdownEvent, ShutdownReason, UncaughtExceptionEvent, WorkerEventWithMetadata,
    WorkerEvents, WorkerMemoryUsed,
};
use futures_util::FutureExt;
use log::{debug, error};
use sb_core::conn_sync::ConnSync;
use sb_core::{MetricSource, RuntimeMetricSource, WorkerMetricSource};
use sb_workers::context::{UserWorkerMsgs, WorkerContextInitOpts};
use std::any::Any;
use std::future::{pending, Future};
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::sync::{oneshot, watch, Notify};
use tokio::time::Instant;
use uuid::Uuid;

use super::rt;
use super::supervisor::CPUUsageMetrics;
use super::worker_ctx::TerminationToken;
use super::worker_pool::SupervisorPolicy;

#[derive(Clone)]
pub struct Worker {
    pub worker_boot_start_time: Instant,
    pub events_msg_tx: Option<UnboundedSender<WorkerEventWithMetadata>>,
    pub pool_msg_tx: Option<UnboundedSender<UserWorkerMsgs>>,
    pub cancel: Option<Arc<Notify>>,
    pub event_metadata: EventMetadata,
    pub worker_key: Option<Uuid>,
    pub inspector: Option<Inspector>,
    pub supervisor_policy: SupervisorPolicy,
    pub worker_name: String,
}

pub type HandleCreationType<'r> = Pin<Box<dyn Future<Output = Result<WorkerEvents, Error>> + 'r>>;
pub type UnixStreamEntry = (UnixStream, Option<watch::Receiver<ConnSync>>);

pub trait WorkerHandler: Send {
    fn handle_error(&self, error: Error) -> Result<WorkerEvents, Error>;
    fn handle_creation<'r>(
        &self,
        created_rt: &'r mut DenoRuntime,
        unix_stream_rx: UnboundedReceiver<UnixStreamEntry>,
        termination_event_rx: Receiver<WorkerEvents>,
        maybe_cpu_metrics_tx: Option<UnboundedSender<CPUUsageMetrics>>,
        name: Option<String>,
    ) -> HandleCreationType<'r>;
    fn as_any(&self) -> &dyn Any;
}

impl Worker {
    pub fn new(init_opts: &WorkerContextInitOpts) -> Result<Self, Error> {
        let (worker_key, pool_msg_tx, events_msg_tx, cancel, worker_name) =
            parse_worker_conf(&init_opts.conf);

        let event_metadata = get_event_metadata(&init_opts.conf);
        let worker_boot_start_time = Instant::now();

        Ok(Self {
            worker_boot_start_time,
            events_msg_tx,
            pool_msg_tx,
            cancel,
            event_metadata,
            worker_key,
            supervisor_policy: SupervisorPolicy::default(),
            inspector: None,
            worker_name,
        })
    }

    pub fn set_inspector(&mut self, inspector: Inspector) {
        self.inspector = Some(inspector);
    }

    pub fn set_supervisor_policy(&mut self, supervisor_policy: Option<SupervisorPolicy>) {
        self.supervisor_policy = supervisor_policy.unwrap_or_default();
    }

    pub fn start(
        &self,
        mut opts: WorkerContextInitOpts,
        unix_stream_pair: (
            UnboundedSender<UnixStreamEntry>,
            UnboundedReceiver<UnixStreamEntry>,
        ),
        booter_signal: Sender<Result<MetricSource, Error>>,
        termination_token: Option<TerminationToken>,
        inspector: Option<Inspector>,
    ) {
        let worker_name = self.worker_name.clone();
        let worker_key = self.worker_key;
        let event_metadata = self.event_metadata.clone();
        let supervisor_policy = self.supervisor_policy;

        let (unix_stream_tx, unix_stream_rx) = unix_stream_pair;
        let events_msg_tx = self.events_msg_tx.clone();
        let pool_msg_tx = self.pool_msg_tx.clone();

        let method_cloner = self.clone();
        let timing = opts.timing.take();
        let worker_kind = opts.conf.to_worker_kind();
        let maybe_main_worker_opts = opts.conf.as_main_worker().cloned();

        let cancel = self.cancel.clone();
        let rt = if worker_kind.is_user_worker() {
            &rt::USER_WORKER_RT
        } else {
            &rt::PRIMARY_WORKER_RT
        };

        let _worker_handle = rt.spawn_pinned(move || {
            tokio::task::spawn_local(async move {
                let (maybe_cpu_usage_metrics_tx, maybe_cpu_usage_metrics_rx) = worker_kind
                    .is_user_worker()
                    .then(unbounded_channel::<CPUUsageMetrics>)
                    .unzip();

                let result = match DenoRuntime::new(opts, inspector).await {
                    Ok(mut new_runtime) => {
                        let metric_src = {
                            let js_runtime = &mut new_runtime.js_runtime;
                            let metric_src = WorkerMetricSource::from_js_runtime(js_runtime);

                            if worker_kind.is_main_worker() {
                                let opts = maybe_main_worker_opts.unwrap();
                                let state = js_runtime.op_state();
                                let mut state_mut = state.borrow_mut();
                                let metric_src = RuntimeMetricSource::new(
                                    metric_src.clone(),
                                    opts.event_worker_metric_src
                                        .and_then(|it| it.into_worker().ok()),
                                    opts.shared_metric_src,
                                );

                                state_mut.put(metric_src.clone());
                                MetricSource::Runtime(metric_src)
                            } else {
                                MetricSource::Worker(metric_src)
                            }
                        };

                        let _ = booter_signal.send(Ok(metric_src));

                        // CPU TIMER
                        let (termination_event_tx, termination_event_rx) =
                            oneshot::channel::<WorkerEvents>();

                        let _cpu_timer;
                        let mut supervise_cancel_token = None;

                        // TODO: Allow customization of supervisor
                        let termination_fut = if worker_kind.is_user_worker() {
                            // cputimer is returned from supervisor and assigned here to keep it in scope.
                            let Ok((maybe_timer, cancel_token)) = create_supervisor(
                                worker_key.unwrap_or(Uuid::nil()),
                                &mut new_runtime,
                                supervisor_policy,
                                termination_event_tx,
                                pool_msg_tx.clone(),
                                maybe_cpu_usage_metrics_rx,
                                cancel,
                                timing,
                                termination_token.clone(),
                            ) else {
                                return;
                            };

                            _cpu_timer = maybe_timer;
                            supervise_cancel_token = Some(cancel_token);

                            pending().boxed()
                        } else if let Some(token) = termination_token.clone() {
                            let is_terminated = new_runtime.is_terminated.clone();
                            let is_termination_requested =
                                new_runtime.is_termination_requested.clone();

                            let (waker, thread_safe_handle) = {
                                let js_runtime = &mut new_runtime.js_runtime;
                                (
                                    js_runtime.op_state().borrow().waker.clone(),
                                    js_runtime.v8_isolate().thread_safe_handle(),
                                )
                            };

                            rt::SUPERVISOR_RT
                                .spawn(async move {
                                    token.inbound.cancelled().await;
                                    is_termination_requested.raise();

                                    let data_ptr_mut =
                                        Box::into_raw(Box::new(supervisor::IsolateInterruptData {
                                            should_terminate: true,
                                            isolate_memory_usage_tx: None,
                                        }));

                                    if !thread_safe_handle.request_interrupt(
                                        supervisor::handle_interrupt,
                                        data_ptr_mut as *mut std::ffi::c_void,
                                    ) {
                                        drop(unsafe { Box::from_raw(data_ptr_mut) });
                                    }

                                    while !is_terminated.is_raised() {
                                        waker.wake();
                                        tokio::task::yield_now().await;
                                    }

                                    let _ = termination_event_tx.send(WorkerEvents::Shutdown(
                                        ShutdownEvent {
                                            reason: ShutdownReason::TerminationRequested,
                                            cpu_time_used: 0,
                                            memory_used: WorkerMemoryUsed {
                                                total: 0,
                                                heap: 0,
                                                external: 0,
                                            },
                                        },
                                    ));
                                })
                                .boxed()
                        } else {
                            pending().boxed()
                        };

                        let result = unsafe {
                            let mut runtime = scopeguard::guard(new_runtime, |mut runtime| {
                                runtime.js_runtime.v8_isolate().enter();
                            });

                            runtime.js_runtime.v8_isolate().exit();

                            let result = method_cloner
                                .handle_creation(
                                    &mut runtime,
                                    unix_stream_rx,
                                    termination_event_rx,
                                    maybe_cpu_usage_metrics_tx,
                                    Some(worker_name),
                                )
                                .await;

                            let found_unexpected_error = result.is_err()
                                || matches!(
                                    result.as_ref(),
                                    Ok(WorkerEvents::UncaughtException(_))
                                );

                            if found_unexpected_error {
                                if let Some(token) = supervise_cancel_token {
                                    token.cancel();
                                }
                            }

                            result
                        };

                        if let Some(token) = termination_token.as_ref() {
                            if !worker_kind.is_user_worker() {
                                let _ = termination_fut.await;
                                token.outbound.cancel();
                            }
                        }

                        result
                    }

                    Err(err) => {
                        let _ = booter_signal.send(Err(anyhow!("worker boot error")));
                        method_cloner.handle_error(err)
                    }
                };

                drop(unix_stream_tx);

                match result {
                    Ok(event) => {
                        match event {
                            WorkerEvents::Shutdown(ShutdownEvent { cpu_time_used, .. })
                            | WorkerEvents::UncaughtException(UncaughtExceptionEvent {
                                cpu_time_used,
                                ..
                            }) => {
                                debug!("CPU time used: {:?}ms", cpu_time_used / 1_000_000);
                            }

                            _ => {}
                        };

                        send_event_if_event_worker_available(
                            events_msg_tx.clone(),
                            event,
                            event_metadata.clone(),
                        );
                    }
                    Err(err) => error!("unexpected worker error {}", err),
                };

                worker_key.and_then(|worker_key_unwrapped| {
                    pool_msg_tx.map(|tx| {
                        if let Err(err) = tx.send(UserWorkerMsgs::Shutdown(worker_key_unwrapped)) {
                            error!(
                                "failed to send the shutdown signal to user worker pool: {:?}",
                                err
                            );
                        }
                    })
                });
            })
        });
    }
}
