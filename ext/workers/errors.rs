use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("request has been cancelled by supervisor")]
    RequestCancelledBySupervisor,
}
