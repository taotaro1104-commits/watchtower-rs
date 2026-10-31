use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use http::{HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::charset;
use crate::ssrf::{BlockedAddr, SafeResolver, SsrfPolicy};
use crate::{FetchError, FetchRequest, FetchResponse, Fetcher};

const MAX_REDIRECTS: usize = 10;

/// 静的ページ・JSON API・RSS 向けの HTTP 直取得。プロキシ単位で `Client` をプールする。
///
/// プロキシ経由では対象ホストの名前解決がプロキシ側で行われるため、DNS 後判定は効かない
/// (IP リテラルの判定のみ)。プロキシ自体を信頼できるものに限ること。
pub struct HttpFetcher {
    policy: Arc<SsrfPolicy>,
    clients: Mutex<HashMap<Option<String>, Client>>,
}

impl HttpFetcher {
    pub fn new(policy: SsrfPolicy) -> Self {
        Self {
            policy: Arc::new(policy),
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client_for(&self, proxy: &Option<String>) -> Result<Client, FetchError> {
        let mut map = self.clients.lock().expect("client pool poisoned");
        if let Some(c) = map.get(proxy) {
            return Ok(c.clone());
        }
        let policy = Arc::clone(&self.policy);
        // 環境変数 (HTTP_PROXY 等) のプロキシは使わない。プロキシ経由だと名前解決がそちら側になり、
        // SafeResolver を迂回されるため。プロキシはウォッチ単位の明示指定だけを許す。
        let mut builder = Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(SafeResolver(Arc::clone(&self.policy))))
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.error("too many redirects");
                }
                match policy.check_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(e) => attempt.error(BlockedAddr(e.to_string())),
                }
            }));
        if let Some(p) = proxy {
            let proxy =
                reqwest::Proxy::all(p).map_err(|e| FetchError::InvalidRequest(e.to_string()))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| FetchError::Http(e.to_string()))?;
        map.insert(proxy.clone(), client.clone());
        Ok(client)
    }

    /// プロキシ経由では対象ホストの名前解決がプロキシ側になるため、こちらでも解決して事前に判定する。
    /// 解決結果が 1 つでも許可外なら拒否する (プロキシ側の再解決との差は残るので、プロキシは信頼できるものに限る)。
    async fn check_resolved_locally(&self, url: &url::Url) -> Result<(), FetchError> {
        let Some(url::Host::Domain(host)) = url.host() else {
            return Ok(());
        };
        let port = url.port_or_known_default().unwrap_or(80);
        let addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| FetchError::Http(format!("dns lookup failed for {host}: {e}")))?;
        for a in addrs {
            if !self.policy.is_allowed(a.ip()) {
                return Err(FetchError::Blocked(format!("{host} -> {}", a.ip())));
            }
        }
        Ok(())
    }

    async fn run(&self, req: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.policy.check_url(&req.url)?;
        if req.proxy.is_some() {
            self.check_resolved_locally(&req.url).await?;
        }
        let client = self.client_for(&req.proxy)?;

        let mut builder = client
            .request(req.method.clone(), req.url.clone())
            .timeout(req.timeout);
        for (k, v) in &req.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| FetchError::InvalidRequest(format!("header {k}: {e}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| FetchError::InvalidRequest(format!("header {k}: {e}")))?;
            builder = builder.header(name, value);
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }

        let resp = builder.send().await.map_err(map_err)?;
        let status = resp.status().as_u16();
        let ok = if req.accepted_statuses.is_empty() {
            resp.status().is_success()
        } else {
            req.accepted_statuses.contains(&status)
        };
        if !ok {
            return Err(FetchError::Status(status));
        }
        if resp
            .content_length()
            .is_some_and(|n| n > req.max_body_bytes as u64)
        {
            return Err(FetchError::TooLarge {
                limit: req.max_body_bytes,
            });
        }

        let headers = resp.headers().clone();
        let final_url = resp.url().clone();
        let content_type = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_err)?;
            if buf.len() + chunk.len() > req.max_body_bytes {
                return Err(FetchError::TooLarge {
                    limit: req.max_body_bytes,
                });
            }
            buf.extend_from_slice(&chunk);
        }

        let (body, raw) = if is_pdf(content_type.as_deref(), &buf) {
            (String::new(), Some(Bytes::from(buf)))
        } else {
            (charset::decode(&buf, content_type.as_deref()), None)
        };
        Ok(FetchResponse {
            status,
            headers,
            body,
            raw,
            content_type,
            screenshot: None,
            final_url,
        })
    }
}

/// PDF か。Content-Type が PDF なら無条件に、そうでなければ先頭が `%PDF-` のときだけ
/// (HTML の本文中に `%PDF-` があっても誤検出しない)。
fn is_pdf(content_type: Option<&str>, bytes: &[u8]) -> bool {
    let mime = content_type
        .and_then(|c| c.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase());
    matches!(
        mime.as_deref(),
        Some("application/pdf" | "application/x-pdf")
    ) || bytes.trim_ascii_start().starts_with(b"%PDF-")
}

fn map_err(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        return FetchError::Timeout;
    }
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(err) = src {
        if let Some(b) = err.downcast_ref::<BlockedAddr>() {
            return FetchError::Blocked(b.0.clone());
        }
        src = err.source();
    }
    FetchError::Http(e.to_string())
}

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(
        &self,
        req: &FetchRequest,
        cancel: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(FetchError::Cancelled),
            res = self.run(req) => res,
        }
    }
}
