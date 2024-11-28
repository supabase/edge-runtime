pub mod strategy_per_request;
pub mod strategy_per_worker;

use std::{future::pending, sync::Arc, time::Duration};

use cpu_timer::{CPUAlarmVal, CPUTimer};
use deno_core::v8;
use enum_as_inner::EnumAsInner;
use futures_util::task::AtomicWaker;
use log::{error, warn};
use sb_core::PromiseMetrics;
use sb_workers::context::{Timing, UserWorkerMsgs, UserWorkerRuntimeOpts};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver},
    oneshot,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::{
    deno_runtime::{MaybeDenoRuntime, WillTerminateReason},
    server::ServerFlags,
    utils::units::percentage_value,
};

use super::{worker_ctx::TerminationToken, worker_pool::SupervisorPolicy};

#[repr(C)]
pub struct V8HandleTerminationData {
    pub should_terminate: bool,
    pub isolate_memory_usage_tx: Option<oneshot::Sender<IsolateMemoryStats>>,
}

pub extern "C" fn v8_handle_termination(isolate: &mut v8::Isolate, data: *mut std::ffi::c_void) {
    let mut boxed_data: Box<V8HandleTerminationData>;

    unsafe {
        boxed_data = Box::from_raw(data as *mut V8HandleTerminationData);
    }

    // log memory usage
    let mut heap_stats = v8::HeapStatistics::default();

    isolate.get_heap_statistics(&mut heap_stats);

    let usage = IsolateMemoryStats {
        used_heap_size: heap_stats.used_heap_size(),
        external_memory: heap_stats.external_memory(),
    };

    if let Some(usage_tx) = boxed_data.isolate_memory_usage_tx.take() {
        if usage_tx.send(usage).is_err() {
            error!("failed to send isolate memory usage - receiver may have been dropped");
        }
    }

    if boxed_data.should_terminate {
        isolate.terminate_execution();
    }
}

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

extern "C" fn v8_handle_wall_clock_beforeunload(
    isolate: &mut v8::Isolate,
    _data: *mut std::ffi::c_void,
) {
    if let Err(err) = MaybeDenoRuntime::<()>::Isolate(isolate)
        .dispatch_beforeunload_event(WillTerminateReason::WallClock)
    {
        warn!(
            "found an error while dispatching the beforeunload event: {}",
            err
        );
    }
}

#[instrument(level = "debug", skip_all)]
extern "C" fn v8_handle_early_retire(isolate: &mut v8::Isolate, _data: *mut std::ffi::c_void) {
    isolate.low_memory_notification();
    debug!("sent low mem notification");
}
