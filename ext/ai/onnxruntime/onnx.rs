use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use ctor::ctor;
use scopeguard::guard;
use tracing::{error, instrument};

static ONNX_INIT_ONNX_ENV_DONE: AtomicBool = AtomicBool::new(false);
static ONNX_INIT_RESULT: Mutex<Option<Arc<ort::Error>>> = Mutex::new(None);

#[ctor]
#[instrument]
fn init_onnx_env() {
    let mut guard1 = ONNX_INIT_RESULT.lock().unwrap();
    let mut _guard2 = guard(&ONNX_INIT_ONNX_ENV_DONE, |v| {
        let _ = v.swap(true, Ordering::SeqCst);
    });

    let _guard3 = guard(std::panic::take_hook(), |v| {
        std::panic::set_hook(v);
    });

    std::panic::set_hook(Box::new(|_| {
        // no op
    }));

    let result = std::panic::catch_unwind(|| {
        // Create the ONNX Runtime environment, for all sessions created in this process.
        // TODO: Add CUDA execution provider
        ort::init().with_name("SB_AI_ONNX").commit()
    });

    match result {
        Err(err) => {
            // the most common reason that reaches to this arm is a library loading failure.
            let _ = guard1.insert(Arc::new(ort::Error::CustomError(Box::from(match err
                .downcast_ref::<&'static str>()
            {
                Some(s) => s,
                None => match err.downcast_ref::<String>() {
                    Some(s) => &s[..],
                    None => "unknown error",
                },
            }))));
        }

        Ok(Err(err)) => {
            error!(reason = ?err, "failed to create environment");
            let _ = guard1.insert(Arc::new(err));
        }

        _ => {}
    }
}

pub(crate) fn ensure_onnx_env_init() -> Option<Arc<ort::Error>> {
    assert!(ONNX_INIT_ONNX_ENV_DONE.load(Ordering::SeqCst));
    ONNX_INIT_RESULT.lock().unwrap().clone()
}
