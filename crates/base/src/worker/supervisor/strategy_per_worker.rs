use std::{future::pending, sync::atomic::Ordering, time::Duration};

#[cfg(debug_assertions)]
use std::thread::ThreadId;

use event_worker::events::ShutdownReason;
use log::{error, info};
use sb_workers::context::{Timing, TimingStatus, UserWorkerMsgs};
use tokio_util::sync::CancellationToken;

use crate::{
    deno_runtime::WillTerminateReason,
    worker::supervisor::{
        create_wall_clock_beforeunload_alert, v8_handle_beforeunload, v8_handle_drain,
        v8_handle_early_drop_beforeunload, v8_handle_early_retire, wait_cpu_alarm, CPUUsage,
        Tokens, V8HandleBeforeunloadData, V8HandleEarlyDropData,
    },
};

use super::{v8_handle_termination, Arguments, CPUUsageMetrics, V8HandleTerminationData};

pub async fn supervise(args: Arguments) -> (ShutdownReason, i64) {
    let Arguments {
        key,
        runtime_opts,
        runtime_state,
        promise_metrics,
        timing,
        mut memory_limit_rx,
        cpu_timer,
        cpu_timer_param,
        cpu_usage_metrics_rx,
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
        req: (_, mut req_end_rx),
    } = timing.unwrap_or_default();

    let (cpu_timer, mut cpu_alarms_rx) = cpu_timer.unzip();
    let (soft_limit_ms, hard_limit_ms) = cpu_timer_param.limits();

    let guard = scopeguard::guard(is_retired.clone(), |v| {
        v.raise();
    });

    #[cfg(debug_assertions)]
    let mut current_thread_id = Option::<ThreadId>::None;

    let wall_clock_limit_ms = runtime_opts.worker_timeout_ms;

    let is_wall_clock_limit_disabled = wall_clock_limit_ms == 0;
    let mut is_worker_entered = false;
    let mut is_wall_clock_beforeunload_armed = false;
    let mut is_cpu_time_soft_limit_reached = false;
    let mut is_waiting_for_termination = false;
    let mut have_all_reqs_been_acknowledged: bool;

    let mut cpu_usage_metrics_rx = cpu_usage_metrics_rx.unwrap();
    let mut cpu_usage_ms = 0i64;

    let mut complete_reason = None::<ShutdownReason>;
    let mut wall_clock_alerts = 0;
    let mut req_ack_count = 0usize;

    let wall_clock_limit_ms = if wall_clock_limit_ms < 2 {
        2
    } else {
        wall_clock_limit_ms
    };

    let wall_clock_duration = Duration::from_millis(wall_clock_limit_ms);

    // Split wall clock duration into 2 intervals.
    // At the first interval, we will send a msg to retire the worker.
    let wall_clock_duration_alert = tokio::time::interval(
        wall_clock_duration
            .checked_div(2)
            .unwrap_or(Duration::from_millis(1)),
    );

    let wall_clock_beforeunload_alert = create_wall_clock_beforeunload_alert(
        wall_clock_limit_ms,
        flags.beforeunload_wall_clock_pct,
    );

    let early_retire_fn = || {
        // we should raise a retire signal because subsequent incoming requests are unlikely to get
        // enough wall clock time or cpu time
        guard.raise();

        if thread_safe_handle.request_interrupt(v8_handle_early_retire, std::ptr::null_mut()) {
            waker.wake();
        }
    };

    let early_drop_token = CancellationToken::new();
    let early_drop_fut = early_drop_token.cancelled();

    let mut dispatch_early_drop_beforeunload_fn = Some({
        let token = early_drop_token.clone();
        || {
            let data_ptr_mut = Box::into_raw(Box::new(V8HandleEarlyDropData { token }));

            if !thread_safe_handle.request_interrupt(
                v8_handle_early_drop_beforeunload,
                data_ptr_mut as *mut std::ffi::c_void,
            ) {
                unsafe { Box::from_raw(data_ptr_mut) }.token.cancel();
            } else {
                waker.wake();
            }
        }
    });

    let mut dispatch_drain_fn = Some(|| {
        if thread_safe_handle.request_interrupt(v8_handle_drain, std::ptr::null_mut()) {
            waker.wake();
        }
    });

    let terminate_fn = {
        let thread_safe_handle = thread_safe_handle.clone();
        move |should_terminate: bool| {
            let data_ptr_mut = Box::into_raw(Box::new(V8HandleTerminationData {
                should_terminate,
                isolate_memory_usage_tx: Some(isolate_memory_usage_tx),
            }));

            if !thread_safe_handle
                .request_interrupt(v8_handle_termination, data_ptr_mut as *mut std::ffi::c_void)
            {
                drop(unsafe { Box::from_raw(data_ptr_mut) });
            }
        }
    };

    tokio::pin!(wall_clock_duration_alert);
    tokio::pin!(wall_clock_beforeunload_alert);
    tokio::pin!(early_drop_fut);

    loop {
        tokio::select! {
            _ = supervise.cancelled() => {
                complete_reason = Some(ShutdownReason::TerminationRequested);
            }

            _ = async {
                match termination.as_ref() {
                    Some(token) => token.inbound.cancelled().await,
                    None => pending().await,
                }
            }, if !is_waiting_for_termination => {
                is_waiting_for_termination = true;

                early_retire_fn();
                if let Some(func) = dispatch_drain_fn.take() {
                    func();
                }
                if promise_metrics.have_all_promises_been_resolved() {
                    if let Some(func) = dispatch_early_drop_beforeunload_fn.take() {
                        func();
                    }
                } else {
                    let is_retired = is_retired.clone();
                    let waker = waker.clone();

                    drop(tokio::spawn(async move {
                        let mut interval = tokio::time::interval(Duration::from_secs(1));
                        loop {
                            interval.tick().await;
                            waker.wake();
                            if is_retired.is_raised() {
                                break;
                            }
                        }
                    }));
                }
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
                                error!("can't reset cpu timer: {}", err);
                            }
                        }
                    }

                    CPUUsageMetrics::Leave(CPUUsage { accumulated, .. }) => {
                        assert!(is_worker_entered);

                        is_worker_entered = false;
                        cpu_usage_ms = accumulated / 1_000_000;
                        have_all_reqs_been_acknowledged = req_ack_count == demand.load(Ordering::Acquire);

                        if !cpu_timer_param.is_disabled() {
                            if cpu_usage_ms >= hard_limit_ms as i64 {
                                error!("CPU time hard limit reached: isolate: {:?}", key);
                                complete_reason = Some(ShutdownReason::CPUTime);
                            } else if cpu_usage_ms >= soft_limit_ms as i64 && !is_cpu_time_soft_limit_reached {
                                early_retire_fn();
                                error!("CPU time soft limit reached: isolate: {:?}", key);

                                is_cpu_time_soft_limit_reached = true;

                                if have_all_reqs_been_acknowledged
                                    && promise_metrics.have_all_promises_been_resolved()
                                {
                                    if let Some(func) = dispatch_early_drop_beforeunload_fn.take() {
                                        func();
                                    }
                                }
                            }
                        }

                        if (is_cpu_time_soft_limit_reached || is_waiting_for_termination)
                                && have_all_reqs_been_acknowledged
                                && promise_metrics.have_all_promises_been_resolved()
                        {
                            if let Some(func) = dispatch_early_drop_beforeunload_fn.take() {
                                func();
                            }
                        }
                    }
                }
            }

            Some(_) = wait_cpu_alarm(cpu_alarms_rx.as_mut()) => {
                if is_worker_entered {
                    if !is_cpu_time_soft_limit_reached {
                        early_retire_fn();
                        error!("CPU time soft limit reached: isolate: {:?}", key);

                        is_cpu_time_soft_limit_reached = true;
                        have_all_reqs_been_acknowledged = req_ack_count == demand.load(Ordering::Acquire);

                        if have_all_reqs_been_acknowledged
                            && promise_metrics.have_all_promises_been_resolved()
                        {
                            if let Some(func) = dispatch_early_drop_beforeunload_fn.take() {
                                func();
                            }
                        }
                    } else {
                        error!("CPU time hard limit reached: isolate: {:?}", key);
                        complete_reason = Some(ShutdownReason::CPUTime);
                    }
                }
            }

            Some(_) = req_end_rx.recv() => {
                req_ack_count += 1;
                have_all_reqs_been_acknowledged = req_ack_count == demand.load(Ordering::Acquire);

                if !is_cpu_time_soft_limit_reached {
                    if let Some(tx) = pool_msg_tx.clone() {
                        if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                            error!("failed to send idle msg to pool: {:?}", key);
                        }
                    }
                }

                if have_all_reqs_been_acknowledged && guard.is_raised() {
                    if let Some(func) = dispatch_drain_fn.take() {
                        func();
                    }
                }

                if !is_cpu_time_soft_limit_reached
                    || !have_all_reqs_been_acknowledged
                    || !promise_metrics.have_all_promises_been_resolved()
                {
                    continue;
                }

                if let Some(func) = dispatch_early_drop_beforeunload_fn.take() {
                    func();
                }
            }

            _ = wall_clock_duration_alert.tick(), if !is_wall_clock_limit_disabled => {
                if wall_clock_alerts == 0 {
                    // first tick completes immediately
                    wall_clock_alerts += 1;
                } else if wall_clock_alerts == 1 {
                    early_retire_fn();
                    error!("wall clock duration warning: isolate: {:?}", key);
                    wall_clock_alerts += 1;
                } else {
                    let is_in_flight_req_exists = req_ack_count != demand.load(Ordering::Acquire);

                    error!("wall clock duration reached: isolate: {:?} (in_flight_req_exists = {})", key, is_in_flight_req_exists);
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
                    drop(unsafe { Box::from_raw(data_ptr_mut) });
                }

                is_wall_clock_beforeunload_armed = true;
            }

            Some(_) = memory_limit_rx.recv() => {
                error!("memory limit reached for the worker: isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::Memory);
            }

            _ = &mut early_drop_fut => {
                info!("early termination has been triggered: isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::EarlyDrop);
            }
        }

        match complete_reason.take() {
            Some(ShutdownReason::EarlyDrop) => {
                terminate_fn(runtime_state.is_evaluating_mod());
                return (
                    if is_waiting_for_termination {
                        ShutdownReason::TerminationRequested
                    } else {
                        ShutdownReason::EarlyDrop
                    },
                    cpu_usage_ms,
                );
            }

            Some(result) => {
                terminate_fn(true);
                return (result, cpu_usage_ms);
            }
            None => continue,
        }
    }
}
