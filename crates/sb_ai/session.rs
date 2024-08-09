use deno_core::error::AnyError;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::usize;
use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, Error};
use ort::{
    CPUExecutionProvider, ExecutionProviderDispatch, GraphOptimizationLevel, Session,
    SessionBuilder,
};

use crate::onnx::ensure_onnx_env_init;

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<Session>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(crate) fn get_session_builder() -> Result<SessionBuilder, AnyError> {
    let orm_threads = std::env::var("OMP_NUM_THREADS")
        .map_or(None, |val| val.parse::<usize>().ok())
        .unwrap_or(1);

    let builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        // NOTE(Nyannyacha): This is set to prevent memory leaks caused by different input
        // shapes.
        //
        // Backgrounds:
        // [1]: https://github.com/microsoft/onnxruntime/issues/11118
        // [2]: https://github.com/microsoft/onnxruntime/blob/main/onnxruntime/core/framework/session_options.h#L95-L110
        .with_memory_pattern(false)?
        .with_intra_threads(orm_threads)?;

    Ok(builder)
}

fn cpu_execution_provider() -> Box<dyn Iterator<Item = ExecutionProviderDispatch>> {
    Box::new(
        [
            // NOTE(Nyannacha): See the comment above. This makes `enable_cpu_mem_arena` set to
            // False.
            //
            // Backgrounds:
            // [1]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#9-18
            // [2]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#46-50
            CPUExecutionProvider::default().build(),
        ]
        .into_iter(),
    )
}

fn create_session(model_bytes: &[u8]) -> Result<Arc<Session>, Error> {
    let session = {
        if let Some(err) = ensure_onnx_env_init() {
            return Err(anyhow!("failed to create onnx environment: {err}"));
        }

        get_session_builder()?
            .with_execution_providers(cpu_execution_provider())?
            .commit_from_memory(model_bytes)?
    };

    let session = Arc::new(session);

    Ok(session)
}

pub(crate) fn load_session_from_file(
    model_file_path: PathBuf,
) -> Result<(String, Arc<Session>), Error> {
    let session_id = fxhash::hash(&model_file_path.to_string_lossy()).to_string();

    let mut sessions = SESSIONS.lock().unwrap();

    if let Some(session) = sessions.get(&session_id) {
        return Ok((session_id, session.clone()));
    }
    let model_bytes = std::fs::read(model_file_path)?;

    let session = create_session(model_bytes.as_slice())?;

    sessions.insert(session_id.to_owned(), session.clone());

    Ok((session_id, session))
}

pub fn cleanup() -> Result<usize, AnyError> {
    let mut remove_counter = 0;
    {
        let mut guard = SESSIONS.lock().unwrap();
        let mut to_be_removed = vec![];

        for (key, session) in &mut *guard {
            if Arc::strong_count(session) > 1 {
                continue;
            }

            to_be_removed.push(key.clone());
        }

        for key in to_be_removed {
            let old_store = guard.remove(&key);
            debug_assert!(old_store.is_some());

            remove_counter += 1;
        }
    }

    Ok(remove_counter)
}
