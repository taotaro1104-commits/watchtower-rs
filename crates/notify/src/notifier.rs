use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use watchtower_fetch::{FetchError, Fetcher};

use crate::{Message, NotifyError, NotifyTarget};

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn send(
        &self,
        target: &NotifyTarget,
        msg: &Message,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError>;
}

/// `Fetcher` 経由で HTTP 送信する。SSRF 対策は Fetcher 側のポリシーに従う。
pub struct HttpNotifier {
    fetcher: Arc<dyn Fetcher>,
}

impl HttpNotifier {
    pub fn new(fetcher: Arc<dyn Fetcher>) -> Self {
        Self { fetcher }
    }
}

#[async_trait]
impl Notifier for HttpNotifier {
    async fn send(
        &self,
        target: &NotifyTarget,
        msg: &Message,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError> {
        let req = target.build_request(msg)?;
        self.fetcher
            .fetch(&req, cancel)
            .await
            .map(drop)
            .map_err(|e| match e {
                FetchError::Cancelled => NotifyError::Cancelled,
                other => NotifyError::Delivery(describe(&other)),
            })
    }
}

/// Discord / Slack / Telegram は URL にトークンを含むため、下位のエラー文字列
/// (URL を含み得る) は使わず、種別だけを返す。
fn describe(e: &FetchError) -> String {
    match e {
        FetchError::Status(code) => format!("HTTP {code}"),
        FetchError::Timeout => "timed out".into(),
        FetchError::Cancelled => "cancelled".into(),
        FetchError::TooLarge { .. } => "response too large".into(),
        FetchError::Blocked(_) => "destination blocked by SSRF policy".into(),
        FetchError::InvalidRequest(_) | FetchError::Http(_) => "request failed".into(),
    }
}
