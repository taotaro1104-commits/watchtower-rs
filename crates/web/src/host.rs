//! Host ヘッダの許可リスト。DNS rebinding (攻撃者のドメインが一時的に自分の IP を指し、
//! ブラウザ経由で管理画面に届く) を、想定外の名前宛ての要求を断って防ぐ。
//! IP アドレスそのものの Host は許す (rebinding には名前が要る)。

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use url::Url;

/// 許可するホスト名 (小文字、ポート無し、末尾のドット無し)。
#[derive(Clone, Debug)]
pub struct HostPolicy(Arc<Vec<String>>);

#[derive(Debug, thiserror::Error)]
pub enum HostPolicyError {
    #[error("base_url is not a valid URL with a host")]
    BaseUrl,
    #[error("allowed_hosts entries must be plain host names (no scheme, port, path or wildcard)")]
    Entry,
}

fn normalize(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

impl HostPolicy {
    /// `localhost` / `127.0.0.1` / `[::1]`、`base_url` のホスト、`extra` を許す。
    pub fn new(base_url: &str, extra: &[impl AsRef<str>]) -> Result<Self, HostPolicyError> {
        let mut names = vec!["localhost".to_string()];
        let base = Url::parse(base_url).map_err(|_| HostPolicyError::BaseUrl)?;
        match base.host() {
            Some(url::Host::Domain(d)) => names.push(normalize(d)),
            Some(_) => {} // IP は常に許す
            None => return Err(HostPolicyError::BaseUrl),
        }
        for e in extra {
            let e = normalize(e.as_ref().trim());
            if !valid_name(&e) {
                return Err(HostPolicyError::Entry);
            }
            names.push(e);
        }
        names.sort();
        names.dedup();
        Ok(Self(Arc::new(names)))
    }

    /// `Host` ヘッダの値が許可されるか。ヘッダが無い要求 (HTTP/1.0) はブラウザからは来ないので通す。
    pub fn allows(&self, host: Option<&str>) -> bool {
        let Some(raw) = host else {
            return true;
        };
        // "host", "host:port", "[v6]", "[v6]:port" だけを受け付ける (ユーザー情報などは不可)
        let Ok(auth) = raw.parse::<axum::http::uri::Authority>() else {
            return false;
        };
        if raw.contains('@') {
            return false;
        }
        let name = auth.host();
        // ホストのあとに何かあるなら、数字のポートでなければならない
        let brackets = if raw.starts_with('[') { 2 } else { 0 };
        if raw.len() > name.len() + brackets && auth.port_u16().is_none() {
            return false;
        }
        if name.parse::<IpAddr>().is_ok() || name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
        {
            return true;
        }
        self.0.contains(&normalize(name))
    }
}

/// 許可されない Host の要求を 421 で断る。値は応答にもログにも載せない。
pub async fn guard(State(policy): State<HostPolicy>, req: Request, next: Next) -> Response {
    // HTTP/2 には Host ヘッダが無く、:authority が URI に入る。どちらもあるときは両方を確かめる
    let header_host = req
        .headers()
        .get(header::HOST)
        .map(|v| v.to_str().unwrap_or("\u{0}"));
    let uri_host = req.uri().authority().map(|a| a.as_str());
    if policy.allows(header_host) && policy.allows(uri_host) {
        next.run(req).await
    } else {
        tracing::debug!("request refused: unexpected Host header");
        (StatusCode::MISDIRECTED_REQUEST, "Unexpected Host header.").into_response()
    }
}
