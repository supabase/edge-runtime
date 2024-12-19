use crate::events::{EventMetadata, LogEvent, LogLevel, WorkerEvents};
use crate::WorkerEventWithMetadata;
use deno_core::error::AnyError;
use deno_core::op2;
use deno_core::OpState;
use tokio::sync::mpsc;
use tracing::{event, trace};

#[op2(fast)]
fn op_user_worker_log(
    state: &mut OpState,
    #[string] msg: &str,
    is_err: bool,
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

        let metadata = WorkerEventWithMetadata {
            event: WorkerEvents::Log(LogEvent {
                msg: msg.to_string(),
                level,
            }),
            metadata: EventMetadata { ..event_metadata },
        };

        trace!(?metadata);
        tx.send(metadata)?;
    } else {
        match level {
            LogLevel::Debug => event!(tracing::Level::DEBUG, "{msg}"),
            LogLevel::Info => event!(tracing::Level::INFO, "{msg}"),
            LogLevel::Warning => event!(tracing::Level::WARN, "{msg}"),
            LogLevel::Error => event!(tracing::Level::ERROR, "{msg}"),
        }
    }

    Ok(())
}

deno_core::extension!(sb_events_js_interceptors, ops = [op_user_worker_log,],);
