use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("request timed out")]
    Timeout,
    #[error("fetch cancelled")]
    Cancelled,
    #[error("unexpected status {0}")]
    Status(u16),
    #[error("response body exceeds {limit} bytes")]
    TooLarge { limit: usize },
    #[error("blocked by SSRF policy: {0}")]
    Blocked(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("http error: {0}")]
    Http(String),
}
