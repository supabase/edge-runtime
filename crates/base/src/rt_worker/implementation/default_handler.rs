use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::supervisor::CPUUsageMetrics;
use crate::rt_worker::worker::{DuplexStreamEntry, HandleCreationType, Worker, WorkerHandler};
use anyhow::Error;
use event_worker::events::{
    BootFailureEvent, EventLoopCompletedEvent, UncaughtExceptionEvent, WorkerEvents,
};
use log::error;
use std::any::Any;
use std::time::Duration;
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
        duplex_stream_rx: UnboundedReceiver<DuplexStreamEntry>,
        termination_event_rx: Receiver<WorkerEvents>,
        maybe_cpu_usage_metrics_tx: Option<UnboundedSender<CPUUsageMetrics>>,
        name: Option<String>,
    ) -> HandleCreationType<'r> {
        let run_worker_rt = async move {
            match created_rt
                .run(duplex_stream_rx, maybe_cpu_usage_metrics_tx, name)
                .await
            {
                // if the error is execution terminated, check termination event reason
                (Err(err), cpu_usage_ms) => {
                    let err_string = err.to_string();

                    if err_string.ends_with("execution terminated") {
                        Ok(termination_event_rx
                            .await
                            .unwrap()
                            .with_cpu_time_used(cpu_usage_ms as usize))
                    } else {
                        error!(
                            "runtime has escaped from the event loop unexpectedly: {}",
                            err_string.as_str()
                        );

                        Ok(WorkerEvents::UncaughtException(UncaughtExceptionEvent {
                            exception: err_string,
                            cpu_time_used: cpu_usage_ms as usize,
                        }))
                    }
                }

                (Ok(()), cpu_usage_ms) => {
                    // NOTE(Nyannyacha): If a supervisor unconditionally
                    // requests the isolate to terminate, it might not come to
                    // this branch, so it might be removed in the future.
                    if created_rt.is_termination_requested.is_raised() {
                        static EVENT_RECV_DEADLINE_DUR: Duration = Duration::from_secs(5);

                        if let Ok(Ok(ev)) =
                            tokio::time::timeout(EVENT_RECV_DEADLINE_DUR, termination_event_rx)
                                .await
                        {
                            return Ok(ev.with_cpu_time_used(cpu_usage_ms as usize));
                        }
                    }

                    Ok(WorkerEvents::EventLoopCompleted(EventLoopCompletedEvent {
                        cpu_time_used: cpu_usage_ms as usize,
                    }))
                }
            }
        };

        Box::pin(run_worker_rt)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
