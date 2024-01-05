use std::{sync::atomic::Ordering, time::Duration};

use event_worker::events::ShutdownReason;
use log::error;
use sb_workers::context::{Timing, TimingStatus, UserWorkerMsgs};

use super::{handle_interrupt, Arguments, IsolateInterruptData};

pub async fn supervise(args: Arguments) -> ShutdownReason {
    let Arguments {
        key,
        runtime_opts,
        timing,
        mut cpu_alarms_rx,
        mut memory_limit_rx,
        pool_msg_tx,
        isolate_memory_usage_tx,
        thread_safe_handle,
        ..
    } = args;

    let Timing {
        status: TimingStatus { demand, is_retired },
        req: (_, mut req_end_rx),
    } = timing.unwrap_or_default();

    let is_retired = is_retired.unwrap();

    let mut cpu_time_soft_limit_reached = false;
    let mut wall_clock_alerts = 0;
    let mut req_ack_count = 0usize;

    // reduce 100ms from wall clock duration, so the interrupt can be handled before
    // isolate is dropped
    let wall_clock_duration =
        Duration::from_millis(runtime_opts.worker_timeout_ms) - Duration::from_millis(100);

    // Split wall clock duration into 2 intervals.
    // At the first interval, we will send a msg to retire the worker.
    let wall_clock_duration_alert = tokio::time::interval(
        wall_clock_duration
            .checked_div(2)
            .unwrap_or(Duration::from_millis(0)),
    );

    tokio::pin!(wall_clock_duration_alert);

    loop {
        tokio::select! {
            Some(_) = cpu_alarms_rx.recv() => {
                if !cpu_time_soft_limit_reached {
                    // retire worker
                    is_retired.raise();
                    error!("CPU time soft limit reached. isolate: {:?}", key);
                    cpu_time_soft_limit_reached = true;

                    if req_ack_count == demand.load(Ordering::Acquire) {
                        let interrupt_data = IsolateInterruptData {
                            should_terminate: true,
                            isolate_memory_usage_tx
                        };

                        thread_safe_handle.request_interrupt(handle_interrupt, Box::into_raw(Box::new(interrupt_data)) as *mut std::ffi::c_void);
                        error!("early termination due to the last request being completed. isolate: {:?}", key);
                        return ShutdownReason::EarlyDrop;
                    }
                } else {
                    // shutdown worker
                    let interrupt_data = IsolateInterruptData {
                        should_terminate: true,
                        isolate_memory_usage_tx
                    };
                    thread_safe_handle.request_interrupt(handle_interrupt, Box::into_raw(Box::new(interrupt_data)) as *mut std::ffi::c_void);
                    error!("CPU time hard limit reached. isolate: {:?}", key);
                    return ShutdownReason::CPUTime;
                }
            }

            Some(_) = req_end_rx.recv() => {
                req_ack_count += 1;

                if !cpu_time_soft_limit_reached {
                    if let Some(tx) = pool_msg_tx.clone() {
                        if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                            error!("failed to send idle msg to pool: {:?}", key);
                        }
                    }
                }

                if !cpu_time_soft_limit_reached || req_ack_count != demand.load(Ordering::Acquire) {
                    continue;
                }

                let interrupt_data = IsolateInterruptData {
                    should_terminate: true,
                    isolate_memory_usage_tx
                };

                thread_safe_handle.request_interrupt(handle_interrupt, Box::into_raw(Box::new(interrupt_data)) as *mut std::ffi::c_void);
                error!("early termination due to the last request being completed. isolate: {:?}", key);
                return ShutdownReason::EarlyDrop;
            }

            // wall clock warning
            _ = wall_clock_duration_alert.tick() => {
                if wall_clock_alerts == 0 {
                    // first tick completes immediately
                    wall_clock_alerts += 1;
                } else if wall_clock_alerts == 1 {
                    // retire worker
                    is_retired.raise();
                    error!("wall clock duration warning. isolate: {:?}", key);
                    wall_clock_alerts += 1;
                } else {
                    // wall-clock limit reached
                    // Don't terminate isolate from supervisor when wall-clock
                    // duration reached. It's dropped in deno_runtime.rs
                    let interrupt_data = IsolateInterruptData {
                        // NOTE: Wall clock is also triggered when no more
                        // pending requests, so we must compare the request
                        // count here to judge whether we need to terminate the
                        // isolate.
                        should_terminate: req_ack_count == demand.load(Ordering::Acquire),
                        isolate_memory_usage_tx
                    };
                    thread_safe_handle.request_interrupt(handle_interrupt, Box::into_raw(Box::new(interrupt_data)) as *mut std::ffi::c_void);
                    error!("wall clock duration reached. isolate: {:?}", key);
                    return ShutdownReason::WallClockTime;
                }
            }

            // memory usage
            Some(_) = memory_limit_rx.recv() => {
                let interrupt_data = IsolateInterruptData {
                    should_terminate: true,
                    isolate_memory_usage_tx
                };
                thread_safe_handle.request_interrupt(handle_interrupt, Box::into_raw(Box::new(interrupt_data)) as *mut std::ffi::c_void);
                error!("memory limit reached for the worker. isolate: {:?}", key);
                return ShutdownReason::Memory;
            }
        }
    }
}
