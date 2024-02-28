use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::supervisor::CPUUsageMetrics;
use crate::rt_worker::worker::{HandleCreationType, UnixStreamEntry, Worker, WorkerHandler};
use anyhow::Error;
use event_worker::events::{BootFailureEvent, PseudoEvent, UncaughtExceptionEvent, WorkerEvents};
use log::error;
use std::any::Any;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::Receiver;

impl WorkerHandler for Worker {
    fn handle_error(&self, error: Error) -> Result<WorkerEvents, Error> {
        log::error!("{}", error);
        Ok(WorkerEvents::BootFailure(BootFailureEvent {
            msg: error.to_string(),
        }))
    }

    fn handle_creation<'r>(
        &self,
        created_rt: &'r mut DenoRuntime,
        unix_stream_rx: UnboundedReceiver<UnixStreamEntry>,
        termination_event_rx: Receiver<WorkerEvents>,
        maybe_cpu_usage_metrics_tx: Option<UnboundedSender<CPUUsageMetrics>>,
        name: Option<String>,
    ) -> HandleCreationType<'r> {
        let run_worker_rt = async move {
            match created_rt
                .run(unix_stream_rx, maybe_cpu_usage_metrics_tx, name)
                .await
            {
                // if the error is execution terminated, check termination event reason
                (Err(err), cpu_usage_ns) => {
                    let err_string = err.to_string();

                    if err_string.ends_with("execution terminated") {
                        Ok(termination_event_rx.await.unwrap())
                    } else {
                        error!(
                            "runtime has escaped from the event loop unexpectedly: {}",
                            err_string.as_str()
                        );

                        Ok(WorkerEvents::UncaughtException(UncaughtExceptionEvent {
                            exception: err_string,
                            cpu_time_used: cpu_usage_ns as usize,
                        }))
                    }
                }
                (Ok(()), _) => Ok(WorkerEvents::EventLoopCompleted(PseudoEvent {})),
            }
        };

        Box::pin(run_worker_rt)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
