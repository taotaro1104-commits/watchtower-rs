//! API の入出力の形と、入力の検証。

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;
use uuid::Uuid;
use watchtower_core::effective::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};
use watchtower_core::Watch;
use watchtower_notify::NotifyTarget;

use crate::error::{ApiError, ApiResult};

pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_TAG_TITLE_CHARS: usize = 100;
const MAX_LINES: usize = 50;
const MAX_LINE_CHARS: usize = 500;
const MAX_TAGS_PER_WATCH: usize = 50;
/// 価格の閾値として受け付ける最大値。
const MAX_PRICE: f64 = 1e12;

/// 本家と同じ `{weeks, days, hours, minutes, seconds}` 形式の間隔。
#[derive(Debug, Deserialize)]
pub struct TimeBetween {
    weeks: Option<i64>,
    days: Option<i64>,
    hours: Option<i64>,
    minutes: Option<i64>,
    seconds: Option<i64>,
}

impl TimeBetween {
    fn to_secs(&self) -> ApiResult<u64> {
        let parts = [
            (self.weeks, 604_800),
            (self.days, 86_400),
            (self.hours, 3_600),
            (self.minutes, 60),
            (self.seconds, 1),
        ];
        let mut total: u64 = 0;
        for (value, unit) in parts {
            let v = value.unwrap_or(0);
            let v = u64::try_from(v)
                .map_err(|_| ApiError::bad_request("time_between_check must not be negative"))?;
            total = total.saturating_add(v.saturating_mul(unit));
        }
        if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&total) {
            return Err(ApiError::bad_request(
                "time_between_check is outside the allowed range",
            ));
        }
        Ok(total)
    }
}

/// 作成 (POST) と部分更新 (PUT) で共通の本文。PUT では指定したものだけを変える。
#[derive(Debug, Default, Deserialize)]
pub struct WatchBody {
    pub url: Option<String>,
    pub title: Option<String>,
    pub paused: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub time_between_check: Option<TimeBetween>,
    pub include_filters: Option<Vec<String>>,
    pub subtractive_selectors: Option<Vec<String>>,
    pub ignore_text: Option<Vec<String>>,
    pub trigger_text: Option<Vec<String>>,
    pub notification_urls: Option<Vec<String>>,
    pub notification_muted: Option<bool>,
    /// 指定すると設定、`null` で解除、省略すると変更しない。
    #[serde(default, deserialize_with = "present_or_null")]
    pub price_below: Option<Option<f64>>,
}

/// 「省略」と「null」を区別する (`Some(None)` が null)。
fn present_or_null<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

pub fn parse_watch_url(raw: &str) -> ApiResult<Url> {
    let raw = raw.trim();
    match Url::parse(raw) {
        Ok(u) if raw.chars().count() <= MAX_URL_CHARS && matches!(u.scheme(), "http" | "https") => {
            Ok(u)
        }
        _ => Err(ApiError::bad_request("A valid http(s) URL is required")),
    }
}

fn checked_lines(items: &[String], field: &str) -> ApiResult<Vec<String>> {
    let items: Vec<String> = items
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if items.len() > MAX_LINES {
        return Err(ApiError::bad_request(format!(
            "{field}: at most {MAX_LINES} entries"
        )));
    }
    if items.iter().any(|s| s.chars().count() > MAX_LINE_CHARS) {
        return Err(ApiError::bad_request(format!(
            "{field}: entries are limited to {MAX_LINE_CHARS} characters"
        )));
    }
    Ok(items)
}

fn checked_tags(raw: &[String], known: &HashSet<Uuid>) -> ApiResult<Vec<Uuid>> {
    let mut out: Vec<Uuid> = Vec::new();
    for r in raw {
        let id = Uuid::parse_str(r).ok().filter(|id| known.contains(id));
        let Some(id) = id else {
            return Err(ApiError::bad_request("Unknown tag"));
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_TAGS_PER_WATCH {
        return Err(ApiError::bad_request("Too many tags"));
    }
    Ok(out)
}

impl WatchBody {
    /// 検証してから `w` に反映する。1 つでも不正なら `w` は変更しない。
    pub fn apply(&self, w: &mut Watch, known_tags: &HashSet<Uuid>) -> ApiResult<()> {
        let mut next = w.clone();
        if let Some(u) = &self.url {
            next.url = parse_watch_url(u)?;
        }
        if let Some(t) = &self.title {
            let t = t.trim();
            if t.chars().count() > MAX_TITLE_CHARS {
                return Err(ApiError::bad_request("Title is too long"));
            }
            next.title = (!t.is_empty()).then(|| t.to_string());
        }
        if let Some(p) = self.paused {
            next.paused = p;
        }
        if let Some(tags) = &self.tags {
            next.tags = checked_tags(tags, known_tags)?;
        }
        if let Some(tb) = &self.time_between_check {
            next.config.check_interval_secs = Some(tb.to_secs()?);
        }
        if let Some(v) = &self.include_filters {
            next.config.include_filters = checked_lines(v, "include_filters")?;
        }
        if let Some(v) = &self.subtractive_selectors {
            next.config.exclude_filters = checked_lines(v, "subtractive_selectors")?;
        }
        if let Some(v) = &self.ignore_text {
            next.config.ignore_text = checked_lines(v, "ignore_text")?;
        }
        if let Some(v) = &self.trigger_text {
            next.config.trigger_text = checked_lines(v, "trigger_text")?;
        }
        if let Some(v) = &self.notification_urls {
            let urls = checked_lines(v, "notification_urls")?;
            for u in &urls {
                // 資格情報を含み得るので、URL そのものは応答に含めず理由だけ返す
                if let Err(e) = NotifyTarget::parse(u) {
                    return Err(ApiError::bad_request(format!(
                        "Invalid notification target: {e}"
                    )));
                }
            }
            next.config.notify_urls = urls;
        }
        if let Some(m) = self.notification_muted {
            next.config.notify_muted = m.then_some(true);
        }
        if let Some(p) = self.price_below {
            if p.is_some_and(|p| !p.is_finite() || !(0.0..=MAX_PRICE).contains(&p)) {
                return Err(ApiError::bad_request(
                    "price_below is outside the allowed range",
                ));
            }
            next.config.price_below = p;
        }
        *w = next;
        Ok(())
    }
}

fn time_between(secs: Option<u64>) -> Value {
    let Some(mut rest) = secs else {
        return Value::Null;
    };
    let mut out = serde_json::Map::new();
    for (name, unit) in [
        ("weeks", 604_800),
        ("days", 86_400),
        ("hours", 3_600),
        ("minutes", 60),
        ("seconds", 1),
    ] {
        out.insert(name.into(), json!(rest / unit));
        rest %= unit;
    }
    Value::Object(out)
}

/// 本家の形式に近い JSON (`last_checked` は UNIX 秒、無ければ 0。`last_error` は無ければ false)。
pub fn watch_view(w: &Watch) -> Value {
    let ts = |t: Option<time::OffsetDateTime>| t.map_or(0, |t| t.unix_timestamp());
    json!({
        "uuid": w.id,
        "url": w.url.as_str(),
        "title": w.title,
        "paused": w.paused,
        "tags": w.tags,
        "time_between_check": time_between(w.config.check_interval_secs),
        "include_filters": w.config.include_filters,
        "subtractive_selectors": w.config.exclude_filters,
        "ignore_text": w.config.ignore_text,
        "trigger_text": w.config.trigger_text,
        "notification_urls": w.config.notify_urls,
        "notification_muted": w.config.notify_muted == Some(true),
        "price_below": w.config.price_below,
        "last_checked": ts(w.state.last_checked_at),
        "last_changed": ts(w.state.last_changed_at),
        "last_error": w.state.last_error.as_ref().map_or(Value::Bool(false), |e| json!(e)),
    })
}
