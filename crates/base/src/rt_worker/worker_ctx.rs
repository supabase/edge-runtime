use crate::deno_runtime::DenoRuntime;
use crate::inspector_server::Inspector;
use crate::utils::send_event_if_event_worker_available;
use crate::utils::units::bytes_to_display;

use crate::rt_worker::worker::{Worker, WorkerHandler};
use crate::rt_worker::worker_pool::WorkerPool;
use anyhow::{anyhow, bail, Error};
use cpu_timer::CPUTimer;
use deno_core::{InspectorSessionProxy, LocalInspectorSession};
use event_worker::events::{
    BootEvent, ShutdownEvent, WorkerEventWithMetadata, WorkerEvents, WorkerMemoryUsed,
};
use futures_util::pin_mut;
use http::StatusCode;
use http_utils::io::Upgraded2;
use http_utils::utils::{emit_status_code, get_upgrade_type};
use hyper::client::conn::http1;
use hyper::upgrade::OnUpgrade;
use hyper::{Body, Request, Response};
use log::{debug, error};
use sb_core::conn_sync::ConnSync;
use sb_core::{MetricSource, SharedMetricSource};
use sb_graph::EszipPayloadKind;
use sb_workers::context::{
    EventWorkerRuntimeOpts, MainWorkerRuntimeOpts, Timing, UserWorkerMsgs, WorkerContextInitOpts,
    WorkerRequestMsg, WorkerRuntimeOpts,
};
use sb_workers::errors::WorkerError;
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::copy_bidirectional;
use tokio::net::UnixStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{mpsc, oneshot, watch, Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::rt;
use super::supervisor::{self, CPUTimerParam, CPUUsageMetrics};
use super::worker::UnixStreamEntry;
use super::worker_pool::{SupervisorPolicy, WorkerPoolPolicy};

#[derive(Clone)]
pub struct TerminationToken {
    pub inbound: CancellationToken,
    pub outbound: CancellationToken,
}

impl std::fmt::Debug for TerminationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminationToken").finish()
    }
}

impl Default for TerminationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminationToken {
    pub fn new() -> Self {
        Self {
            inbound: CancellationToken::default(),
            outbound: CancellationToken::default(),
        }
    }

    pub fn child_token(&self) -> Self {
        Self {
            inbound: self.inbound.child_token(),
            outbound: self.outbound.clone(),
        }
    }

    pub fn cancel(&self) {
        self.inbound.cancel();
    }

    pub async fn cancel_and_wait(&self) {
        self.cancel();
        self.outbound.cancelled().await;
    }
}

async fn handle_request(
    unix_stream_tx: mpsc::UnboundedSender<UnixStreamEntry>,
    msg: WorkerRequestMsg,
) -> Result<(), Error> {
    // create a unix socket pair
    let (sender_stream, recv_stream) = UnixStream::pair()?;
    let WorkerRequestMsg {
        mut req,
        res_tx,
        conn_watch,
    } = msg;

    let _ = unix_stream_tx.send((recv_stream, conn_watch.clone()));
    let req_upgrade_type = get_upgrade_type(req.headers());
    let req_upgrade = req_upgrade_type
        .clone()
        .and_then(|it| Some(it).zip(req.extensions_mut().remove::<OnUpgrade>()));

    // send the HTTP request to the worker over Unix stream
    let (mut request_sender, connection) = http1::Builder::new()
        .writev(true)
        .handshake(sender_stream)
        .await?;

    let (upgrade_tx, upgrade_rx) = oneshot::channel();

    // spawn a task to poll the connection and drive the HTTP state
    tokio::task::spawn({
        async move {
            match connection.without_shutdown().await {
                Err(e) => {
                    error!("Error in worker connection: {}", e.message());
                }

                Ok(parts) => {
                    if let Some((requested, req_upgrade)) = req_upgrade {
                        if let Ok((Some(accepted), status)) = upgrade_rx.await {
                            if status == StatusCode::SWITCHING_PROTOCOLS && accepted == requested {
                                tokio::spawn(relay_upgraded_request_and_response(
                                    req_upgrade,
                                    parts,
                                ));

                                return;
                            }
                        };
                    }

                    if let Some(mut watcher) = conn_watch {
                        if watcher.wait_for(|it| *it == ConnSync::Recv).await.is_err() {
                            error!("cannot track outbound connection correctly");
                        }
                    }
                }
            }
        }
    });

    tokio::task::yield_now().await;

    let res = request_sender.send_request(req).await;
    let Ok(res) = res else {
        drop(res_tx.send(res));
        return Ok(());
    };

    if let Some(requested) = req_upgrade_type {
        let res_upgrade_type = get_upgrade_type(res.headers());
        let _ = upgrade_tx.send((res_upgrade_type.clone(), res.status()));

        match res_upgrade_type {
            Some(accepted) if accepted == requested => {}
            _ => {
                drop(res_tx.send(Ok(emit_status_code(StatusCode::BAD_GATEWAY))));
                return Ok(());
            }
        }
    }

    drop(res_tx.send(Ok(res)));
    Ok(())
}

async fn relay_upgraded_request_and_response(
    downstream: OnUpgrade,
    parts: http1::Parts<UnixStream>,
) {
    let mut upstream = Upgraded2::new(parts.io, parts.read_buf);
    let mut downstream = downstream.await.expect("failed to upgrade request");

    copy_bidirectional(&mut upstream, &mut downstream)
        .await
        .expect("coping between upgraded connections failed");

    // XXX(Nyannyacha): Here you might want to emit the event metadata.
}

#[allow(clippy::too_many_arguments)]
pub fn create_supervisor(
    key: Uuid,
    worker_runtime: &mut DenoRuntime,
    supervisor_policy: SupervisorPolicy,
    termination_event_tx: oneshot::Sender<WorkerEvents>,
    pool_msg_tx: Option<UnboundedSender<UserWorkerMsgs>>,
    cpu_usage_metrics_rx: Option<UnboundedReceiver<CPUUsageMetrics>>,
    cancel: Option<Arc<Notify>>,
    timing: Option<Timing>,
    termination_token: Option<TerminationToken>,
) -> Result<(Option<CPUTimer>, CancellationToken), Error> {
    let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<()>();
    let (waker, thread_safe_handle) = {
        let js_runtime = &mut worker_runtime.js_runtime;
        (
            js_runtime.op_state().borrow().waker.clone(),
            js_runtime.v8_isolate().thread_safe_handle(),
        )
    };

    // we assert supervisor is only run for user workers
    let conf = worker_runtime.conf.as_user_worker().unwrap().clone();
    let is_termination_requested = worker_runtime.is_termination_requested.clone();

    let giveup_process_requests_token = cancel.clone();
    let supervise_cancel_token = CancellationToken::new();
    let tokens = supervisor::Tokens {
        termination: termination_token,
        supervise: supervise_cancel_token.clone(),
    };

    let maybe_inspector_params = worker_runtime.inspector().map(|_| {
        (
            worker_runtime
                .js_runtime
                .inspector()
                .borrow_mut()
                .get_session_sender(),
            worker_runtime.is_terminated.clone(),
            worker_runtime.is_found_inspector_session.clone(),
        )
    });

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

    let cpu_timer_param =
        CPUTimerParam::new(conf.cpu_time_soft_limit_ms, conf.cpu_time_hard_limit_ms);

    let (maybe_cpu_timer, maybe_cpu_alarms_rx) =
        cpu_timer_param.get_cpu_timer(supervisor_policy).unzip();

    drop({
        let _rt_guard = rt::SUPERVISOR_RT.enter();
        let maybe_cpu_timer_inner = maybe_cpu_timer.clone();
        let supervise_cancel_token_inner = supervise_cancel_token.clone();

        tokio::spawn(async move {
            let (isolate_memory_usage_tx, isolate_memory_usage_rx) =
                oneshot::channel::<supervisor::IsolateMemoryStats>();

            let args = supervisor::Arguments {
                key,
                runtime_opts: conf.clone(),
                cpu_timer: maybe_cpu_timer_inner.zip(maybe_cpu_alarms_rx),
                cpu_usage_metrics_rx,
                cpu_timer_param,
                supervisor_policy,
                timing,
                memory_limit_rx,
                pool_msg_tx,
                isolate_memory_usage_tx,
                thread_safe_handle,
                waker: waker.clone(),
                tokens,
            };

            let (reason, cpu_usage_ms) = {
                use supervisor::*;
                match supervisor_policy {
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
                cancel.notify_waiters();
            }

            if let Some((session_tx, is_terminated, is_found)) = maybe_inspector_params {
                use deno_core::futures::channel::mpsc;
                use deno_core::serde_json::Value;

                rt::SUPERVISOR_RT
                    .spawn_blocking(move || {
                        let wait_inspector_disconnect_fut = async move {
                            let ls = tokio::task::LocalSet::new();
                            ls.run_until(async move {
                                if is_terminated.is_raised() || is_termination_requested.is_raised()
                                {
                                    return;
                                }

                                is_termination_requested.raise();

                                if is_found.is_raised() {
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
                                        let is_terminated = is_terminated.clone();
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
                                                        if is_terminated.is_raised() {
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

                        rt::SUPERVISOR_RT.block_on(wait_inspector_disconnect_fut);
                    })
                    .await
                    .unwrap();
            } else {
                is_termination_requested.raise();
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

            let memory_used = match isolate_memory_usage_rx.await {
                Ok(v) => WorkerMemoryUsed {
                    total: v.used_heap_size + v.external_memory,
                    heap: v.used_heap_size,
                    external: v.external_memory,
                },
                Err(_) => {
                    if !supervise_cancel_token_inner.is_cancelled() {
                        error!("isolate memory usage sender dropped");
                    }

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
                cpu_time_used: cpu_usage_ms as usize,
            });

            let _ = termination_event_tx.send(termination_event);
        })
    });

    Ok((maybe_cpu_timer, supervise_cancel_token))
}

pub struct CreateWorkerArgs(
    WorkerContextInitOpts,
    Option<SupervisorPolicy>,
    Option<TerminationToken>,
);

impl From<WorkerContextInitOpts> for CreateWorkerArgs {
    fn from(val: WorkerContextInitOpts) -> Self {
        CreateWorkerArgs(val, None, None)
    }
}

impl From<(WorkerContextInitOpts, SupervisorPolicy)> for CreateWorkerArgs {
    fn from(val: (WorkerContextInitOpts, SupervisorPolicy)) -> Self {
        CreateWorkerArgs(val.0, Some(val.1), None)
    }
}

impl<T: Into<Option<TerminationToken>>> From<(WorkerContextInitOpts, T)> for CreateWorkerArgs {
    fn from(val: (WorkerContextInitOpts, T)) -> Self {
        CreateWorkerArgs(val.0, None, val.1.into())
    }
}

impl
    From<(
        WorkerContextInitOpts,
        SupervisorPolicy,
        Option<TerminationToken>,
    )> for CreateWorkerArgs
{
    fn from(
        val: (
            WorkerContextInitOpts,
            SupervisorPolicy,
            Option<TerminationToken>,
        ),
    ) -> Self {
        CreateWorkerArgs(val.0, Some(val.1), val.2)
    }
}

impl CreateWorkerArgs {
    pub fn with_supervisor_policy(mut self, policy: SupervisorPolicy) -> Self {
        self.1 = Some(policy);
        self
    }

    pub fn with_termination_token(mut self, token: TerminationToken) -> Self {
        self.2 = Some(token);
        self
    }
}

pub async fn create_worker<Opt: Into<CreateWorkerArgs>>(
    init_opts: Opt,
    inspector: Option<Inspector>,
) -> Result<(MetricSource, mpsc::UnboundedSender<WorkerRequestMsg>), Error> {
    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStreamEntry>();
    let (worker_boot_result_tx, worker_boot_result_rx) =
        oneshot::channel::<Result<MetricSource, Error>>();

    let CreateWorkerArgs(init_opts, maybe_supervisor_policy, maybe_termination_token) =
        init_opts.into();

    let mut worker_init = Worker::new(&init_opts)?;

    if init_opts.conf.is_user_worker() {
        worker_init.set_supervisor_policy(maybe_supervisor_policy);
    }

    let worker: Box<dyn WorkerHandler> = Box::new(worker_init);

    // Downcast to call the method in "Worker" since the implementation might be of worker
    // But at the end we are using the trait itself.
    // Downcasting it to Worker will give us access to its parent implementation
    let downcast_reference = worker.as_any().downcast_ref::<Worker>();

    if let Some(worker_struct_ref) = downcast_reference {
        worker_struct_ref.start(
            init_opts,
            (unix_stream_tx.clone(), unix_stream_rx),
            worker_boot_result_tx,
            maybe_termination_token.clone(),
            inspector,
        );

        // create an async task waiting for requests for worker
        let (worker_req_tx, mut worker_req_rx) = mpsc::unbounded_channel::<WorkerRequestMsg>();

        let worker_req_handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::task::spawn({
            let stream_tx = unix_stream_tx;
            async move {
                while let Some(msg) = worker_req_rx.recv().await {
                    tokio::task::spawn({
                        let stream_tx_inner = stream_tx.clone();
                        async move {
                            if let Err(err) = handle_request(stream_tx_inner, msg).await {
                                error!("worker failed to handle request: {:?}", err);
                            }
                        }
                    });
                }

                Ok(())
            }
        });

        // wait for worker to be successfully booted
        let worker_boot_result = worker_boot_result_rx.await?;

        match worker_boot_result {
            Err(err) => {
                worker_req_handle.abort();

                if let Some(token) = maybe_termination_token.as_ref() {
                    token.outbound.cancel();
                }

                bail!(err)
            }
            Ok(metric) => {
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

                Ok((metric, worker_req_tx))
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
    conn_watch: Option<watch::Receiver<ConnSync>>,
) -> Result<Response<Body>, Error> {
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_watch,
    };

    // send the message to worker
    worker_request_msg_tx.send(msg)?;

    // wait for the response back from the worker
    let res = tokio::select! {
        () = cancel.notified() => bail!(WorkerError::RequestCancelledBySupervisor),
        res = res_rx => res,
    }??;

    // send the response back to the caller
    Ok(res)
}

pub async fn create_main_worker(
    main_worker_path: PathBuf,
    import_map_path: Option<String>,
    no_module_cache: bool,
    runtime_opts: MainWorkerRuntimeOpts,
    maybe_entrypoint: Option<String>,
    termination_token: Option<TerminationToken>,
    inspector: Option<Inspector>,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let mut service_path = main_worker_path.clone();
    let mut maybe_eszip = None;
    if let Some(ext) = main_worker_path.extension() {
        if ext == "eszip" {
            service_path = main_worker_path.parent().unwrap().to_path_buf();
            maybe_eszip = Some(EszipPayloadKind::VecKind(std::fs::read(main_worker_path)?));
        }
    }

    let (_, sender) = create_worker(
        (
            WorkerContextInitOpts {
                service_path,
                import_map_path,
                no_module_cache,
                events_rx: None,
                timing: None,
                maybe_eszip,
                maybe_entrypoint,
                maybe_module_code: None,
                conf: WorkerRuntimeOpts::MainWorker(runtime_opts),
                env_vars: std::env::vars().collect(),
            },
            termination_token,
        ),
        inspector,
    )
    .await
    .map_err(|err| anyhow!("main worker boot error: {}", err))?;

    Ok(sender)
}

pub async fn create_events_worker(
    events_worker_path: PathBuf,
    import_map_path: Option<String>,
    no_module_cache: bool,
    maybe_entrypoint: Option<String>,
    termination_token: Option<TerminationToken>,
) -> Result<(MetricSource, mpsc::UnboundedSender<WorkerEventWithMetadata>), Error> {
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

    let (metric, _) = create_worker(
        (
            WorkerContextInitOpts {
                service_path,
                no_module_cache,
                import_map_path,
                env_vars: std::env::vars().collect(),
                events_rx: Some(events_rx),
                timing: None,
                maybe_eszip,
                maybe_entrypoint,
                maybe_module_code: None,
                conf: WorkerRuntimeOpts::EventsWorker(EventWorkerRuntimeOpts {}),
            },
            termination_token,
        ),
        None,
    )
    .await
    .map_err(|err| anyhow!("events worker boot error: {}", err))?;

    Ok((metric, events_tx))
}

pub async fn create_user_worker_pool(
    policy: WorkerPoolPolicy,
    worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    termination_token: Option<TerminationToken>,
    inspector: Option<Inspector>,
) -> Result<(SharedMetricSource, mpsc::UnboundedSender<UserWorkerMsgs>), Error> {
    let metric_src = SharedMetricSource::default();
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    let user_worker_msgs_tx_clone = user_worker_msgs_tx.clone();

    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::spawn({
        let metric_src_inner = metric_src.clone();
        async move {
            let token = termination_token.as_ref();
            let mut termination_requested = false;
            let mut worker_pool = WorkerPool::new(
                policy,
                metric_src_inner,
                worker_event_sender,
                user_worker_msgs_tx_clone,
                inspector,
            );

            // Note: Keep this loop non-blocking. Spawn a task to run blocking calls.
            // Handle errors within tasks and log them - do not bubble up errors.
            loop {
                tokio::select! {
                    _ = async {
                        if let Some(token) = token {
                            token.inbound.cancelled().await;
                        } else {
                            pending::<()>().await;
                        }
                    }, if !termination_requested => {
                        termination_requested = true;

                        if worker_pool.user_workers.is_empty() {
                            if let Some(token) = token {
                                token.outbound.cancel();
                            }

                            break;
                        }
                    }

                    msg = user_worker_msgs_rx.recv() => {
                        match msg {
                            None => break,
                            Some(UserWorkerMsgs::Create(worker_options, tx)) => {
                                worker_pool.create_user_worker(worker_options, tx, termination_token.as_ref().map(|it| it.child_token()));
                            }
                            Some(UserWorkerMsgs::Created(key, profile)) => {
                                worker_pool.add_user_worker(key, profile);
                            }
                            Some(UserWorkerMsgs::SendRequest(key, req, res_tx, conn_watch)) => {
                                worker_pool.send_request(&key, req, res_tx, conn_watch);
                            }
                            Some(UserWorkerMsgs::Idle(key)) => {
                                worker_pool.idle(&key);
                            }
                            Some(UserWorkerMsgs::Shutdown(key)) => {
                                worker_pool.shutdown(&key);

                                if let Some(token) = token {
                                    if token.inbound.is_cancelled() && worker_pool.user_workers.is_empty() {
                                        token.outbound.cancel();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    });

    Ok((metric_src, user_worker_msgs_tx))
}
