//! 取得層。`Fetcher` trait と HTTP 直取得の実装。

mod charset;
mod error;
mod http_fetcher;
mod ssrf;

pub use error::FetchError;
pub use http_fetcher::HttpFetcher;
pub use ssrf::SsrfPolicy;

use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, Method};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: Url,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// `http://host:port` 形式。未指定なら直接接続。
    pub proxy: Option<String>,
    pub timeout: Duration,
    pub max_body_bytes: usize,
    /// 空なら 2xx のみ成功扱い。
    pub accepted_statuses: Vec<u16>,
}

impl FetchRequest {
    pub fn get(url: Url) -> Self {
        Self {
            url,
            method: Method::GET,
            headers: Vec::new(),
            body: None,
            proxy: None,
            timeout: DEFAULT_TIMEOUT,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            accepted_statuses: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub content_type: Option<String>,
    /// charset 判定後に UTF-8 化した本文。
    pub body: String,
    /// ブラウザ取得のみ。
    pub screenshot: Option<Bytes>,
    pub final_url: Url,
}

#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(
        &self,
        req: &FetchRequest,
        cancel: CancellationToken,
    ) -> Result<FetchResponse, FetchError>;
}
