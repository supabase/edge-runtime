use anyhow::{anyhow, bail};
use hyper_v014::{Body, Request, Response};
use sb_event_worker::events::{EventMetadata, WorkerEventWithMetadata, WorkerEvents};
use sb_workers::{
    context::{WorkerExit, WorkerRequestMsg, WorkerRuntimeOpts},
    errors::WorkerError,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub fn get_event_metadata(conf: &WorkerRuntimeOpts) -> EventMetadata {
    let mut event_metadata = EventMetadata {
        service_path: None,
        execution_id: None,
    };
    if conf.is_user_worker() {
        let conf = conf.as_user_worker().unwrap();
        event_metadata = EventMetadata {
            service_path: conf.service_path.clone(),
            execution_id: conf.key,
        };
    }

    event_metadata
}

pub async fn send_user_worker_request(
    worker_request_msg_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    req: Request<Body>,
    cancel: CancellationToken,
    exit: WorkerExit,
    conn_token: Option<CancellationToken>,
) -> Result<Response<Body>, anyhow::Error> {
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper_v014::Error>>();
    let msg = WorkerRequestMsg {
        req,
        res_tx,
        conn_token,
    };

    // send the message to worker
    worker_request_msg_tx.send(msg)?;

    // wait for the response back from the worker
    let res = tokio::select! {
        () = cancel.cancelled() => {
            bail!(exit
                .error()
                .await
                .unwrap_or(anyhow!(WorkerError::RequestCancelledBySupervisor)))
        }

        res = res_rx => res,
    }?;

    match res {
        Ok(v) => {
            // send the response back to the caller
            Ok(v)
        }

        Err(err) => {
            if let Some(actual_error) = exit.error().await {
                return Err(actual_error);
            }

            Err(err.into())
        }
    }
}

pub fn send_event_if_event_worker_available(
    maybe_event_worker: Option<&mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    event: WorkerEvents,
    metadata: EventMetadata,
) {
    if let Some(event_worker) = maybe_event_worker {
        let _ = event_worker.send(WorkerEventWithMetadata { event, metadata });
    }
}
