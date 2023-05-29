use sb_worker_context::events::WorkerEvents;
use tokio::sync::mpsc;

pub mod units;

pub fn send_event_if_event_manager_available(
    maybe_event_manager: Option<mpsc::UnboundedSender<WorkerEvents>>,
    event: WorkerEvents,
) {
    if let Some(event_manager) = maybe_event_manager {
        let _ = event_manager.send(event);
    }
}
