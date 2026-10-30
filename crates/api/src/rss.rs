//! `/rss?token=...`: 変更のあったウォッチを RSS 2.0 で配信する。
//!
//! RSS リーダーはヘッダを送れないので、API キーとは別の読み取り専用トークンを URL で受け取る。

use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use time::format_description::well_known::Rfc2822;
use watchtower_core::Watch;
use watchtower_process::diff;

use crate::auth::{constant_time_eq, hash_key, random_token};
use crate::error::{ApiError, ApiResult};
use crate::ApiState;

const TOKEN_SETTING: &str = "rss_token_hash";
const MAX_ITEMS: usize = 50;
/// 1 件の説明に載せる差分の最大文字数。
const MAX_DESCRIPTION_CHARS: usize = 2000;

impl ApiState {
    /// 新しいフィード用トークンを作って保存し、平文を返す (以前のものは無効になる)。
    pub async fn rotate_rss_token(&self) -> Result<String, watchtower_store::StoreError> {
        let token = random_token("rss_");
        self.0
            .store
            .set_setting(TOKEN_SETTING, &Value::String(hash_key(&token)))
            .await?;
        Ok(token)
    }

    async fn rss_token_hash(&self) -> Result<Option<String>, watchtower_store::StoreError> {
        Ok(match self.0.store.get_setting(TOKEN_SETTING).await? {
            Some(Value::String(h)) => Some(h),
            _ => None,
        })
    }
}

#[derive(Deserialize)]
pub struct FeedQuery {
    token: Option<String>,
}

/// XML に書けない文字を除き、特殊文字をエスケープする。
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t'
            | '\n'
            | '\r'
            | '\u{20}'..='\u{D7FF}'
            | '\u{E000}'..='\u{FFFD}'
            | '\u{10000}'..='\u{10FFFF}' => out.push(c),
            _ => {}
        }
    }
    out
}

/// 直近 2 つのスナップショットの差分 (`- 消えた行` / `+ 増えた行`)。読めなければ空。
async fn latest_diff(state: &ApiState, w: &Watch) -> String {
    let Ok(snaps) = state.0.store.list_snapshots(w.id).await else {
        return String::new();
    };
    let [.., prev, last] = snaps.as_slice() else {
        return String::new();
    };
    let (Ok(old), Ok(new)) = (
        state.0.store.read_snapshot(prev).await,
        state.0.store.read_snapshot(last).await,
    ) else {
        return String::new();
    };
    let d = diff(&old, &new);
    let text = d
        .removed
        .iter()
        .map(|l| format!("- {l}"))
        .chain(d.added.iter().map(|l| format!("+ {l}")))
        .collect::<Vec<_>>()
        .join("\n");
    match text.char_indices().nth(MAX_DESCRIPTION_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

pub async fn feed(
    State(state): State<ApiState>,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Response> {
    let Some(stored) = state.rss_token_hash().await? else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The RSS feed is not configured. Run `watchtower rss-token` first.",
        ));
    };
    let supplied = q.token.unwrap_or_default();
    if supplied.is_empty() || !constant_time_eq(&hash_key(&supplied), &stored) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing token",
        ));
    }

    let mut changed: Vec<Watch> = state
        .0
        .store
        .list_watches()
        .await?
        .into_iter()
        .filter(|w| w.state.last_changed_at.is_some())
        .collect();
    changed.sort_by_key(|w| std::cmp::Reverse(w.state.last_changed_at));
    changed.truncate(MAX_ITEMS);

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"><channel>\
         <title>watchtower changes</title><link>/</link>\
         <description>Pages that changed</description>\n",
    );
    for w in &changed {
        let at = w.state.last_changed_at.expect("filtered above");
        let title = w.title.clone().unwrap_or_else(|| w.url.to_string());
        let date = at.format(&Rfc2822).unwrap_or_default();
        xml.push_str(&format!(
            "<item><title>{}</title><link>{}</link><guid isPermaLink=\"false\">{}-{}</guid>\
             <pubDate>{}</pubDate><description>{}</description></item>\n",
            xml_escape(&title),
            xml_escape(w.url.as_str()),
            w.id,
            at.unix_timestamp(),
            xml_escape(&date),
            xml_escape(&latest_diff(&state, w).await),
        ));
    }
    xml.push_str("</channel></rss>\n");

    let mut res = xml.into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/rss+xml; charset=utf-8"),
    );
    Ok(res)
}
