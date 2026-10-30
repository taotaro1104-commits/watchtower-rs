use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("invalid selector: {0}")]
    InvalidSelector(String),
    #[error("invalid json: {0}")]
    InvalidJson(String),
    #[error("invalid json path: {0}")]
    InvalidJsonPath(String),
    #[error("html nesting deeper than {limit} levels")]
    TooDeep { limit: usize },
    #[error("html to text failed: {0}")]
    Text(String),
}
