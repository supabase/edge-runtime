use once_cell::sync::Lazy;
use std::{collections::HashMap, sync::Mutex};
use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, Error};
use ort::{CUDAExecutionProvider, ExecutionProvider, GraphOptimizationLevel, Session};

static SESSIONS: Lazy<Mutex<HashMap<String, Arc<Session>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(crate) fn create_session(model_file_path: PathBuf) -> Result<Arc<Session>, Error> {
    let Some(model_path) = model_file_path.to_str() else {
        return Err(anyhow!("failed to parse model filename as key"));
    };

    let mut sessions = SESSIONS.lock().unwrap();

    let Some(session) = sessions.get(model_path) else {
        println!("initializing a new session for {model_path}");
        let orm_threads = std::env::var("OMP_NUM_THREADS")
            .map_or(None, |val| val.parse::<usize>().ok())
            .unwrap_or(1);

        let builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(orm_threads)?;

        let cuda = CUDAExecutionProvider::default();
        if let Ok(is_cuda_available) = cuda.is_available() {
            println!("ORT: CUDA is_available: [{}]", is_cuda_available);

            if is_cuda_available {
                if let Err(error) = cuda.register(&builder) {
                    eprintln!("ORT: Failed to register CUDA:  - {:?}", error);
                }
            }
        }
        let session = builder.commit_from_file(&model_file_path).map(Arc::new)?;
        let _ = &sessions.insert(model_path.to_string(), session.to_owned());

        return Ok(session);
    };

    println!("loading session for {model_path}");
    Ok(session.to_owned())
}
