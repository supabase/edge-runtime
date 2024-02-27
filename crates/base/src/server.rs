use crate::inspector_server::Inspector;
use crate::rt_worker::worker_ctx::{
    create_events_worker, create_main_worker, create_user_worker_pool, TerminationToken,
};
use crate::rt_worker::worker_pool::WorkerPoolPolicy;
use crate::InspectorOption;
use anyhow::{anyhow, bail, Context, Error};
use event_worker::events::WorkerEventWithMetadata;
use futures_util::{FutureExt, Stream};
use hyper::{server::conn::Http, service::Service, Body, Request, Response};
use log::{debug, error, info};
use sb_core::conn_sync::ConnSync;
use sb_core::SharedMetricSource;
use sb_workers::context::{MainWorkerRuntimeOpts, WorkerRequestMsg};
use std::future::{pending, Future};
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tls_listener::TlsListener;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{Sender, UnboundedSender};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

pub enum ServerEvent {
    ConnectionError(hyper::Error),
}

pub enum ServerHealth {
    Listening(mpsc::UnboundedReceiver<ServerEvent>),
    Failure,
}

struct NotifyOnEos<S> {
    inner: S,
    cancel: Option<CancellationToken>,
}

impl<S> Drop for NotifyOnEos<S> {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

impl<S: Stream + Unpin> Stream for NotifyOnEos<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.as_mut().inner).poll_next(cx)
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
            let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
            let (ob_conn_watch_tx, ob_conn_watch_rx) = watch::channel(ConnSync::Want);

            let req_uri = req.uri().clone();
            let msg = WorkerRequestMsg {
                req,
                res_tx,
                conn_watch: Some(ob_conn_watch_rx.clone()),
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
                            if let Err(ex) = ob_conn_watch_tx.send(ConnSync::Recv) {
                                error!("can't update connection watcher: {}", ex.to_string());
                            }
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
                Ok(res) => res,
                Err(e) => {
                    error!(
                        "request failed (uri: {:?} reason: {:?})",
                        req_uri.to_string(),
                        e
                    );

                    // FIXME: add an error body
                    return Ok(Response::builder()
                        .status(500)
                        .body(Body::wrap_stream(NotifyOnEos {
                            inner: Body::empty(),
                            cancel: Some(cancel.clone()),
                        }))
                        .unwrap());
                }
            };

            let (parts, body) = res.into_parts();
            let res = Response::from_parts(
                parts,
                Body::wrap_stream(NotifyOnEos {
                    inner: body,
                    cancel: Some(cancel),
                }),
            );

            Ok(res)
        };

        // Return the response as an immediate future
        Box::pin(fut)
    }
}

pub struct WorkerEntrypoints {
    pub main: Option<String>,
    pub events: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServerFlags {
    pub no_module_cache: bool,
    pub allow_main_inspector: bool,
    pub graceful_exit_deadline_sec: u64,
}

pub struct Tls {
    port: u16,
    key: PrivateKeyDer<'static>,
    cert: CertificateDer<'static>,
}

impl Tls {
    pub fn new(port: u16, key: &[u8], cert: &[u8]) -> anyhow::Result<Self> {
        use rustls_pemfile::read_one_from_slice;
        use rustls_pemfile::Item;

        let Some((key_item, _)) =
            read_one_from_slice(key).map_err(|err| anyhow!("can't resolve key: {:?}", err))?
        else {
            bail!("invalid key data")
        };

        let Some((Item::X509Certificate(cert), _)) =
            read_one_from_slice(cert).map_err(|err| anyhow!("can't resolve cert: {:?}", err))?
        else {
            bail!("invalid cert data")
        };

        let key = match key_item {
            Item::Pkcs1Key(key) => PrivateKeyDer::Pkcs1(key),
            Item::Pkcs8Key(key) => PrivateKeyDer::Pkcs8(key),
            Item::Sec1Key(key) => PrivateKeyDer::Sec1(key),
            _ => bail!("invalid key data"),
        };

        Ok(Self { port, key, cert })
    }

    fn into_acceptor(self) -> anyhow::Result<TlsAcceptor> {
        Ok(Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![self.cert], self.key)
                .with_context(|| "can't make TLS acceptor")?,
        )
        .into())
    }
}

pub struct Server {
    ip: Ipv4Addr,
    port: u16,
    tls: Option<Tls>,
    main_worker_req_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    callback_tx: Option<Sender<ServerHealth>>,
    termination_token: TerminationToken,
    flags: ServerFlags,
    metric_src: SharedMetricSource,
}

impl Server {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ip: &str,
        port: u16,
        tls: Option<Tls>,
        main_service_path: String,
        maybe_events_service_path: Option<String>,
        maybe_user_worker_policy: Option<WorkerPoolPolicy>,
        import_map_path: Option<String>,
        flags: ServerFlags,
        callback_tx: Option<Sender<ServerHealth>>,
        entrypoints: WorkerEntrypoints,
        termination_token: Option<TerminationToken>,
        inspector: Option<Inspector>,
    ) -> Result<Self, Error> {
        let mut worker_events_tx: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>> = None;
        let maybe_events_entrypoint = entrypoints.events;
        let maybe_main_entrypoint = entrypoints.main;
        let termination_token = termination_token.unwrap_or_default();

        // Create Event Worker
        let event_worker_metric_src = if let Some(events_service_path) = maybe_events_service_path {
            let events_path = Path::new(&events_service_path);
            let events_path_buf = events_path.to_path_buf();

            let (event_worker_metric, sender) = create_events_worker(
                events_path_buf,
                import_map_path.clone(),
                flags.no_module_cache,
                maybe_events_entrypoint,
                Some(termination_token.child_token()),
            )
            .await?;

            worker_events_tx = Some(sender);
            Some(event_worker_metric)
        } else {
            None
        };

        // Create a user worker pool
        let (shared_metric_src, worker_pool_tx) = create_user_worker_pool(
            maybe_user_worker_policy.unwrap_or_default(),
            worker_events_tx,
            None,
            inspector.clone(),
        )
        .await?;

        // create main worker
        let main_worker_path = Path::new(&main_service_path).to_path_buf();
        let main_worker_req_tx = create_main_worker(
            main_worker_path,
            import_map_path.clone(),
            flags.no_module_cache,
            MainWorkerRuntimeOpts {
                worker_pool_tx,
                shared_metric_src: Some(shared_metric_src.clone()),
                event_worker_metric_src,
            },
            maybe_main_entrypoint,
            Some(termination_token.child_token()),
            if flags.allow_main_inspector {
                inspector.map(|it| Inspector {
                    option: InspectorOption::Inspect(it.option.socket_addr()),
                    server: it.server,
                })
            } else {
                None
            },
        )
        .await?;

        let ip = Ipv4Addr::from_str(ip)?;

        Ok(Self {
            ip,
            port,
            tls,
            main_worker_req_tx,
            callback_tx,
            termination_token,
            flags,
            metric_src: shared_metric_src,
        })
    }

    pub async fn terminate(&self) {
        self.termination_token.cancel_and_wait().await;
    }

    pub async fn listen(&mut self) -> Result<(), Error> {
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

        let termination_token = self.termination_token.clone();
        let flags = self.flags;

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
            let _ = callback.send(ServerHealth::Listening(event_rx)).await;
        }

        let event_tx = can_receive_event.then_some(event_tx.clone());

        loop {
            let main_worker_req_tx = self.main_worker_req_tx.clone();
            let event_tx = event_tx.clone();
            let metric_src = self.metric_src.clone();

            tokio::select! {
                msg = non_secure_listener.accept() => {
                    match msg {
                        Ok((stream, _)) => {
                            accept_tcp_stream(stream, main_worker_req_tx, event_tx, metric_src)
                        }
                        Err(e) => error!("socket error: {}", e),
                    }
                }

                msg = async {
                    if let Some((listener, _addr)) = secure_listener.as_mut() {
                        listener.accept().boxed()
                    } else {
                        pending().boxed()
                    }.await
                } => {
                    match msg {
                        Ok((stream, _)) => {
                            accept_tcp_stream(stream, main_worker_req_tx, event_tx, metric_src);
                        }
                        Err(e) => error!("socket error: {}", e),
                    }
                }

                _ = termination_token.outbound.cancelled() => {
                    info!("termination token resolved");
                    break;
                }

                // wait for shutdown signal...
                _ = tokio::signal::ctrl_c() => {
                    info!("shutdown signal received");
                    break;
                }
            }
        }

        let graceful_exit_deadline = flags.graceful_exit_deadline_sec;

        if graceful_exit_deadline > 0 {
            let timeout_fut = timeout(
                Duration::from_secs(graceful_exit_deadline),
                termination_token.cancel_and_wait(),
            );

            tokio::select! {
                res = timeout_fut => {
                    if res.is_err() {
                        error!(
                            "did not able to terminate the workers within {} seconds",
                            graceful_exit_deadline,
                        );
                    }
                }

                _ = tokio::signal::ctrl_c() => {
                    error!("received interrupt signal while waiting workers");
                }
            }
        }

        Ok(())
    }
}

fn accept_tcp_stream<I>(
    io: I,
    req_tx: UnboundedSender<WorkerRequestMsg>,
    event_tx: Option<UnboundedSender<ServerEvent>>,
    metric_src: SharedMetricSource,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::task::spawn({
        async move {
            let (service, cancel) = WorkerService::new(metric_src, req_tx);
            let _guard = cancel.drop_guard();

            let conn_fut = Http::new().serve_connection(io, service).with_upgrades();

            if let Err(e) = conn_fut.await {
                // Most common cause for these errors are
                // when the client closes the connection
                // before we could send a response
                if e.is_incomplete_message() {
                    debug!("connection reset ({:?})", e);
                } else {
                    error!("client connection error ({:?})", e);
                }

                if let Some(tx) = event_tx {
                    let _ = tx.send(ServerEvent::ConnectionError(e));
                }
            }
        }
    });
}
