use crate::deno_runtime::DenoRuntime;
use crate::utils::send_event_if_event_worker_available;
use crate::utils::units::bytes_to_display;

use crate::rt_worker::worker::{Worker, WorkerHandler};
use crate::rt_worker::worker_pool::WorkerPool;
use anyhow::{anyhow, bail, Error};
use cpu_timer::{CPUAlarmVal, CPUTimer};
use event_worker::events::{
    BootEvent, ShutdownEvent, WorkerEventWithMetadata, WorkerEvents, WorkerMemoryUsed,
};
use hyper::{Body, Request, Response};
use log::{debug, error};
use once_cell::sync::Lazy;
use sb_graph::EszipPayloadKind;
use sb_workers::context::{
    EventWorkerRuntimeOpts, MainWorkerRuntimeOpts, UserWorkerMsgs, WorkerContextInitOpts,
    WorkerRequestMsg, WorkerRuntimeOpts,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{mpsc, oneshot, Notify};
use uuid::Uuid;

use super::supervisor;
use super::worker_pool::{CPUTimerPolicy, WorkerPoolPolicy};

static SUPERVISOR_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sb-supervisor")
        .build()
        .unwrap()
});

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
            error!("Error in worker connection: {}", e.message());
        }
    });

    tokio::task::yield_now().await;

    let result = request_sender.send_request(msg.req).await;
    let _ = msg.res_tx.send(result);

    Ok(())
}

pub fn create_supervisor(
    key: Uuid,
    worker_runtime: &mut DenoRuntime,
    cpu_timer_policy: CPUTimerPolicy,
    termination_event_tx: oneshot::Sender<WorkerEvents>,
    pool_msg_tx: Option<UnboundedSender<UserWorkerMsgs>>,
    cancel: Option<Arc<Notify>>,
    timing_rx_pair: Option<(UnboundedReceiver<()>, UnboundedReceiver<()>)>,
) -> Result<Option<CPUTimer>, Error> {
    let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<()>();
    let (waker, thread_safe_handle) = {
        let js_runtime = &mut worker_runtime.js_runtime;
        (
            js_runtime.op_state().borrow().waker.clone(),
            js_runtime.v8_isolate().thread_safe_handle(),
        )
    };

    let (req_start_rx, req_end_rx) = if let Some((start, end)) = timing_rx_pair {
        (start, end)
    } else {
        let (_, dumb_start_rx) = unbounded_channel::<()>();
        let (_, dumb_end_rx) = unbounded_channel::<()>();

        (dumb_start_rx, dumb_end_rx)
    };

    // we assert supervisor is only run for user workers
    let conf = worker_runtime.conf.as_user_worker().unwrap().clone();
    let cancel = cancel.unwrap();

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
    let (cpu_timer, cpu_alarms_rx) = {
        let (cpu_alarms_tx, cpu_alarms_rx) = mpsc::unbounded_channel::<()>();

        (
            if !cfg!(test) && conf.cpu_time_soft_limit_ms != 0 && conf.cpu_time_hard_limit_ms != 0 {
                Some(CPUTimer::start(
                    if cpu_timer_policy.is_per_worker() {
                        conf.cpu_time_soft_limit_ms
                    } else {
                        conf.cpu_time_hard_limit_ms
                    },
                    conf.cpu_time_hard_limit_ms,
                    CPUAlarmVal { cpu_alarms_tx },
                )?)
            } else {
                None
            },
            cpu_alarms_rx,
        )
    };

    let cpu_timer_inner = cpu_timer.clone();
    let _rt_guard = SUPERVISOR_RT.enter();
    let _ = tokio::spawn(async move {
        let (isolate_memory_usage_tx, isolate_memory_usage_rx) =
            oneshot::channel::<supervisor::IsolateMemoryStats>();

        let args = supervisor::Arguments {
            key,
            runtime_opts: conf.clone(),
            cpu_timer: cpu_timer_inner,
            cpu_timer_policy,
            cpu_alarms_rx,
            req_start_rx,
            req_end_rx,
            memory_limit_rx,
            pool_msg_tx,
            isolate_memory_usage_tx,
            thread_safe_handle,
            waker: waker.clone(),
        };

        let reason = {
            use supervisor::*;
            match cpu_timer_policy {
                CPUTimerPolicy::PerWorker => strategy_per_worker::supervise(args).await,
                CPUTimerPolicy::PerRequest => strategy_per_request::supervise(args).await,
            }
        };

        cancel.notify_waiters();

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

        let memory_used = match isolate_memory_usage_rx.await {
            Ok(v) => WorkerMemoryUsed {
                total: v.used_heap_size + v.external_memory,
                heap: v.used_heap_size,
                external: v.external_memory,
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
        let termination_event = WorkerEvents::Shutdown(ShutdownEvent {
            reason,
            memory_used,
            cpu_time_used: 0, // this will be set later
        });

        let _ = termination_event_tx.send(termination_event);
    });

    Ok(cpu_timer)
}

pub struct CreateWorkerArgs(WorkerContextInitOpts, Option<CPUTimerPolicy>);

impl From<WorkerContextInitOpts> for CreateWorkerArgs {
    fn from(val: WorkerContextInitOpts) -> Self {
        CreateWorkerArgs(val, None)
    }
}

impl From<(WorkerContextInitOpts, CPUTimerPolicy)> for CreateWorkerArgs {
    fn from(val: (WorkerContextInitOpts, CPUTimerPolicy)) -> Self {
        CreateWorkerArgs(val.0, Some(val.1))
    }
}

pub async fn create_worker<Opt: Into<CreateWorkerArgs>>(
    init_opts: Opt,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let (worker_boot_result_tx, worker_boot_result_rx) = oneshot::channel::<Result<(), Error>>();
    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
    let CreateWorkerArgs(init_opts, maybe_cpu_timer_policy) = init_opts.into();
    let mut worker_init = Worker::new(&init_opts)?;

    if init_opts.conf.is_user_worker() {
        worker_init.set_cpu_timer_policy(maybe_cpu_timer_policy);
    }

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
    cancel: Arc<Notify>,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
    let msg = WorkerRequestMsg { req, res_tx };

    // send the message to worker
    worker_request_msg_tx.send(msg)?;

    // wait for the response back from the worker
    let res = tokio::select! {
        () = cancel.notified() => bail!("request has been cancelled"),
        res = res_rx => res,
    }??;

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
        timing_rx_pair: None,
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
        timing_rx_pair: None,
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
    policy: WorkerPoolPolicy,
    worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
) -> Result<mpsc::UnboundedSender<UserWorkerMsgs>, Error> {
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    let user_worker_msgs_tx_clone = user_worker_msgs_tx.clone();

    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        let mut worker_pool =
            WorkerPool::new(policy, worker_event_sender, user_worker_msgs_tx_clone);

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
                Some(UserWorkerMsgs::Idle(key)) => {
                    worker_pool.idle(&key);
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
