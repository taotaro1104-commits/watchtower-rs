use thiserror::Error;

#[derive(Debug, Error)]
pub enum NotifyError {
    /// 宛先 URL の形式が不正 (資格情報を含み得るので原因の種別だけを持つ)。
    #[error("invalid notification URL: {0}")]
    InvalidUrl(&'static str),
    #[error("unsupported notification scheme: {0}")]
    UnsupportedScheme(String),
    #[error("invalid template: {0}")]
    Template(String),
    /// 停止要求で送信を中断した (失敗ではない)。
    #[error("cancelled")]
    Cancelled,
    #[error("storage error: {0}")]
    Store(String),
    /// 送信失敗。URL や資格情報を含めない。
    #[error("delivery failed: {0}")]
    Delivery(String),
}

impl From<watchtower_store::StoreError> for NotifyError {
    fn from(e: watchtower_store::StoreError) -> Self {
        Self::Store(e.to_string())
    }
}
