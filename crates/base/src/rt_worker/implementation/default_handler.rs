use crate::deno_runtime::DenoRuntime;
use crate::rt_worker::worker::{HandleCreationType, Worker, WorkerHandler};
use crate::rt_worker::worker_ctx::create_supervisor;
use anyhow::Error;
use event_worker::events::{BootFailure, PseudoEvent, UncaughtException, WorkerEvents};
use std::any::Any;
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;

impl WorkerHandler for Worker {
    fn handle_error(&self, error: Error) -> Result<WorkerEvents, Error> {
        println!("{}", error);
        Ok(WorkerEvents::BootFailure(BootFailure {
            msg: error.to_string(),
        }))
    }

    fn handle_creation(
        &self,
        mut created_rt: DenoRuntime,
        unix_stream_rx: UnboundedReceiver<UnixStream>,
    ) -> HandleCreationType {
        let (termination_event_tx, termination_event_rx) = oneshot::channel::<WorkerEvents>();

        let _cputimer;

        let user_supervisor_factory = if created_rt.conf.is_user_worker() {
            // cputimer is returned from supervisor and assigned here to keep it in scope.
            _cputimer = create_supervisor(
                self.worker_key.unwrap_or(0),
                &mut created_rt,
                termination_event_tx,
            );
            if let Err(e) = _cputimer {
                Err(e)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };

        let run_worker_rt = async {
            user_supervisor_factory?;
            match created_rt.run(unix_stream_rx).await {
                // if the error is execution terminated, check termination event reason
                Err(err) => {
                    let err_string = err.to_string();
                    if err_string.ends_with("execution terminated")
                        || err_string.ends_with("wall clock duration reached")
                    {
                        Ok(termination_event_rx.await.unwrap())
                    } else {
                        Ok(WorkerEvents::UncaughtException(UncaughtException {
                            exception: err_string,
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
