use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("store error: {0}")]
    Store(#[from] watchtower_store::StoreError),
    #[error("{0}")]
    Other(String),
}
