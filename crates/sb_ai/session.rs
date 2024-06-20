use std::path::PathBuf;

use anyhow::Error;
use ort::{GraphOptimizationLevel, Session};

pub(crate) fn create_session(model_file_path: PathBuf) -> Result<Session, Error> {
    let orm_threads = std::env::var("OMP_NUM_THREADS")
        .map_or(None, |val| val.parse::<usize>().ok())
        .unwrap_or(1);

    let session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(orm_threads)?
        .commit_from_file(model_file_path)?;

    Ok(session)
}
