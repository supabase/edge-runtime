use std::{future::pending, sync::atomic::Ordering, time::Duration};

#[cfg(debug_assertions)]
use std::thread::ThreadId;

use sb_event_worker::events::ShutdownReason;
use sb_workers::context::{Timing, TimingStatus, UserWorkerMsgs};
use tokio::time::Instant;

use crate::{
    deno_runtime::WillTerminateReason,
    worker::supervisor::{
        create_wall_clock_beforeunload_alert, v8_handle_beforeunload, v8_handle_early_retire,
        v8_handle_termination, wait_cpu_alarm, CPUUsage, CPUUsageMetrics, Tokens,
        V8HandleBeforeunloadData, V8HandleTerminationData,
    },
};

use super::Arguments;

pub async fn supervise(args: Arguments, oneshot: bool) -> (ShutdownReason, i64) {
    let Arguments {
        key,
        runtime_opts,
        timing,
        cpu_timer,
        cpu_timer_param,
        cpu_usage_metrics_rx,
        mut memory_limit_rx,
        pool_msg_tx,
        isolate_memory_usage_tx,
        thread_safe_handle,
        waker,
        tokens: Tokens {
            termination,
            supervise,
        },
        flags,
        ..
    } = args;

    let Timing {
        status: TimingStatus { demand, is_retired },
        req: (mut req_start_rx, mut req_end_rx),
        ..
    } = timing.unwrap_or_default();

    let (cpu_timer, mut cpu_alarms_rx) = cpu_timer.unzip();
    let (_, hard_limit_ms) = cpu_timer_param.limits();

    let _guard = scopeguard::guard(is_retired, |v| {
        v.raise();

        if thread_safe_handle.request_interrupt(v8_handle_early_retire, std::ptr::null_mut()) {
            waker.wake();
        }
    });

    #[cfg(debug_assertions)]
    let mut current_thread_id = Option::<ThreadId>::None;

    let wall_clock_limit_ms = runtime_opts.worker_timeout_ms;

    let is_wall_clock_limit_disabled = wall_clock_limit_ms == 0;
    let mut is_worker_entered = false;
    let mut is_wall_clock_beforeunload_armed = false;

    let mut cpu_usage_metrics_rx = cpu_usage_metrics_rx.unwrap();
    let mut cpu_usage_ms = 0i64;
    let mut cpu_usage_accumulated_ms = 0i64;

    let mut complete_reason = None::<ShutdownReason>;
    let mut req_ack_count = 0usize;
    let mut req_start_ack = false;

    let wall_clock_limit_ms = if wall_clock_limit_ms < 1 {
        1
    } else {
        wall_clock_limit_ms
    };

    let wall_clock_duration = Duration::from_millis(wall_clock_limit_ms);
    let wall_clock_duration_alert = tokio::time::sleep(wall_clock_duration);
    let wall_clock_beforeunload_alert = create_wall_clock_beforeunload_alert(
        wall_clock_limit_ms,
        flags.beforeunload_wall_clock_pct,
    );

    tokio::pin!(wall_clock_duration_alert);
    tokio::pin!(wall_clock_beforeunload_alert);

    loop {
        tokio::select! {
            _ = supervise.cancelled() => {
                return (ShutdownReason::TerminationRequested, cpu_usage_ms);
            }

            _ = async {
                match termination.as_ref() {
                    Some(token) => token.inbound.cancelled().await,
                    None => pending().await,
                }
            } => {
                complete_reason = Some(ShutdownReason::TerminationRequested);
            }

            Some(metrics) = cpu_usage_metrics_rx.recv() => {
                match metrics {
                    CPUUsageMetrics::Enter(_thread_id) => {
                        // INVARIANT: Thread ID MUST equal with previously captured
                        // Thread ID.
                        #[cfg(debug_assertions)]
                        {
                            assert!(current_thread_id.unwrap_or(_thread_id) == _thread_id);
                            current_thread_id = Some(_thread_id);
                        }

                        assert!(!is_worker_entered);
                        is_worker_entered = true;

                        if !cpu_timer_param.is_disabled() {
                            if let Some(Err(err)) = cpu_timer.as_ref().map(|it| it.reset()) {
                                log::error!("can't reset cpu timer: {}", err);
                            }
                        }
                    }

                    CPUUsageMetrics::Leave(CPUUsage { accumulated, diff }) => {
                        assert!(is_worker_entered);

                        is_worker_entered = false;
                        cpu_usage_ms += diff / 1_000_000;
                        cpu_usage_accumulated_ms = accumulated / 1_000_000;

                        if !cpu_timer_param.is_disabled() {
                            if cpu_usage_ms >= hard_limit_ms as i64 {
                                log::error!("CPU time limit reached: isolate: {:?}", key);
                                complete_reason = Some(ShutdownReason::CPUTime);
                            }

                            if let Some(Err(err)) = cpu_timer.as_ref().map(|it| it.reset()) {
                                log::error!("can't reset cpu timer: {}", err);
                            }
                        }
                    }
                }
            }

            Some(_) = wait_cpu_alarm(cpu_alarms_rx.as_mut()) => {
                if is_worker_entered && req_start_ack {
                    log::error!("CPU time limit reached: isolate: {:?}", key);
                    complete_reason = Some(ShutdownReason::CPUTime);
                }
            }

            Some(notify) = req_start_rx.recv() => {
                // INVARIANT: This branch MUST not be satisfied more than once
                // during the same request cycle.
                assert!(!req_start_ack, "supervisor has seen request start signal twice");

                notify.notify_one();

                if let Some(cpu_timer) = cpu_timer.as_ref() {
                    if let Err(ex) = cpu_timer.reset() {
                        log::error!("cannot reset the cpu timer: {}", ex);
                    }
                }

                cpu_usage_ms = 0;
                req_start_ack = true;
                complete_reason = None;
            }

            Some(_) = req_end_rx.recv() => {
                // INVARIANT: This branch MUST be satisfied only once after the
                // request start signal has arrived during the same request
                // cycle.
                assert!(req_start_ack, "supervisor observed the request end signal but did not see request start signal");

                req_ack_count += 1;
                complete_reason = Some(ShutdownReason::EarlyDrop);
            }

            _ = &mut wall_clock_duration_alert, if !is_wall_clock_limit_disabled => {
                if !oneshot && req_ack_count != demand.load(Ordering::Acquire) {
                    wall_clock_duration_alert
                        .as_mut()
                        .reset(Instant::now() + wall_clock_duration);

                    continue;
                } else {
                    log::error!("wall clock duraiton reached: isolate: {:?}", key);
                    complete_reason = Some(ShutdownReason::WallClockTime);
                }
            }

            _ = &mut wall_clock_beforeunload_alert,
                if !is_wall_clock_limit_disabled && !is_wall_clock_beforeunload_armed
            => {
                let data_ptr_mut = Box::into_raw(Box::new(V8HandleBeforeunloadData {
                    reason: WillTerminateReason::WallClock
                }));

                if thread_safe_handle
                    .request_interrupt(v8_handle_beforeunload, data_ptr_mut as *mut _)
                {
                    waker.wake();
                } else {
                    drop(unsafe { Box::from_raw(data_ptr_mut)});
                }

                is_wall_clock_beforeunload_armed = true;
            }

            Some(_) = memory_limit_rx.recv() => {
                log::error!("memory limit reached for the worker: isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::Memory);
            }
        }

        match complete_reason.take() {
            Some(ShutdownReason::EarlyDrop) if !oneshot => {
                req_start_ack = false;
                wall_clock_duration_alert
                    .as_mut()
                    .reset(Instant::now() + wall_clock_duration);

                if let Some(tx) = pool_msg_tx.clone() {
                    if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                        log::error!("failed to send idle msg to pool: {:?}", key);
                    }
                }

                continue;
            }

            Some(reason) => {
                let data_ptr_mut = Box::into_raw(Box::new(V8HandleTerminationData {
                    should_terminate: true,
                    isolate_memory_usage_tx: Some(isolate_memory_usage_tx),
                }));

                if !thread_safe_handle
                    .request_interrupt(v8_handle_termination, data_ptr_mut as *mut _)
                {
                    drop(unsafe { Box::from_raw(data_ptr_mut) });
                }

                return (reason, cpu_usage_accumulated_ms);
            }

            None => continue,
        }
    }
}
