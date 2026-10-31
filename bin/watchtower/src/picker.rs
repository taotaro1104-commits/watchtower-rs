//! ビジュアルセレクタ用に、外部のブラウザでページを描画する。

use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::pick::Inspection;
use watchtower_core::BrowserStep;
use watchtower_fetch::{CdpFetcher, FetchRequest, SsrfPolicy};
use watchtower_web::PagePicker;

const RENDER_TIMEOUT: Duration = Duration::from_secs(45);

pub struct CdpPicker {
    fetcher: CdpFetcher,
}

impl CdpPicker {
    pub fn new(ws_url: String, policy: SsrfPolicy) -> Self {
        Self {
            fetcher: CdpFetcher::new(ws_url, policy),
        }
    }
}

#[async_trait]
impl PagePicker for CdpPicker {
    async fn inspect(&self, url: Url, steps: Vec<BrowserStep>) -> Result<Inspection, String> {
        let mut req = FetchRequest::get(url);
        req.timeout = RENDER_TIMEOUT;
        req.browser_steps = steps;
        // 取得器のエラーは接続先を含まないように作られている
        self.fetcher
            .inspect(&req, CancellationToken::new())
            .await
            .map_err(|e| e.to_string())
    }
}
