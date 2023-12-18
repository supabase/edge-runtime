use std::time::Duration;

use event_worker::events::ShutdownReason;
use log::error;
use sb_workers::context::UserWorkerMsgs;
use tokio::time::Instant;

use crate::rt_worker::supervisor::{handle_interrupt, IsolateInterruptData};

use super::Arguments;

pub async fn supervise(args: Arguments) -> ShutdownReason {
    let Arguments {
        key,
        runtime_opts,
        mut cpu_alarms_rx,
        mut req_start_rx,
        mut req_end_rx,
        mut memory_limit_rx,
        pool_msg_tx,
        isolate_memory_usage_tx,
        thread_safe_handle,
        cpu_timer,
        ..
    } = args;

    let mut complete_reason = None::<ShutdownReason>;
    let mut req_start_ack = false;

    // reduce 100ms from wall clock duration, so the interrupt can be handled before
    // isolate is dropped
    let wall_clock_duration =
        Duration::from_millis(runtime_opts.worker_timeout_ms) - Duration::from_millis(100);

    let wall_clock_duration_alert = tokio::time::sleep(wall_clock_duration);

    tokio::pin!(wall_clock_duration_alert);
    loop {
        tokio::select! {
            Some(_) = cpu_alarms_rx.recv() => {
                if req_start_ack {
                    error!("CPU time limit reached. isolate: {:?}", key);
                    complete_reason = Some(ShutdownReason::CPUTime);
                }
            }

            Some(_) = req_start_rx.recv() => {
                // INVARIANT: This branch MUST not be satisfied more than once
                // during the same request cycle.
                assert!(!req_start_ack, "supervisor has seen request start signal twice");

                if let Some(x) = cpu_timer.as_ref() {
                    if let Err(ex) = x.reset() {
                        error!("cannot reset the cpu timer: {}", ex);
                    }
                }

                req_start_ack = true;
                complete_reason = None;
            }

            Some(_) = req_end_rx.recv() => {
                // INVARIANT: This branch MUST be satisfied only once after the
                // request start signal has arrived during the same request
                // cycle.
                assert!(req_start_ack, "supervisor observed the request end signal but did not see request start signal");
                complete_reason = Some(ShutdownReason::EarlyDrop);
            }

            _ = &mut wall_clock_duration_alert => {
                error!("wall clock duraiton reached. isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::WallClockTime);
            }

            Some(_) = memory_limit_rx.recv() => {
                error!("memory limit reached for the worker. isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::Memory);
            }
        }

        match complete_reason.take() {
            Some(ShutdownReason::EarlyDrop) => {
                req_start_ack = false;
                wall_clock_duration_alert
                    .as_mut()
                    .reset(Instant::now() + wall_clock_duration);

                if let Some(tx) = pool_msg_tx.clone() {
                    if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                        error!("failed to send idle msg to pool: {:?}", key);
                    }
                }

                continue;
            }

            Some(x) => {
                thread_safe_handle.request_interrupt(
                    handle_interrupt,
                    Box::into_raw(Box::new(IsolateInterruptData {
                        should_terminate: true,
                        isolate_memory_usage_tx,
                    })) as *mut std::ffi::c_void,
                );

                return x;
            }

            None => continue,
        }
    }
}
