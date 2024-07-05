use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use ctor::ctor;
use log::error;
use scopeguard::guard;
use tracing::instrument;

static ONNX_INIT_ONNX_ENV_DONE: AtomicBool = AtomicBool::new(false);
static ONNX_INIT_RESULT: Mutex<Option<Arc<ort::Error>>> = Mutex::new(None);

#[ctor]
#[instrument]
fn init_onnx_env() {
    let mut guard1 = ONNX_INIT_RESULT.lock().unwrap();
    let mut _guard2 = guard(&ONNX_INIT_ONNX_ENV_DONE, |v| {
        let _ = v.swap(true, Ordering::SeqCst);
    });

    // Create the ONNX Runtime environment, for all sessions created in this process.
    // TODO: Add CUDA execution provider
    if let Err(err) = ort::init().with_name("SB_AI_ONNX").commit() {
        error!("sb_ai: failed to create environment: {}", err);
        let _ = guard1.insert(Arc::new(err));
    }
}

pub(crate) fn ensure_onnx_env_init() -> Option<Arc<ort::Error>> {
    assert!(ONNX_INIT_ONNX_ENV_DONE.load(Ordering::SeqCst));
    ONNX_INIT_RESULT.lock().unwrap().clone()
}
