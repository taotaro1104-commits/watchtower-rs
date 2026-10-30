use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// 入力が上限を超えている (利用者に見せてよい文面)。
    #[error("{0}")]
    LimitExceeded(String),
    #[error("corrupt data: {0}")]
    Corrupt(String),
}
