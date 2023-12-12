use std::collections::HashMap;

use base::rt_worker::worker_ctx::create_worker;
use sb_workers::context::{UserWorkerRuntimeOpts, WorkerContextInitOpts, WorkerRuntimeOpts};

#[tokio::test]
async fn test_worker_boot_invalid_imports() {
    let user_rt_opts = UserWorkerRuntimeOpts::default();
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/invalid_imports".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing_rx_pair: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(user_rt_opts),
    };
    let result = create_worker(opts).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "worker boot error");
}
