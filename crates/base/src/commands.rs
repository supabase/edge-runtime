use crate::{
    inspector_server::Inspector,
    server::{Server, ServerFlags, ServerHealth, SignumOrExitCode, Tls, WorkerEntrypoints},
    worker::{pool::WorkerPoolPolicy, TerminationToken},
    InspectorOption,
};
use anyhow::Error;
use graph::DecoratorType;
use tokio::sync::mpsc::Sender;

#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    ip: &str,
    port: u16,
    tls: Option<Tls>,
    main_service_path: String,
    event_worker_path: Option<String>,
    decorator: Option<DecoratorType>,
    user_worker_policy: Option<WorkerPoolPolicy>,
    import_map_path: Option<String>,
    flags: ServerFlags,
    callback_tx: Option<Sender<ServerHealth>>,
    entrypoints: WorkerEntrypoints,
    termination_token: Option<TerminationToken>,
    static_patterns: Vec<String>,
    inspector_option: Option<InspectorOption>,
    jsx_specifier: Option<String>,
    jsx_module: Option<String>,
) -> Result<Option<SignumOrExitCode>, Error> {
    let mut server = Server::new(
        ip,
        port,
        tls,
        main_service_path,
        event_worker_path,
        decorator,
        user_worker_policy,
        import_map_path,
        flags,
        callback_tx,
        entrypoints,
        termination_token,
        static_patterns,
        inspector_option.map(Inspector::from_option),
        jsx_specifier,
        jsx_module,
    )
    .await?;

    server.listen().await
}
