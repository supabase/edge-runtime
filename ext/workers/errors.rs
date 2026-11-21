use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkerError {
  /// The supervisor cancelled the request and did not record why.
  #[error("request has been cancelled by supervisor")]
  RequestCancelledBySupervisor,
  /// The worker used up its own CPU, memory or wall-clock budget.
  #[error("request cancelled: worker exceeded its resource limits")]
  WorkerResourceExhausted,
  /// The host stopped the worker while it was still serving the request.
  #[error("request cancelled: worker was reclaimed")]
  WorkerReclaimed,
  #[error("request cannot be handled because the worker has already retired")]
  WorkerAlreadyRetired,
  #[error("request timed out")]
  RequestIdleTimeout,
}
