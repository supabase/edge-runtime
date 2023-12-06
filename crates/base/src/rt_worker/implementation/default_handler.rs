use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::worker::{HandleCreationType, Worker, WorkerHandler};
use anyhow::Error;
use event_worker::events::{BootFailureEvent, PseudoEvent, UncaughtExceptionEvent, WorkerEvents};
use std::any::Any;
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot::Receiver;

impl WorkerHandler for Worker {
    fn handle_error(&self, error: Error) -> Result<WorkerEvents, Error> {
        log::error!("{}", error);
        Ok(WorkerEvents::BootFailure(BootFailureEvent {
            msg: error.to_string(),
        }))
    }

    fn handle_creation(
        &self,
        created_rt: DenoRuntime,
        unix_stream_rx: UnboundedReceiver<UnixStream>,
        termination_event_rx: Receiver<WorkerEvents>,
    ) -> HandleCreationType {
        let run_worker_rt = async {
            match created_rt.run(unix_stream_rx).await {
                // if the error is execution terminated, check termination event reason
                Err(err) => {
                    let err_string = err.to_string();
                    if err_string.ends_with("execution terminated")
                        || err_string.ends_with("wall clock duration reached")
                    {
                        Ok(termination_event_rx.await.unwrap())
                    } else {
                        Ok(WorkerEvents::UncaughtException(UncaughtExceptionEvent {
                            exception: err_string,
                            cpu_time_used: 0, // this will be set later
                        }))
                    }
                }
                Ok(()) => Ok(WorkerEvents::EventLoopCompleted(PseudoEvent {})),
            }
        };

        Box::pin(run_worker_rt)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
