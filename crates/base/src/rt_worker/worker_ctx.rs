use crate::deno_runtime::DenoRuntime;
use crate::utils::send_event_if_event_worker_available;
use crate::utils::units::bytes_to_display;

use crate::rt_worker::worker::{Worker, WorkerHandler};
use crate::rt_worker::worker_pool::WorkerPool;
use anyhow::{anyhow, bail, Error};
use cpu_timer::{CPUAlarmVal, CPUTimer};
use event_worker::events::{
    BootEvent, ShutdownEvent, ShutdownReason, WorkerEventWithMetadata, WorkerEvents,
    WorkerMemoryUsed,
};
use hyper::{Body, Request, Response};
use log::{debug, error};
use sb_graph::EszipPayloadKind;
use sb_workers::context::{
    EventWorkerRuntimeOpts, MainWorkerRuntimeOpts, UserWorkerMsgs, WorkerContextInitOpts,
    WorkerRequestMsg, WorkerRuntimeOpts,
};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

async fn handle_request(
    unix_stream_tx: mpsc::UnboundedSender<UnixStream>,
    msg: WorkerRequestMsg,
) -> Result<(), Error> {
    // create a unix socket pair
    let (sender_stream, recv_stream) = UnixStream::pair()?;

    let _ = unix_stream_tx.send(recv_stream);

    // send the HTTP request to the worker over Unix stream
    let (mut request_sender, connection) = hyper::client::conn::handshake(sender_stream).await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::task::spawn(async move {
        if let Err(e) = connection.without_shutdown().await {
            error!("Error in worker connection: {}", e);
        }
    });
    tokio::task::yield_now().await;

    let result = request_sender.send_request(msg.req).await;
    let _ = msg.res_tx.send(result);

    Ok(())
}

#[repr(C)]
pub struct IsolateMemoryStats {
    pub used_heap_size: usize,
    pub external_memory: usize,
}

#[repr(C)]
pub struct IsolateInterruptData {
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

pub fn create_supervisor(
    key: Uuid,
    worker_runtime: &mut DenoRuntime,
    termination_event_tx: oneshot::Sender<WorkerEvents>,
    pool_msg_tx: Option<UnboundedSender<UserWorkerMsgs>>,
) -> Result<CPUTimer, Error> {
    let (memory_limit_tx, mut memory_limit_rx) = mpsc::unbounded_channel::<()>();
    let (waker, thread_safe_handle) = {
        let js_runtime = &mut worker_runtime.js_runtime;
        (
            js_runtime.op_state().borrow().waker.clone(),
            js_runtime.v8_isolate().thread_safe_handle(),
        )
    };

    // we assert supervisor is only run for user workers
    let conf = worker_runtime.conf.as_user_worker().unwrap().clone();

    worker_runtime.js_runtime.add_near_heap_limit_callback(move |cur, _| {
        debug!(
            "Low memory alert triggered: {}",
            bytes_to_display(cur as u64),
        );

        if memory_limit_tx.send(()).is_err() {
            error!("failed to send memory limit reached notification - isolate may already be terminating");
        };

        // give an allowance on current limit (until the isolate is terminated)
        // we do this so that oom won't end up killing the edge-runtime process
        cur * (conf.low_memory_multiplier as usize)
    });

    // Note: CPU timer must be started in the same thread as the worker runtime
    let (cpu_alarms_tx, mut cpu_alarms_rx) = mpsc::unbounded_channel::<()>();
    let cputimer = CPUTimer::start(
        conf.cpu_time_soft_limit_ms,
        conf.cpu_time_hard_limit_ms,
        CPUAlarmVal { cpu_alarms_tx },
    )?;

    let thread_name = format!("sb-sup-{:?}", key);
    let _handle = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();

            let (isolate_memory_usage_tx, isolate_memory_usage_rx) = oneshot::channel::<IsolateMemoryStats>();

            let future = async move {
                let mut cpu_time_soft_limit_reached = false;
                let mut wall_clock_alerts = 0;

                // reduce 100ms from wall clock duration, so the interrupt can be handled before
                // isolate is dropped
                let wall_clock_duration = Duration::from_millis(conf.worker_timeout_ms) - Duration::from_millis(100);

                // Split wall clock duration into 2 intervals.
                // At the first interval, we will send a msg to retire the worker.
                let wall_clock_duration_alert = tokio::time::interval(wall_clock_duration.checked_div(2).unwrap_or(Duration::from_millis(0)));
                tokio::pin!(wall_clock_duration_alert);

                loop {
                    tokio::select! {
                        Some(_) = cpu_alarms_rx.recv() => {
                            if !cpu_time_soft_limit_reached {
                                // retire worker
                                if let Some(tx) = pool_msg_tx.clone() {
                                    if tx.send(UserWorkerMsgs::Retire(key)).is_err() {
                                        error!("failed to send retire msg to pool: {:?}", key);
                                    }
                                }
                                error!("CPU time soft limit reached. isolate: {:?}", key);
                                cpu_time_soft_limit_reached = true;
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

                        // wall clock warning
                        _ = wall_clock_duration_alert.tick() => {
                            if wall_clock_alerts == 0 {
                                // first tick completes immediately
                                wall_clock_alerts += 1;
                            } else if wall_clock_alerts == 1 {
                                if let Some(tx) = pool_msg_tx.clone() {
                                    if tx.send(UserWorkerMsgs::Retire(key)).is_err() {
                                        error!("failed to send retire msg to pool: {:?}", key);
                                    }
                                }
                                error!("wall clock duration warning. isolate: {:?}", key);
                                wall_clock_alerts += 1;
                            } else {
                                // wall-clock limit reached
                                // Don't terminate isolate from supervisor when wall-clock
                                // duration reached. It's dropped in deno_runtime.rs
                                let interrupt_data = IsolateInterruptData {
                                    should_terminate: false,
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
            };

            let reason = local.block_on(&rt, future);

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

            let memory_used = match isolate_memory_usage_rx.blocking_recv() {
                Ok(v) => {
                    WorkerMemoryUsed {
                        total: v.used_heap_size + v.external_memory,
                        heap: v.used_heap_size,
                        external: v.external_memory,
                    }
                },
                Err(_) => {
                    error!("isolate memory usage sender dropped");
                    WorkerMemoryUsed {
                        total: 0,
                        heap: 0,
                        external: 0,
                    }
                }
            };

            // send termination reason
            let termination_event = WorkerEvents::Shutdown(ShutdownEvent{
                reason,
                memory_used,
                cpu_time_used: 0, // this will be set later
            });
            let _ = termination_event_tx.send(termination_event);
        })
        .unwrap();

    Ok(cputimer)
}

pub async fn create_worker(
    init_opts: WorkerContextInitOpts,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let (worker_boot_result_tx, worker_boot_result_rx) = oneshot::channel::<Result<(), Error>>();
    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
    let worker_init = Worker::new(&init_opts)?;

    let worker: Box<dyn WorkerHandler> = Box::new(worker_init);

    // Downcast to call the method in "Worker" since the implementation might be of worker
    // But at the end we are using the trait itself.
    // Downcasting it to Worker will give us access to its parent implementation
    let downcast_reference = worker.as_any().downcast_ref::<Worker>();
    if let Some(worker_struct_ref) = downcast_reference {
        worker_struct_ref.start(init_opts, unix_stream_rx, worker_boot_result_tx);

        // create an async task waiting for requests for worker
        let (worker_req_tx, mut worker_req_rx) = mpsc::unbounded_channel::<WorkerRequestMsg>();

        let worker_req_handle: tokio::task::JoinHandle<Result<(), Error>> =
            tokio::task::spawn(async move {
                while let Some(msg) = worker_req_rx.recv().await {
                    let unix_stream_tx_clone = unix_stream_tx.clone();
                    tokio::task::spawn(async move {
                        if let Err(err) = handle_request(unix_stream_tx_clone, msg).await {
                            error!("worker failed to handle request: {:?}", err);
                        }
                    });
                }

                Ok(())
            });

        // wait for worker to be successfully booted
        let worker_boot_result = worker_boot_result_rx.await?;

        match worker_boot_result {
            Err(err) => {
                worker_req_handle.abort();
                bail!(err)
            }
            Ok(_) => {
                let elapsed = worker_struct_ref
                    .worker_boot_start_time
                    .elapsed()
                    .as_millis();
                send_event_if_event_worker_available(
                    worker_struct_ref.events_msg_tx.clone(),
                    WorkerEvents::Boot(BootEvent {
                        boot_time: elapsed as usize,
                    }),
                    worker_struct_ref.event_metadata.clone(),
                );
                Ok(worker_req_tx)
            }
        }
    } else {
        bail!("Unknown")
    }
}

pub async fn send_user_worker_request(
    worker_request_msg_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
    let msg = WorkerRequestMsg { req, res_tx };

    // send the message to worker
    worker_request_msg_tx.send(msg)?;

    // wait for the response back from the worker
    let res = res_rx.await??;

    // send the response back to the caller

    Ok(res)
}

pub async fn create_main_worker(
    main_worker_path: PathBuf,
    import_map_path: Option<String>,
    no_module_cache: bool,
    user_worker_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    maybe_entrypoint: Option<String>,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let mut service_path = main_worker_path.clone();
    let mut maybe_eszip = None;
    if let Some(ext) = main_worker_path.extension() {
        if ext == "eszip" {
            service_path = main_worker_path.parent().unwrap().to_path_buf();
            maybe_eszip = Some(EszipPayloadKind::VecKind(std::fs::read(main_worker_path)?));
        }
    }

    let main_worker_req_tx = create_worker(WorkerContextInitOpts {
        service_path,
        import_map_path,
        no_module_cache,
        events_rx: None,
        maybe_eszip,
        maybe_entrypoint,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
            worker_pool_tx: user_worker_msgs_tx,
        }),
        env_vars: std::env::vars().collect(),
    })
    .await
    .map_err(|err| anyhow!("main worker boot error: {}", err))?;

    Ok(main_worker_req_tx)
}

pub async fn create_events_worker(
    events_worker_path: PathBuf,
    import_map_path: Option<String>,
    no_module_cache: bool,
    maybe_entrypoint: Option<String>,
) -> Result<mpsc::UnboundedSender<WorkerEventWithMetadata>, Error> {
    let (events_tx, events_rx) = mpsc::unbounded_channel::<WorkerEventWithMetadata>();

    let mut service_path = events_worker_path.clone();
    let mut maybe_eszip = None;
    if let Some(ext) = events_worker_path.extension() {
        if ext == "eszip" {
            service_path = events_worker_path.parent().unwrap().to_path_buf();
            maybe_eszip = Some(EszipPayloadKind::VecKind(std::fs::read(
                events_worker_path,
            )?));
        }
    }

    let _ = create_worker(WorkerContextInitOpts {
        service_path,
        no_module_cache,
        import_map_path,
        env_vars: std::env::vars().collect(),
        events_rx: Some(events_rx),
        maybe_eszip,
        maybe_entrypoint,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::EventsWorker(EventWorkerRuntimeOpts {}),
    })
    .await
    .map_err(|err| anyhow!("events worker boot error: {}", err))?;

    Ok(events_tx)
}

pub async fn create_user_worker_pool(
    worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
) -> Result<mpsc::UnboundedSender<UserWorkerMsgs>, Error> {
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    let user_worker_msgs_tx_clone = user_worker_msgs_tx.clone();

    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        let mut worker_pool = WorkerPool::new(worker_event_sender, user_worker_msgs_tx_clone);

        // Note: Keep this loop non-blocking. Spawn a task to run blocking calls.
        // Handle errors within tasks and log them - do not bubble up errors.
        loop {
            match user_worker_msgs_rx.recv().await {
                None => break,
                Some(UserWorkerMsgs::Create(worker_options, tx)) => {
                    worker_pool.create_user_worker(worker_options, tx);
                }
                Some(UserWorkerMsgs::Created(key, profile)) => {
                    worker_pool.add_user_worker(key, profile);
                }
                Some(UserWorkerMsgs::SendRequest(key, req, res_tx)) => {
                    worker_pool.send_request(&key, req, res_tx);
                }
                Some(UserWorkerMsgs::Retire(key)) => {
                    worker_pool.retire(&key);
                }
                Some(UserWorkerMsgs::Shutdown(key)) => {
                    worker_pool.shutdown(&key);
                }
            }
        }

        Ok(())
    });

    Ok(user_worker_msgs_tx)
}
