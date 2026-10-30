use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
