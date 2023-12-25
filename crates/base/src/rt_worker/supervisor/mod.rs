pub mod strategy_per_request;
pub mod strategy_per_worker;

use std::sync::Arc;

use cpu_timer::CPUTimer;
use deno_core::v8::IsolateHandle;
use futures_util::task::AtomicWaker;
use log::error;
use sb_workers::context::{UserWorkerMsgs, UserWorkerRuntimeOpts};
use tokio::sync::{mpsc, oneshot, Notify};
use uuid::Uuid;

use super::worker_pool::CPUTimerPolicy;

#[repr(C)]
struct IsolateInterruptData {
    pub should_terminate: bool,
    pub isolate_memory_usage_tx: oneshot::Sender<IsolateMemoryStats>,
}

extern "C" fn handle_interrupt(isolate: &mut deno_core::v8::Isolate, data: *mut std::ffi::c_void) {
    let boxed_data: Box<IsolateInterruptData>;
    unsafe {
        boxed_data = Box::from_raw(data as *mut IsolateInterruptData);
    }

    // log memory usage
    let mut heap_stats = deno_core::v8::HeapStatistics::default();
    isolate.get_heap_statistics(&mut heap_stats);
    let usage = IsolateMemoryStats {
        used_heap_size: heap_stats.used_heap_size(),
        external_memory: heap_stats.external_memory(),
    };

    if boxed_data.isolate_memory_usage_tx.send(usage).is_err() {
        error!("failed to send isolate memory usage - receiver may have been dropped");
    };

    if boxed_data.should_terminate {
        isolate.terminate_execution();
    }
}

#[repr(C)]
pub struct IsolateMemoryStats {
    pub used_heap_size: usize,
    pub external_memory: usize,
}

pub struct Arguments {
    pub key: Uuid,
    pub runtime_opts: UserWorkerRuntimeOpts,
    pub cpu_timer: Option<CPUTimer>,
    pub cpu_timer_policy: CPUTimerPolicy,
    pub cpu_alarms_rx: mpsc::UnboundedReceiver<()>,
    pub req_start_rx: mpsc::UnboundedReceiver<Arc<Notify>>,
    pub req_end_rx: mpsc::UnboundedReceiver<()>,
    pub memory_limit_rx: mpsc::UnboundedReceiver<()>,
    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub isolate_memory_usage_tx: oneshot::Sender<IsolateMemoryStats>,
    pub thread_safe_handle: IsolateHandle,
    pub waker: Arc<AtomicWaker>,
}
