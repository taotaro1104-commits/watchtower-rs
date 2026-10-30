//! 外部の Apprise API サーバ (apprise-api コンテナ) への転送。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::Fetcher;

use crate::notifier::deliver;
use crate::request::post_json;
use crate::{Message, Notifier, NotifyError, NotifyTarget};

/// 未実装のサービスの宛先 URL と本文を、Apprise API の `POST /notify` に渡す。
/// 宛先 URL (トークンを含む) は設定したサーバに渡る。通信は Fetcher の SSRF ポリシーに従う。
pub struct AppriseApiNotifier {
    endpoint: Url,
    fetcher: Arc<dyn Fetcher>,
}

impl AppriseApiNotifier {
    pub fn new(base: Url, fetcher: Arc<dyn Fetcher>) -> Self {
        let mut endpoint = base;
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        // 設定の誤りで資格情報つきの URL がログに出ないよう、ユーザー情報は持たない
        let _ = endpoint.set_username("");
        let _ = endpoint.set_password(None);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        let endpoint = endpoint.join("notify").unwrap_or(endpoint);
        Self { endpoint, fetcher }
    }
}

#[async_trait]
impl Notifier for AppriseApiNotifier {
    async fn send(
        &self,
        target: &NotifyTarget,
        msg: &Message,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError> {
        let Some(url) = target.foreign_url() else {
            return Err(NotifyError::Delivery("not a forwarded target".into()));
        };
        let payload = json!({
            "urls": url,
            "title": msg.title,
            "body": msg.body,
            "type": "info",
            "format": "text",
        });
        let req = post_json(self.endpoint.clone(), Vec::new(), &payload);
        deliver(&self.fetcher, &req, cancel).await
    }
}
