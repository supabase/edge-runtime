use event_worker::events::{EventMetadata, WorkerEventWithMetadata, WorkerEvents};
use tokio::sync::mpsc;

pub mod units;

pub fn send_event_if_event_worker_available(
    maybe_event_worker: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    event: WorkerEvents,
    metadata: EventMetadata,
) {
    if let Some(event_worker) = maybe_event_worker {
        let _ = event_worker.send(WorkerEventWithMetadata { event, metadata });
    }
}
