//! 本家 changedetection.io の `url-watches.json` をインポートする (読み込み専用の移行用)。

use serde_json::{Map, Value};
use url::Url;
use uuid::Uuid;
use watchtower_core::effective::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};
use watchtower_core::Watch;

use crate::{Store, StoreError};

#[derive(Debug, Default)]
pub struct ImportReport {
    pub imported: usize,
    /// 取り込んだウォッチの ID (スケジューラへ伝えるために使う)。
    pub ids: Vec<Uuid>,
    /// 取り込まなかったウォッチ (`id: 理由`)。
    pub skipped: Vec<String>,
    /// 取り込んだが、対応していないため落とした設定。
    pub warnings: Vec<String>,
}

/// 取り込める件数と、1 件あたりの大きさの上限 (Web のフォームと同じ値)。
const MAX_WATCHES: usize = 5000;
const MAX_URL_CHARS: usize = 2048;
const MAX_TITLE_CHARS: usize = 200;
const MAX_LINES: usize = 50;
const MAX_LINE_CHARS: usize = 500;

/// 本家にあって未対応の項目 (値があるときだけ警告する)。
const UNSUPPORTED: &[&str] = &["headers", "body", "proxy", "browser_steps", "extract_text"];

impl Store {
    pub async fn import_changedetection(&self, json: &str) -> Result<ImportReport, StoreError> {
        let root: Value = serde_json::from_str(json)?;
        let watching = root
            .get("watching")
            .and_then(Value::as_object)
            .ok_or_else(|| StoreError::Corrupt("no \"watching\" object".into()))?;
        if watching.len() > MAX_WATCHES {
            return Err(StoreError::LimitExceeded(format!(
                "too many watches in the file (at most {MAX_WATCHES})"
            )));
        }

        let mut report = ImportReport::default();
        let mut unsupported: Vec<&str> = Vec::new();
        let mut tags_seen = false;
        for (key, entry) in watching {
            let Some(obj) = entry.as_object() else {
                report.skipped.push(format!("{key}: not an object"));
                continue;
            };
            let watch = match to_watch(key, obj) {
                Ok(w) => w,
                Err(why) => {
                    report.skipped.push(format!("{key}: {why}"));
                    continue;
                }
            };
            if self.get_watch(watch.id).await?.is_some() {
                report.skipped.push(format!("{key}: already exists"));
                continue;
            }
            self.add_watch(&watch).await?;
            report.imported += 1;
            report.ids.push(watch.id);
            tags_seen |= obj
                .get("tags")
                .and_then(Value::as_array)
                .is_some_and(|t| !t.is_empty());
            for f in UNSUPPORTED {
                if obj.get(*f).is_some_and(is_meaningful) && !unsupported.contains(f) {
                    unsupported.push(f);
                }
            }
        }
        if tags_seen {
            report.warnings.push("tags were not imported".into());
        }
        for f in unsupported {
            report
                .warnings
                .push(format!("{f} is not supported and was dropped"));
        }
        Ok(report)
    }
}

fn is_meaningful(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

fn to_watch(key: &str, o: &Map<String, Value>) -> Result<Watch, String> {
    let raw = o.get("url").and_then(Value::as_str).ok_or("missing url")?;
    if raw.chars().count() > MAX_URL_CHARS {
        return Err("url is too long".into());
    }
    let url = Url::parse(raw.trim()).map_err(|_| "invalid url")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http/https URLs are supported".into());
    }
    let mut w = Watch::new(url);
    if let Ok(id) = Uuid::parse_str(key) {
        w.id = id;
    }
    let title = o
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty());
    if title.is_some_and(|t| t.chars().count() > MAX_TITLE_CHARS) {
        return Err("title is too long".into());
    }
    w.title = title.map(str::to_string);
    w.paused = o.get("paused").and_then(Value::as_bool).unwrap_or(false);
    w.config.check_interval_secs = interval(o.get("time_between_check"));
    w.config.include_filters = strings(o.get("include_filters"), "include_filters")?;
    w.config.exclude_filters = strings(o.get("subtractive_selectors"), "subtractive_selectors")?;
    w.config.ignore_text = strings(o.get("ignore_text"), "ignore_text")?;
    w.config.trigger_text = strings(o.get("trigger_text"), "trigger_text")?;
    w.config.notify_urls = strings(o.get("notification_urls"), "notification_urls")?;
    Ok(w)
}

fn strings(v: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    let items: Vec<String> = v
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if items.len() > MAX_LINES {
        return Err(format!("too many entries in {field}"));
    }
    if items.iter().any(|s| s.chars().count() > MAX_LINE_CHARS) {
        return Err(format!("an entry in {field} is too long"));
    }
    Ok(items)
}

/// `{weeks, days, hours, minutes, seconds}` を秒にする。すべて空なら既定値を使う (None)。
fn interval(v: Option<&Value>) -> Option<u64> {
    let o = v?.as_object()?;
    let part = |name: &str, unit: u64| {
        o.get(name)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_mul(unit)
    };
    let secs = part("weeks", 604_800)
        .saturating_add(part("days", 86_400))
        .saturating_add(part("hours", 3_600))
        .saturating_add(part("minutes", 60))
        .saturating_add(part("seconds", 1));
    (secs > 0).then(|| secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS))
}
