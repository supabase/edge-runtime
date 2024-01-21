#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use std::collections::HashMap;

use sb_workers::context::{WorkerContextInitOpts, WorkerRuntimeOpts};

use crate::integration_test_helper::{create_test_user_worker, test_user_runtime_opts};

#[tokio::test]
async fn test_worker_boot_invalid_imports() {
    let opts = WorkerContextInitOpts {
        service_path: "./test_cases/invalid_imports".into(),
        no_module_cache: false,
        import_map_path: None,
        env_vars: HashMap::new(),
        events_rx: None,
        timing: None,
        maybe_eszip: None,
        maybe_entrypoint: None,
        maybe_module_code: None,
        conf: WorkerRuntimeOpts::UserWorker(test_user_runtime_opts()),
    };

    let result = create_test_user_worker(opts).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "worker boot error");
}
