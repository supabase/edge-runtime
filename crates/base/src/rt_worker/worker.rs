use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::utils::{get_event_metadata, parse_worker_conf};
use crate::rt_worker::worker_ctx::create_supervisor;
use crate::utils::send_event_if_event_worker_available;
use anyhow::{anyhow, Error};
use cpu_timer::get_thread_time;
use event_worker::events::{
    EventMetadata, LogEvent, LogLevel, WorkerEventWithMetadata, WorkerEvents,
};
use log::{debug, error};
use sb_worker_context::essentials::{UserWorkerMsgs, WorkerContextInitOpts};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::thread;
use tokio::net::UnixStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::sync::oneshot::{Receiver, Sender};
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
        termination_event_rx: Receiver<WorkerEvents>,
    ) -> HandleCreationType;
    fn as_any(&self) -> &dyn Any;
}

impl Worker {
    pub fn new(init_opts: &WorkerContextInitOpts) -> Result<Self, Error> {
        let (worker_key, pool_msg_tx, events_msg_tx, thread_name) =
            parse_worker_conf(&init_opts.conf);
        let event_metadata = get_event_metadata(&init_opts.conf);

        let worker_boot_start_time = Instant::now();

        Ok(Self {
            worker_boot_start_time,
            events_msg_tx,
            pool_msg_tx,
            event_metadata,
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
        let worker_key = self.worker_key;
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

                            // CPU TIMER
                            let (termination_event_tx, termination_event_rx) =
                                oneshot::channel::<WorkerEvents>();
                            let _cputimer;

                            // TODO: Allow customization of supervisor
                            if new_runtime.conf.is_user_worker() {
                                start_time = get_thread_time()?;

                                // cputimer is returned from supervisor and assigned here to keep it in scope.
                                _cputimer = create_supervisor(
                                    worker_key.unwrap_or(0),
                                    &mut new_runtime,
                                    termination_event_tx,
                                )?;
                            }

                            // TODO: Should we handle it gracefully?
                            start_time = get_thread_time()?;
                            let data = method_cloner.handle_creation(
                                new_runtime,
                                unix_channel_rx,
                                termination_event_rx,
                            );
                            data.await
                        }
                        Err(err) => {
                            let _ = booter_signal.send(Err(anyhow!("worker boot error")));
                            method_cloner.handle_error(err)
                        }
                    }
                });

                match result {
                    Ok(event) => {
                        send_event_if_event_worker_available(
                            events_msg_tx.clone(),
                            event,
                            event_metadata.clone(),
                        );
                    }
                    Err(err) => error!("unexpected worker error {}", err),
                };

                let end_time = get_thread_time()?;
                let cpu_time_msg =
                    format!("CPU time used: {:?}ms", (end_time - start_time) / 1_000_000);

                debug!("{}", cpu_time_msg);
                send_event_if_event_worker_available(
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
