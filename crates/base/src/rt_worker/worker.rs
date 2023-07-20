use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::utils::{get_event_metadata, parse_worker_conf};
use crate::rt_worker::worker_ctx::create_supervisor;
use crate::utils::send_event_if_event_manager_available;
use anyhow::{anyhow, bail, Error};
use cpu_timer::get_thread_time;
use event_manager::events::{
    BootFailure, EventMetadata, LogEvent, LogLevel, PseudoEvent, UncaughtException,
    WorkerEventWithMetadata, WorkerEvents,
};
use log::{debug, error};
use sb_worker_context::essentials::{UserWorkerMsgs, WorkerContextInitOpts, WorkerRuntimeOpts};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::thread;
use tokio::net::UnixStream;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::{Receiver, Sender};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

#[derive(Clone)]
pub struct Worker {
    pub worker_boot_start_time: Instant,
    pub events_msg_tx: Option<UnboundedSender<WorkerEventWithMetadata>>,
    pub pool_msg_tx: Option<UnboundedSender<UserWorkerMsgs>>,
    pub event_metadata: EventMetadata,
    pub worker_key: Option<u64>,
    pub thread_name: String,
}

pub type HandleCreationType = Pin<Box<dyn Future<Output = Result<WorkerEvents, Error>>>>;

pub trait WorkerHandler: Send {
    fn handle_error(&self, error: Error) -> Result<WorkerEvents, Error>;
    fn handle_creation(
        &self,
        created_rt: DenoRuntime,
        unix_stream_rx: UnboundedReceiver<UnixStream>,
    ) -> HandleCreationType;
    fn as_any(&self) -> &dyn Any;
}

impl Worker {
    pub fn new(init_opts: &WorkerContextInitOpts) -> Result<Self, Error> {
        let service_path = init_opts.service_path.clone();

        let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
        let (worker_key, pool_msg_tx, events_msg_tx, thread_name) =
            parse_worker_conf(&init_opts.conf);
        let event_metadata = get_event_metadata(&init_opts.conf);

        if !service_path.exists() {
            bail!("service does not exist {:?}", &service_path)
        }

        let worker_boot_start_time = Instant::now();

        Ok(Self {
            worker_boot_start_time,
            events_msg_tx: events_msg_tx.clone(),
            pool_msg_tx,
            event_metadata: event_metadata.clone(),
            worker_key,
            thread_name,
        })
    }

    pub fn start(
        &self,
        opts: WorkerContextInitOpts,
        unix_channel_rx: UnboundedReceiver<UnixStream>,
        booter_signal: Sender<Result<(), Error>>,
    ) {
        let thread_name = self.thread_name.clone();
        let events_msg_tx = self.events_msg_tx.clone();
        let event_metadata = self.event_metadata.clone();
        let worker_key = self.worker_key.clone();
        let pool_msg_tx = self.pool_msg_tx.clone();
        let method_cloner = self.clone();

        let _handle: thread::JoinHandle<Result<(), Error>> = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local = tokio::task::LocalSet::new();

                let mut start_time = 0;

                let result: Result<WorkerEvents, Error> = local.block_on(&runtime, async {
                    match DenoRuntime::new(opts).await {
                        Ok(mut new_runtime) => {
                            let _ = booter_signal.send(Ok(()));
                            // TODO: Should we handle it gracefully?
                            let data = method_cloner.handle_creation(new_runtime, unix_channel_rx);
                            let resolve = data.await;
                            resolve
                        }
                        Err(err) => {
                            let _ = booter_signal.send(Err(anyhow!("worker boot error")));
                            method_cloner.handle_error(err)
                        }
                    }
                });

                match result {
                    Ok(event) => {
                        let lived_event = event.clone();
                        send_event_if_event_manager_available(
                            events_msg_tx.clone(),
                            lived_event,
                            event_metadata.clone(),
                        );
                    }
                    Err(err) => error!("unexpected worker error {}", err),
                };

                let end_time = get_thread_time()?;
                let cpu_time_msg =
                    format!("CPU time used: {:?}ms", (end_time - start_time) / 1_000_000);

                debug!("{}", cpu_time_msg);
                send_event_if_event_manager_available(
                    events_msg_tx,
                    WorkerEvents::Log(LogEvent {
                        msg: cpu_time_msg,
                        level: LogLevel::Info,
                    }),
                    event_metadata,
                );

                worker_key.and_then(|worker_key_unwrapped| {
                    pool_msg_tx.map(|tx| {
                        if let Err(err) = tx.send(UserWorkerMsgs::Shutdown(worker_key_unwrapped)) {
                            error!(
                                "failed to send the shutdown signal to user worker pool: {:?}",
                                err
                            );
                        }
                    })
                });

                Ok(())
            })
            .unwrap();
    }
}
