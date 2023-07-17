use event_manager::events::{EventMetadata, WorkerEventWithMetadata, WorkerEvents};
use tokio::sync::mpsc;

pub mod units;

pub fn send_event_if_event_manager_available(
    maybe_event_manager: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    event: WorkerEvents,
    metadata: EventMetadata,
) {
    if let Some(event_manager) = maybe_event_manager {
        let _ = event_manager.send(WorkerEventWithMetadata { event, metadata });
    }
}
