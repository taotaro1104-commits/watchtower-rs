use scraper::{Html, Selector};
use serde_json_path::JsonPath;

use crate::{guard, ProcessError};
use watchtower_core::EffectiveConfig;

const JSON_PREFIX: &str = "json:";
const TEXT_WIDTH: usize = 10_000;

fn selector(s: &str) -> Result<Selector, ProcessError> {
    Selector::parse(s).map_err(|e| ProcessError::InvalidSelector(format!("{s}: {e}")))
}

pub fn html_to_text(body: &str, cfg: &EffectiveConfig) -> Result<String, ProcessError> {
    if guard::exceeds_depth(body, guard::MAX_HTML_DEPTH) {
        return Err(ProcessError::TooDeep {
            limit: guard::MAX_HTML_DEPTH,
        });
    }
    let mut doc = Html::parse_document(body);

    for f in &cfg.exclude_filters {
        let sel = selector(f)?;
        let ids: Vec<_> = doc.select(&sel).map(|e| e.id()).collect();
        for id in ids {
            if let Some(mut node) = doc.tree.get_mut(id) {
                node.detach();
            }
        }
    }

    let html = if cfg.include_filters.is_empty() {
        doc.root_element().html()
    } else {
        let mut parts = Vec::new();
        for f in &cfg.include_filters {
            let sel = selector(f)?;
            parts.extend(doc.select(&sel).map(|e| e.html()));
        }
        parts.join("\n")
    };

    html2text::from_read(html.as_bytes(), TEXT_WIDTH).map_err(|e| ProcessError::Text(e.to_string()))
}

fn render(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn json_to_text(body: &str, cfg: &EffectiveConfig) -> Result<String, ProcessError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ProcessError::InvalidJson(e.to_string()))?;

    let paths: Vec<&str> = cfg
        .include_filters
        .iter()
        .filter_map(|f| f.strip_prefix(JSON_PREFIX))
        .collect();
    if paths.is_empty() {
        return serde_json::to_string_pretty(&value)
            .map_err(|e| ProcessError::InvalidJson(e.to_string()));
    }

    let mut lines = Vec::new();
    for p in paths {
        let path =
            JsonPath::parse(p).map_err(|e| ProcessError::InvalidJsonPath(format!("{p}: {e}")))?;
        lines.extend(path.query(&value).all().into_iter().map(render));
    }
    Ok(lines.join("\n"))
}
