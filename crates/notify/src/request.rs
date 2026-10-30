//! 宛先ごとの HTTP リクエスト生成 (純粋関数。送信は `Notifier` が行う)。

use std::time::Duration;

use http::Method;
use serde_json::{json, Value};
use url::Url;
use watchtower_fetch::FetchRequest;

use crate::target::{Kind, NotifyTarget};
use crate::{Message, NotifyError};

const SEND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DISCORD_LIMIT: usize = 2000;
const SLACK_LIMIT: usize = 4000;
const TELEGRAM_LIMIT: usize = 4096;
const NTFY_LIMIT: usize = 4096;

impl NotifyTarget {
    pub fn build_request(&self, msg: &Message) -> Result<FetchRequest, NotifyError> {
        let (url, headers, payload) = match &self.kind {
            Kind::Mail(_) => {
                return Err(NotifyError::InvalidUrl(
                    "email targets are not sent over HTTP",
                ));
            }
            Kind::Json { url } => (
                url.clone(),
                vec![],
                json!({
                    "version": "1.0",
                    "title": msg.title,
                    "message": msg.body,
                    "type": "info",
                }),
            ),
            Kind::Discord { id, token } => (
                fixed(&format!("https://discord.com/api/webhooks/{id}/{token}"))?,
                vec![],
                json!({
                    "content": truncate(&format!("**{}**\n{}", msg.title, msg.body), DISCORD_LIMIT),
                    // 差分に含まれる @everyone などでメンションが飛ばないようにする
                    "allowed_mentions": { "parse": [] },
                }),
            ),
            Kind::Slack { parts: [a, b, c] } => (
                fixed(&format!("https://hooks.slack.com/services/{a}/{b}/{c}"))?,
                vec![],
                json!({ "text": truncate(&slack_escape(&format!("*{}*\n{}", msg.title, msg.body)), SLACK_LIMIT) }),
            ),
            Kind::Telegram { bot_token, chat_id } => (
                fixed(&format!(
                    "https://api.telegram.org/bot{bot_token}/sendMessage"
                ))?,
                vec![],
                json!({
                    "chat_id": chat_id,
                    "text": truncate(&format!("{}\n{}", msg.title, msg.body), TELEGRAM_LIMIT),
                }),
            ),
            Kind::Ntfy { endpoint, topic } => (
                endpoint.clone(),
                vec![],
                json!({
                    "topic": topic,
                    "title": msg.title,
                    "message": truncate(&msg.body, NTFY_LIMIT),
                }),
            ),
            Kind::Gotify { endpoint, token } => (
                endpoint
                    .join("message")
                    .map_err(|_| NotifyError::InvalidUrl("malformed URL"))?,
                vec![("X-Gotify-Key".to_string(), token.clone())],
                json!({ "title": msg.title, "message": msg.body, "priority": 5 }),
            ),
        };
        Ok(post_json(url, headers, &payload))
    }
}

/// Slack は `<!channel>` や `<@U…>` を解釈するので、ページ由来の文字列は `& < >` を無害化する。
fn slack_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn fixed(url: &str) -> Result<Url, NotifyError> {
    Url::parse(url).map_err(|_| NotifyError::InvalidUrl("malformed URL"))
}

fn post_json(url: Url, mut headers: Vec<(String, String)>, payload: &Value) -> FetchRequest {
    headers.push(("Content-Type".into(), "application/json".into()));
    let mut req = FetchRequest::get(url);
    req.method = Method::POST;
    req.headers = headers;
    req.body = Some(payload.to_string().into_bytes());
    req.timeout = SEND_TIMEOUT;
    req.max_body_bytes = MAX_RESPONSE_BYTES;
    req
}

/// 文字数で切り詰め、切った場合は末尾を `…` にする。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}
