use std::path::PathBuf;

use anyhow::Error;
use ort::{CUDAExecutionProvider, ExecutionProvider, GraphOptimizationLevel, Session};

pub(crate) fn create_session(model_file_path: PathBuf) -> Result<Session, Error> {
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

    let session = builder.commit_from_file(model_file_path)?;
    Ok(session)
}
