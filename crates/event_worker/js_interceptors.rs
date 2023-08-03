use crate::events::{EventMetadata, LogEvent, LogLevel, WorkerEvents, WorkerMemoryUsage};
use crate::WorkerEventWithMetadata;
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::OpState;
use log::error;
use tokio::sync::mpsc;

#[op]
pub fn op_user_worker_log(
    state: &mut OpState,
    msg: &str,
    is_err: bool,
    v8_heap_stats: Option<WorkerMemoryUsage>,
) -> Result<(), AnyError> {
    let maybe_tx = state.try_borrow::<mpsc::UnboundedSender<WorkerEventWithMetadata>>();
    let mut level = LogLevel::Info;
    if is_err {
        level = LogLevel::Error;
    }

    if let Some(tx) = maybe_tx {
        let event_metadata = state
            .try_borrow::<EventMetadata>()
            .unwrap_or(&EventMetadata::default())
            .clone();

        let metadata = EventMetadata {
            v8_heap_stats,
            ..event_metadata
        };

        tx.send(WorkerEventWithMetadata {
            event: WorkerEvents::Log(LogEvent {
                msg: msg.to_string(),
                level,
            }),
            metadata,
        })?;
    } else {
        error!("[{:?}] {}", level, msg);
    }

    Ok(())
}

deno_core::extension!(sb_events_js_interceptors, ops = [op_user_worker_log,]);
