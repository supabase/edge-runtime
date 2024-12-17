use crate::inspector_server::Inspector;
use crate::worker::pool::WorkerPoolPolicy;
use crate::worker::{self, TerminationToken};
use crate::InspectorOption;
use anyhow::{anyhow, bail, Context, Error};
use deno_config::JsxImportSourceConfig;
use either::Either::{self, Left, Right};
use enum_as_inner::EnumAsInner;
use futures_util::future::{poll_fn, BoxFuture};
use futures_util::{FutureExt, Stream};
use graph::DecoratorType;
use hyper_v014::{server::conn::Http, service::Service, Body, Request, Response};
use log::{debug, error, info, trace, warn};
use rustls_pemfile::read_one_from_slice;
use rustls_pemfile::Item;
use sb_core::SharedMetricSource;
use sb_workers::context::WorkerRequestMsg;
use std::future::{pending, Future};
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::pin::Pin;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tls_listener::TlsListener;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::pin;
use tokio::sync::mpsc::{Sender, UnboundedSender};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use url::Url;

mod signal {
    pub use tokio::signal::ctrl_c;

    #[cfg(unix)]
    pub use tokio::signal::unix;
}

pub enum ServerEvent {
    ConnectionError(hyper_v014::Error),
    #[cfg(debug_assertions)]
    Draining,
}

#[derive(Debug, EnumAsInner)]
pub enum ServerHealth {
    Listening(mpsc::UnboundedReceiver<ServerEvent>, SharedMetricSource),
    Failure,
}

struct CancelOnDrop<S> {
    inner: S,
    cancel: Option<CancellationToken>,
}

impl<S> Drop for CancelOnDrop<S> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

impl<S: Stream + Unpin> Stream for CancelOnDrop<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.as_mut().inner).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

#[derive(Clone)]
struct TerminationTokens {
    input: Option<TerminationToken>,
    event: Option<TerminationToken>,
    pool: TerminationToken,
    main: TerminationToken,
}

impl TerminationTokens {
    fn new(maybe_input: Option<TerminationToken>, with_event: bool) -> Self {
        Self {
            input: maybe_input,
            event: with_event.then(TerminationToken::new),
            pool: TerminationToken::new(),
            main: TerminationToken::new(),
        }
    }

    async fn terminate(&self) {
        self.pool.cancel_and_wait().await;
        self.main.cancel_and_wait().await;

        if let Some(token) = self.event.as_ref() {
            token.cancel_and_wait().await;
        }

        if let Some(token) = self.input.as_ref() {
            assert!(token.inbound.is_cancelled());

            if !token.outbound.is_cancelled() {
                token.outbound.cancel();
            }
        }
    }
}

struct WorkerService {
    metric_src: SharedMetricSource,
    worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    cancel: CancellationToken,
}

impl WorkerService {
    fn new(
        metric_src: SharedMetricSource,
        worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    ) -> (Self, CancellationToken) {
        let cancel = CancellationToken::new();
        (
            Self {
                metric_src,
                worker_req_tx,
                cancel: cancel.clone(),
            },
            cancel,
        )
    }
}

impl Service<Request<Body>> for WorkerService {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // create a response in a future.
        let cancel = self.cancel.child_token();
        let metric_src = self.metric_src.clone();
        let worker_req_tx = self.worker_req_tx.clone();
        let fut = async move {
            let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper_v014::Error>>();

            let req_uri = req.uri().clone();
            let msg = WorkerRequestMsg {
                req,
                res_tx,
                conn_token: Some(cancel.clone()),
            };

            worker_req_tx.send(msg)?;
            metric_src.incl_received_requests();

            tokio::spawn({
                let metric_src_inner = metric_src.clone();
                let cancel = cancel.clone();

                async move {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            metric_src_inner.incl_handled_requests();
                        }
                        // TODO: I think it would be good to introduce the hard
                        // timeout here to prevent the requester's inability to get
                        // the response for any reason.
                    }
                }
            });

            let res = match res_rx.await {
                Ok(res) => res,
                Err(err) => {
                    metric_src.incl_handled_requests();
                    return Err(err.into());
                }
            };

            let res = match res {
                Ok(res) => {
                    let (parts, body) = res.into_parts();
                    Response::from_parts(
                        parts,
                        Body::wrap_stream(CancelOnDrop {
                            inner: body,
                            cancel: Some(cancel),
                        }),
                    )
                }

                Err(e) => {
                    error!(
                        "request failed (uri: {:?} reason: {:?})",
                        req_uri.to_string(),
                        e
                    );

                    // FIXME: add an error body
                    Response::builder()
                        .status(http_v02::StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::wrap_stream(CancelOnDrop {
                            inner: Body::empty(),
                            cancel: Some(cancel),
                        }))
                        .unwrap()
                }
            };

            Ok(res)
        };

        // Return the response as an immediate future
        Box::pin(fut)
    }
}

#[derive(Default)]
pub struct WorkerEntrypoints {
    pub main: Option<String>,
    pub events: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServerFlags {
    pub no_module_cache: bool,
    pub allow_main_inspector: bool,
    pub tcp_nodelay: bool,

    pub graceful_exit_deadline_sec: u64,
    pub graceful_exit_keepalive_deadline_ms: Option<u64>,
    pub event_worker_exit_deadline_sec: u64,
    pub request_wait_timeout_ms: Option<u64>,
    pub request_idle_timeout_ms: Option<u64>,
    pub request_read_timeout_ms: Option<u64>,
    pub request_buffer_size: Option<u64>,

    pub beforeunload_wall_clock_pct: Option<u8>,
    pub beforeunload_cpu_pct: Option<u8>,
    pub beforeunload_memory_pct: Option<u8>,
}

#[derive(Debug)]
pub struct Tls {
    port: u16,
    key: PrivateKeyDer<'static>,
    cert_chain: Vec<CertificateDer<'static>>,
}

impl Clone for Tls {
    fn clone(&self) -> Self {
        Self {
            port: self.port,
            key: self.key.clone_key(),
            cert_chain: self.cert_chain.clone(),
        }
    }
}

impl Tls {
    pub fn new(port: u16, key: &[u8], cert: &[u8]) -> anyhow::Result<Self> {
        let Some((key_item, _)) =
            read_one_from_slice(key).map_err(|err| anyhow!("can't resolve key: {:?}", err))?
        else {
            bail!("invalid key data")
        };

        let mut cert_chain = vec![];
        let mut cert_slice = cert;
        loop {
            let Some((Item::X509Certificate(cert), remain_cert_slice)) =
                read_one_from_slice(cert_slice)
                    .map_err(|err| anyhow!("can't resolve cert: {:?}", err))?
            else {
                bail!("invalid cert data")
            };

            cert_chain.push(cert);

            if remain_cert_slice.is_empty() {
                break;
            }

            cert_slice = remain_cert_slice;
        }

        let key = match key_item {
            Item::Pkcs1Key(key) => PrivateKeyDer::Pkcs1(key),
            Item::Pkcs8Key(key) => PrivateKeyDer::Pkcs8(key),
            Item::Sec1Key(key) => PrivateKeyDer::Sec1(key),
            _ => bail!("invalid key data"),
        };

        Ok(Self {
            port,
            key,
            cert_chain,
        })
    }

    fn into_acceptor(self) -> anyhow::Result<TlsAcceptor> {
        Ok(Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(self.cert_chain, self.key)
                .with_context(|| "can't make TLS acceptor")?,
        )
        .into())
    }
}

pub type SignumOrExitCode = Either<i32, std::process::ExitCode>;

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    tls: Option<Tls>,
    main_worker_surface: worker::MainWorkerSurface,
    callback_tx: Option<Sender<ServerHealth>>,
    termination_tokens: TerminationTokens,
    flags: Arc<ServerFlags>,
    metric_src: SharedMetricSource,
}

impl Server {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ip: &str,
        port: u16,
        tls: Option<Tls>,
        main_service_path: String,
        maybe_event_service_path: Option<String>,
        maybe_decorator: Option<DecoratorType>,
        maybe_user_worker_policy: Option<WorkerPoolPolicy>,
        import_map_path: Option<String>,
        flags: ServerFlags,
        callback_tx: Option<Sender<ServerHealth>>,
        entrypoints: WorkerEntrypoints,
        termination_token: Option<TerminationToken>,
        static_patterns: Vec<String>,
        inspector: Option<Inspector>,
        jsx_specifier: Option<String>,
        jsx_module: Option<String>,
    ) -> Result<Self, Error> {
        let flags = Arc::new(flags);
        let maybe_event_entrypoint = entrypoints.events;
        let maybe_main_entrypoint = entrypoints.main;
        let termination_tokens =
            TerminationTokens::new(termination_token, maybe_event_service_path.is_some());

        // create an event worker
        let event_worker_surface = if let Some(service_path) = maybe_event_service_path {
            let mut builder = worker::EventWorkerSurfaceBuilder::new(&service_path);

            builder
                .set_server_flags(Some(Left(flags.clone())))
                .set_termination_token(Some(termination_tokens.event.clone().unwrap()));

            builder
                .set_import_map_path(import_map_path.as_deref())
                .set_entrypoint(maybe_event_entrypoint.as_deref())
                .set_decorator(maybe_decorator);

            Some(builder.build().await?)
        } else {
            None
        };

        let jsx_config = jsx_module.map(|jsx_mod| JsxImportSourceConfig {
            default_specifier: jsx_specifier,
            default_types_specifier: None,
            module: jsx_mod,
            base_url: Url::from_file_path(std::env::current_dir().unwrap()).unwrap(),
        });

        // create a user worker pool
        let (shared_metric_src, worker_pool_tx) = worker::create_user_worker_pool(
            flags.clone(),
            maybe_user_worker_policy.unwrap_or_default(),
            event_worker_surface
                .as_ref()
                .map(|it| it.event_message_sender()),
            Some(termination_tokens.pool.clone()),
            static_patterns,
            inspector.clone(),
            jsx_config.clone(),
        )
        .await?;

        // create main worker
        let main_worker_surface = {
            let mut builder = worker::MainWorkerSurfaceBuilder::new(&main_service_path);

            builder
                .set_server_flags(Some(Left(flags.clone())))
                .set_termination_token(Some(termination_tokens.main.clone()));

            if flags.allow_main_inspector {
                builder.set_inspector(inspector.map(|it| Inspector {
                    option: InspectorOption::Inspect(it.option.socket_addr()),
                    server: it.server,
                }));
            }

            builder
                .set_import_map_path(import_map_path.as_deref())
                .set_entrypoint(maybe_main_entrypoint.as_deref())
                .set_decorator(maybe_decorator)
                .set_no_module_cache(Some(flags.no_module_cache))
                .set_jsx_import_source_config(jsx_config)
                .set_worker_pool_sender(Some(worker_pool_tx))
                .set_shared_metric_source(Some(shared_metric_src.clone()))
                .set_event_worker_metric_source(
                    event_worker_surface.as_ref().map(|it| it.metric.clone()),
                );

            builder.build().await?
        };

        Ok(Self {
            ip: Ipv4Addr::from_str(ip)?,
            port,
            tls,
            main_worker_surface,
            callback_tx,
            termination_tokens,
            flags,
            metric_src: shared_metric_src,
        })
    }

    pub async fn terminate(&self) {
        self.termination_tokens.terminate().await;
    }

    pub async fn listen(&mut self) -> Result<Option<SignumOrExitCode>, Error> {
        let addr = SocketAddr::new(IpAddr::V4(self.ip), self.port);
        let non_secure_listener = TcpListener::bind(&addr).await?;
        let mut secure_listener = if let Some(tls) = self.tls.take() {
            let addr = SocketAddr::new(IpAddr::V4(self.ip), tls.port);
            Some((
                TlsListener::new(tls.into_acceptor()?, TcpListener::bind(addr).await?),
                addr,
            ))
        } else {
            None
        };

        let metric_src = self.metric_src.clone();
        let termination_tokens = &self.termination_tokens;
        let input_termination_token = termination_tokens.input.as_ref();

        let mut ret = None::<SignumOrExitCode>;
        let mut can_receive_event = false;
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        debug!(
            "edge-runtime is listening on {:?}",
            non_secure_listener.local_addr()?
        );

        if let Some((_, addr)) = secure_listener.as_ref() {
            debug!("edge-runtime is listening on {:?} (secure)", addr);
        }

        if let Some(callback) = self.callback_tx.clone() {
            can_receive_event = true;
            let _ = callback
                .send(ServerHealth::Listening(event_rx, metric_src.clone()))
                .await;
        }

        let event_tx = can_receive_event.then_some(event_tx.clone());
        let graceful_exit_token = CancellationToken::new();
        let main_worker_cancel_token = self.main_worker_surface.cancel.clone();

        let ServerFlags {
            tcp_nodelay,
            request_read_timeout_ms,
            mut graceful_exit_deadline_sec,
            mut graceful_exit_keepalive_deadline_ms,
            ..
        } = *self.flags;

        let request_read_timeout_dur = request_read_timeout_ms.map(Duration::from_millis);
        let mut terminate_signal_fut = get_termination_signal();

        #[derive(Default, Clone, Copy, EnumAsInner)]
        enum LoopState {
            #[default]
            Normal,
            Interrupted,
            MainWorkerDestroyed,
        }

        let mut loop_state = LoopState::Normal;
        loop {
            let main_worker_req_tx = self.main_worker_surface.msg_tx.clone();
            let main_worker_cancel_fut = main_worker_cancel_token.cancelled();
            let event_tx = event_tx.clone();
            let metric_src = metric_src.clone();

            pin!(main_worker_cancel_fut);

            tokio::select! {
                msg = non_secure_listener.accept() => {
                    match msg {
                        Ok((stream, _)) => {
                            if tcp_nodelay {
                                let _ = stream.set_nodelay(true);
                            }

                            accept_stream(
                                stream,
                                main_worker_req_tx,
                                event_tx,
                                metric_src,
                                graceful_exit_token.clone(),
                                request_read_timeout_dur
                            )
                        }
                        Err(e) => error!("socket error: {}", e)
                    }
                }

                msg = async {
                    if let Some((listener, _addr)) = secure_listener.as_mut() {
                        listener.accept()
                    } else {
                        pending::<()>().await;
                        unreachable!();
                    }.await
                } => {
                    match msg {
                        Ok((stream, _)) => {
                            if tcp_nodelay {
                                let _ = stream.get_ref().0.set_nodelay(true);
                            }

                            accept_stream(
                                stream,
                                main_worker_req_tx,
                                event_tx,
                                metric_src,
                                graceful_exit_token.clone(),
                                request_read_timeout_dur
                            )
                        }
                        Err(e) => error!("socket error: {}", e)
                    }
                }

                _ = async move {
                    if let Some(token) = input_termination_token {
                        token.inbound.cancelled()
                    } else {
                        pending::<()>().await;
                        unreachable!();
                    }.await
                } => {
                    info!("termination token resolved");

                    if graceful_exit_deadline_sec == 0 {
                        graceful_exit_deadline_sec = u64::MAX;
                        graceful_exit_keepalive_deadline_ms = graceful_exit_keepalive_deadline_ms.map(|_| u64::MAX);
                    }

                    break;
                }

                _ = &mut main_worker_cancel_fut => {
                    error!("main worker has been destroyed");
                    loop_state = LoopState::MainWorkerDestroyed;
                    break;
                }

                signum = &mut terminate_signal_fut => {
                    info!("shutdown signal received: {}", signum);
                    ret = Some(Left(signum));
                    break;
                }

                _ = signal::ctrl_c() => {
                    info!("interrupt signal received");
                    loop_state = LoopState::Interrupted;
                    break;
                }
            }
        }

        if loop_state.is_main_worker_destroyed() {
            ret = Some(Right(std::process::ExitCode::FAILURE));

            let Some(err) = self.main_worker_surface.exit.error().await else {
                return Ok(ret);
            };

            error!("{}", format!("{err:?}"));

            return Ok(ret);
        }

        if !loop_state.is_interrupted() && graceful_exit_deadline_sec > 0 {
            static REQ_METRIC_CHECK_SLEEP_DUR: Duration = Duration::from_millis(10);

            let wait_fut = async move {
                #[cfg(debug_assertions)]
                {
                    if let Some(tx) = event_tx.as_ref() {
                        let _ = tx.send(ServerEvent::Draining);
                    }
                }

                if let Some(keepalive_deadline_ms) = graceful_exit_keepalive_deadline_ms {
                    // NOTE(Nyannyacha): What this branch does is not an
                    // implementation in the strict sense of the graceful close
                    // on the transport connection mentioned in RFC 2616 section
                    // 8.1.4.
                    //
                    // It pushes back the cancellation of the
                    // `graceful_exit_token` so that it can accept additional
                    // requests coming in over an already established keep-alive
                    // HTTP connection.
                    //
                    // However, this is non-standard behavior and therefore
                    // flagged as experimental.
                    //
                    // Backgrounds:
                    // [1]: https://datatracker.ietf.org/doc/html/rfc2616#autoid-59
                    // [2]: https://trac.nginx.org/nginx/ticket/1022
                    // [3]: https://mailman.nginx.org/pipermail/nginx-devel/2019-January/011810.html
                    // [4]: https://github.com/kubernetes-retired/contrib/issues/1123
                    // [5]: https://github.com/golang/go/issues/23829
                    // [6]: https://github.com/reactor/reactor-netty/issues/1247
                    // [7]: https://theantway.com/2017/11/analyze-connection-reset-error-in-nginx-upstream-with-keep-alive-enabled
                    // [8]: https://github.com/hyperium/hyper/issues/3553#issuecomment-1917170510

                    if keepalive_deadline_ms < u64::MAX {
                        tokio::spawn(async move {
                            sleep(Duration::from_millis(keepalive_deadline_ms)).await;
                            graceful_exit_token.cancel();
                        });
                    }
                } else {
                    graceful_exit_token.cancel();
                }

                loop {
                    let active_io = metric_src.active_io();
                    let received_request_count = metric_src.received_requests();
                    let handled_request_count = metric_src.handled_requests();

                    trace!(
                        "io: {}, received: {}, handled: {}",
                        active_io,
                        received_request_count,
                        handled_request_count
                    );

                    if active_io == 0 && received_request_count == handled_request_count {
                        break;
                    }

                    sleep(REQ_METRIC_CHECK_SLEEP_DUR).await;
                }

                termination_tokens.terminate().await;
            };

            let timeout_fut = timeout(Duration::from_secs(graceful_exit_deadline_sec), wait_fut);

            tokio::select! {
                res = timeout_fut => {
                    if res.is_err() {
                        error!(
                            "did not able to terminate the workers within {} seconds",
                            graceful_exit_deadline_sec,
                        );
                    }
                }

                _ = signal::ctrl_c() => {
                    error!("received interrupt signal while waiting workers");
                }
            }
        } else if !loop_state.is_interrupted()
            && metric_src.received_requests() != metric_src.handled_requests()
        {
            warn!("runtime exits immediately since the graceful exit feature has been disabled");
        }

        Ok(ret)
    }
}

#[cfg(unix)]
fn get_termination_signal() -> BoxFuture<'static, i32> {
    use signal::unix::signal;
    use signal::unix::SignalKind;

    let mut signals = vec![SignalKind::terminate()]
        .into_iter()
        .chain(if cfg!(feature = "termination-signal-ext") {
            vec![SignalKind::window_change()]
        } else {
            vec![]
        })
        .map(|it| (it.as_raw_value(), signal(it).unwrap()))
        .collect::<Vec<_>>();

    poll_fn(move |cx| {
        for (signum, signal) in &mut signals {
            let poll_result = signal.poll_recv(cx);

            if poll_result.is_ready() {
                return Poll::Ready::<std::os::raw::c_int>(*signum);
            }
        }

        Poll::Pending
    })
    .boxed()
}

#[cfg(not(unix))]
fn get_termination_signal() -> BoxFuture<'static, i32> {
    pending().boxed()
}

fn accept_stream<I>(
    io: I,
    req_tx: UnboundedSender<WorkerRequestMsg>,
    event_tx: Option<UnboundedSender<ServerEvent>>,
    metric_src: SharedMetricSource,
    graceful_exit_token: CancellationToken,
    maybe_req_read_timeout_dur: Option<Duration>,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    metric_src.incl_active_io();
    tokio::task::spawn({
        async move {
            let (service, cancel) = WorkerService::new(metric_src.clone(), req_tx);
            let (io, maybe_timeout_tx) = if let Some(timeout_dur) = maybe_req_read_timeout_dur {
                crate::timeout::Stream::with_timeout(io, timeout_dur)
            } else {
                crate::timeout::Stream::with_bypass(io)
            };

            let _guard = cancel.drop_guard();
            let _active_io_count_guard = scopeguard::guard(metric_src, |it| {
                it.decl_active_io();
            });

            let mut shutting_down = false;
            let conn_fut = Http::new()
                .serve_connection(io, crate::timeout::Service::new(service, maybe_timeout_tx))
                .with_upgrades();

            pin!(conn_fut);

            let conn_result = loop {
                tokio::select! {
                    res = conn_fut.as_mut() => break res,
                    _ = graceful_exit_token.cancelled(), if !shutting_down => {
                        shutting_down = true;
                        conn_fut.as_mut().graceful_shutdown();
                    }
                }
            };

            if let Err(e) = conn_result {
                // Most common cause for these errors are when the client closes
                // the connection before we could send a response
                if e.is_incomplete_message() {
                    debug!("connection reset ({:?})", e);
                } else {
                    error!("client connection error ({:?})", e);
                }

                if let Some(tx) = event_tx.as_ref() {
                    let _ = tx.send(ServerEvent::ConnectionError(e));
                }
            }
        }
    });
}
