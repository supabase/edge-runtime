use std::{future::pending, sync::Arc, time::Duration};

use anyhow::anyhow;
use base_mem_check::MemCheckState;
use cpu_timer::{CPUAlarmVal, CPUTimer};
use deno_core::{v8, InspectorSessionProxy, LocalInspectorSession};
use enum_as_inner::EnumAsInner;
use futures_util::{pin_mut, task::AtomicWaker};
use sb_core::PromiseMetrics;
use sb_event_worker::events::{ShutdownEvent, WorkerEvents, WorkerMemoryUsed};
use sb_workers::context::{Timing, UserWorkerMsgs, UserWorkerRuntimeOpts};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver},
    oneshot, Mutex,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    deno_runtime::{DenoRuntime, RuntimeState},
    server::ServerFlags,
    utils::units::percentage_value,
};

use super::{pool::SupervisorPolicy, termination_token::TerminationToken};

pub mod strategy_per_request;
pub mod strategy_per_worker;

pub mod v8_handler;

pub use v8_handler::*;

#[repr(C)]
pub struct IsolateMemoryStats {
    pub used_heap_size: usize,
    pub external_memory: usize,
}

#[derive(Clone, Copy)]
pub struct CPUTimerParam {
    soft_limit_ms: u64,
    hard_limit_ms: u64,
}

impl CPUTimerParam {
    pub fn new(soft_limit_ms: u64, hard_limit_ms: u64) -> Self {
        Self {
            soft_limit_ms,
            hard_limit_ms,
        }
    }

    pub fn get_cpu_timer(
        &self,
        policy: SupervisorPolicy,
    ) -> Option<(CPUTimer, UnboundedReceiver<()>)> {
        let (cpu_alarms_tx, cpu_alarms_rx) = mpsc::unbounded_channel::<()>();

        if self.is_disabled() {
            return None;
        }

        Some((
            CPUTimer::start(
                if policy.is_per_worker() {
                    self.soft_limit_ms
                } else {
                    self.hard_limit_ms
                },
                if policy.is_per_request() {
                    0
                } else {
                    self.hard_limit_ms
                },
                CPUAlarmVal { cpu_alarms_tx },
            )
            .ok()?,
            cpu_alarms_rx,
        ))
    }

    pub fn limits(&self) -> (u64, u64) {
        (self.soft_limit_ms, self.hard_limit_ms)
    }

    pub fn is_disabled(&self) -> bool {
        self.soft_limit_ms == 0 && self.hard_limit_ms == 0
    }
}

pub struct Tokens {
    pub termination: Option<TerminationToken>,
    pub supervise: CancellationToken,
}

pub struct Arguments {
    pub key: Uuid,
    pub runtime_opts: UserWorkerRuntimeOpts,
    pub cpu_timer: Option<(CPUTimer, mpsc::UnboundedReceiver<()>)>,
    pub cpu_usage_metrics_rx: Option<mpsc::UnboundedReceiver<CPUUsageMetrics>>,
    pub cpu_timer_param: CPUTimerParam,
    pub supervisor_policy: SupervisorPolicy,
    pub runtime_state: Arc<RuntimeState>,
    pub promise_metrics: PromiseMetrics,
    pub timing: Option<Timing>,
    pub memory_limit_rx: mpsc::UnboundedReceiver<()>,
    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub isolate_memory_usage_tx: oneshot::Sender<IsolateMemoryStats>,
    pub thread_safe_handle: v8::IsolateHandle,
    pub waker: Arc<AtomicWaker>,
    pub tokens: Tokens,
    pub flags: Arc<ServerFlags>,
}

pub struct CPUUsage {
    pub accumulated: i64,
    pub diff: i64,
}

#[derive(EnumAsInner)]
pub enum CPUUsageMetrics {
    Enter(std::thread::ThreadId),
    Leave(CPUUsage),
}

async fn wait_cpu_alarm(maybe_alarm: Option<&mut UnboundedReceiver<()>>) -> Option<()> {
    match maybe_alarm {
        Some(alarm) => Some(alarm.recv().await?),
        None => None,
    }
}

async fn create_wall_clock_beforeunload_alert(wall_clock_limit_ms: u64, pct: Option<u8>) {
    let dur = pct
        .and_then(|it| percentage_value(wall_clock_limit_ms, it))
        .map(Duration::from_millis);

    if let Some(dur) = dur {
        tokio::time::sleep(dur).await;
    } else {
        pending::<()>().await;
        unreachable!()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_supervisor(
    key: Uuid,
    runtime: &mut DenoRuntime,
    policy: SupervisorPolicy,
    termination_event_tx: oneshot::Sender<WorkerEvents>,
    pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    cpu_usage_metrics_rx: Option<mpsc::UnboundedReceiver<CPUUsageMetrics>>,
    cancel: Option<CancellationToken>,
    timing: Option<Timing>,
    termination_token: Option<TerminationToken>,
    flags: Arc<ServerFlags>,
) -> Result<(Option<CPUTimer>, CancellationToken), anyhow::Error> {
    let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel();
    let (waker, thread_safe_handle) = {
        let js_runtime = &mut runtime.js_runtime;
        (
            js_runtime.op_state().borrow().waker.clone(),
            js_runtime.v8_isolate().thread_safe_handle(),
        )
    };

    // we assert supervisor is only run for user workers
    let conf = runtime.conf.as_user_worker().unwrap().clone();
    let mem_check_state = runtime.mem_check_state();
    let termination_request_token = runtime.termination_request_token.clone();
    let runtime_drop_token = runtime.drop_token.clone();

    let giveup_process_requests_token = cancel.clone();
    let supervise_cancel_token = CancellationToken::new();
    let tokens = Tokens {
        termination: termination_token.clone(),
        supervise: supervise_cancel_token.clone(),
    };

    let maybe_inspector_params = runtime.inspector().map(|_| {
        (
            runtime
                .js_runtime
                .inspector()
                .borrow_mut()
                .get_session_sender(),
            runtime.runtime_state.clone(),
        )
    });

    let send_memory_limit_fn = move |kind: &'static str| {
        log::debug!("memory limit triggered: isolate: {:?}, kind: {}", key, kind);

        if memory_limit_tx.send(()).is_err() {
            log::error!(
                "failed to send memory limit reached notification(isolate may already be terminating): isolate: {:?}, kind: {}",
                key, kind
            );
        }
    };

    runtime.add_memory_limit_callback({
        let send_fn = send_memory_limit_fn.clone();
        move |_| {
            send_fn("mem_check");
        }
    });

    runtime.js_runtime.add_near_heap_limit_callback({
        let send_fn = send_memory_limit_fn;
        move |current, _| {
            send_fn("v8");

            // give an allowance on current limit (until the isolate is
            // terminated) we do this so that oom won't end up killing the
            // edge-runtime process
            current * (conf.low_memory_multiplier as usize)
        }
    });

    // Note: CPU timer must be started in the same thread as the worker runtime

    let cpu_timer_param =
        CPUTimerParam::new(conf.cpu_time_soft_limit_ms, conf.cpu_time_hard_limit_ms);

    let (maybe_cpu_timer, maybe_cpu_alarms_rx) = cpu_timer_param.get_cpu_timer(policy).unzip();

    drop({
        let _rt_guard = base_rt::SUPERVISOR_RT.enter();
        let maybe_cpu_timer_inner = maybe_cpu_timer.clone();
        let supervise_cancel_token_inner = supervise_cancel_token.clone();
        let runtime_state = runtime.runtime_state.clone();
        let promise_metrics = runtime.promise_metrics();

        tokio::spawn(async move {
            let (isolate_memory_usage_tx, isolate_memory_usage_rx) =
                oneshot::channel::<IsolateMemoryStats>();

            let args = Arguments {
                key,
                runtime_opts: conf.clone(),
                cpu_timer: maybe_cpu_timer_inner.zip(maybe_cpu_alarms_rx),
                cpu_usage_metrics_rx,
                cpu_timer_param,
                supervisor_policy: policy,
                runtime_state,
                promise_metrics,
                timing,
                memory_limit_rx,
                pool_msg_tx,
                isolate_memory_usage_tx,
                thread_safe_handle,
                waker: waker.clone(),
                tokens,
                flags,
            };

            let (reason, cpu_usage_ms) = {
                match policy {
                    SupervisorPolicy::PerWorker => strategy_per_worker::supervise(args).await,
                    SupervisorPolicy::PerRequest { oneshot, .. } => {
                        strategy_per_request::supervise(args, oneshot).await
                    }
                }
            };

            // NOTE: Sending a signal to the pooler that it is the user worker going
            // disposed down and will not accept awaiting subsequent requests, so
            // they must be re-polled again.
            if let Some(cancel) = giveup_process_requests_token.as_ref() {
                cancel.cancel();
            }

            if let Some((session_tx, state)) = maybe_inspector_params {
                use deno_core::futures::channel::mpsc;
                use deno_core::serde_json::Value;

                let termination_request_token = termination_request_token.clone();

                base_rt::SUPERVISOR_RT
                    .spawn_blocking(move || {
                        let wait_inspector_disconnect_fut = async move {
                            let ls = tokio::task::LocalSet::new();
                            ls.run_until(async move {
                                if state.is_terminated() || termination_request_token.is_cancelled()
                                {
                                    return;
                                }

                                termination_request_token.cancel();

                                if state.is_found_inspector_session() {
                                    return;
                                }

                                let (outbound_tx, outbound_rx) = mpsc::unbounded();
                                let (inbound_tx, inbound_rx) = mpsc::unbounded();

                                if session_tx
                                    .unbounded_send(InspectorSessionProxy {
                                        tx: outbound_tx,
                                        rx: inbound_rx,
                                    })
                                    .is_err()
                                {
                                    return;
                                }

                                let session = Arc::new(Mutex::new(LocalInspectorSession::new(
                                    inbound_tx,
                                    outbound_rx,
                                )));

                                let send_msg_fn = {
                                    |msg| {
                                        let state = state.clone();
                                        let session = session.clone();
                                        async move {
                                            let mut session = session.lock().await;
                                            let mut int =
                                                tokio::time::interval(Duration::from_millis(61));

                                            let fut = session.post_message(msg, None::<Value>);

                                            pin_mut!(fut);

                                            loop {
                                                tokio::select! {
                                                    _ = int.tick() => {
                                                        if state.is_terminated() {
                                                            break
                                                        }
                                                    }

                                                    res = &mut fut => {
                                                        res.unwrap();
                                                        break
                                                    }
                                                }
                                            }
                                        }
                                    }
                                };

                                send_msg_fn("Debugger.enable").await;
                                send_msg_fn("Runtime.runIfWaitingForDebugger").await;
                            })
                            .await;
                        };

                        base_rt::SUPERVISOR_RT.block_on(wait_inspector_disconnect_fut);
                    })
                    .await
                    .unwrap();
            }

            // NOTE: If we issue a hard CPU time limit, It's OK because it is
            // still possible the worker's context is in the v8 event loop. The
            // interrupt callback would be invoked from the V8 engine
            // gracefully. But some case doesn't.
            //
            // Such as the worker going to a retired state due to the soft CPU
            // time limit but not hitting the hard CPU time limit. In this case,
            // we must wake up the worker's event loop manually. Otherwise, the
            // supervisor has to wait until the wall clock future that we placed
            // out on the runtime side is times out.
            waker.wake();

            let memory_report = tokio::select! {
                report = isolate_memory_usage_rx => report.map_err(anyhow::Error::from),
                _ = runtime_drop_token.cancelled() => Err(anyhow!("termination requested"))
            };

            let memory_used = match memory_report {
                Ok(v) => WorkerMemoryUsed {
                    total: v.used_heap_size + v.external_memory,
                    heap: v.used_heap_size,
                    external: v.external_memory,
                    mem_check_captured: tokio::task::spawn_blocking(move || {
                        *mem_check_state.read().unwrap()
                    })
                    .await
                    .unwrap(),
                },

                Err(_) => {
                    if !supervise_cancel_token_inner.is_cancelled() {
                        log::warn!("isolate memory usage sender dropped");
                    }

                    WorkerMemoryUsed {
                        total: 0,
                        heap: 0,
                        external: 0,
                        mem_check_captured: MemCheckState::default(),
                    }
                }
            };

            if !termination_request_token.is_cancelled() {
                termination_request_token.cancel();
                waker.wake();
            }

            // send termination reason
            let termination_event = WorkerEvents::Shutdown(ShutdownEvent {
                reason,
                memory_used,
                cpu_time_used: cpu_usage_ms as usize,
            });

            let _ = termination_event_tx.send(termination_event);
        })
    });

    Ok((maybe_cpu_timer, supervise_cancel_token))
}
