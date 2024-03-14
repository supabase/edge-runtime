use event_worker::events::{EventMetadata, WorkerEventWithMetadata};
use sb_workers::context::{UserWorkerMsgs, WorkerRuntimeOpts};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type WorkerCoreConfig = (
    Option<Uuid>,
    Option<UnboundedSender<UserWorkerMsgs>>,
    Option<UnboundedSender<WorkerEventWithMetadata>>,
    Option<CancellationToken>,
    String,
);

pub fn parse_worker_conf(conf: &WorkerRuntimeOpts) -> WorkerCoreConfig {
    let worker_core: WorkerCoreConfig = match conf {
        WorkerRuntimeOpts::UserWorker(worker_opts) => (
            worker_opts.key,
            worker_opts.pool_msg_tx.clone(),
            worker_opts.events_msg_tx.clone(),
            worker_opts.cancel.clone(),
            worker_opts
                .key
                .map(|k| format!("sb-iso-{:?}", k))
                .unwrap_or("isolate-worker-unknown".to_string()),
        ),
        WorkerRuntimeOpts::MainWorker(_) => (None, None, None, None, "main-worker".to_string()),
        WorkerRuntimeOpts::EventsWorker(_) => (None, None, None, None, "events-worker".to_string()),
    };

    worker_core
}

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
