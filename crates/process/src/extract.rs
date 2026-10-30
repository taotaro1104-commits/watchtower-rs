use scraper::{ElementRef, Html, Selector};
use serde_json_path::JsonPath;

use crate::xpath::{self, Hit};
use crate::{guard, jq, product, ProcessError};
use watchtower_core::EffectiveConfig;

const JSON_PREFIX: &str = "json:";
const JQ_PREFIX: &str = "jq:";
const XPATH_PREFIX: &str = "xpath:";
/// 価格・在庫の検知 (JSON-LD / microdata / OpenGraph)。
const PRODUCT_FILTER: &str = "product:";
const TEXT_WIDTH: usize = 10_000;

fn selector(s: &str) -> Result<Selector, ProcessError> {
    Selector::parse(s).map_err(|e| ProcessError::InvalidSelector(format!("{s}: {e}")))
}

/// JSON 用の接頭辞を持つ絞り込みは HTML では使わない。
fn is_json_filter(f: &str) -> bool {
    f.starts_with(JSON_PREFIX) || f.starts_with(JQ_PREFIX)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 除外: CSS セレクタ、または `xpath:` に一致した要素を木から外す。
fn remove_matches(doc: &mut Html, filter: &str) -> Result<(), ProcessError> {
    if filter == PRODUCT_FILTER {
        return Ok(()); // 除外には意味がない
    }
    let ids: Vec<_> = match filter.strip_prefix(XPATH_PREFIX) {
        Some(expr) => xpath::select(doc, expr)?
            .into_iter()
            .filter_map(|h| match h {
                Hit::Element(id) => Some(id),
                Hit::Text(_) => None,
            })
            .collect(),
        None => {
            let sel = selector(filter)?;
            doc.select(&sel).map(|e| e.id()).collect()
        }
    };
    for id in ids {
        if let Some(mut node) = doc.tree.get_mut(id) {
            node.detach();
        }
    }
    Ok(())
}

/// 抽出: 一致した部分を HTML の断片として集める。
fn collect_matches(doc: &Html, filter: &str, parts: &mut Vec<String>) -> Result<(), ProcessError> {
    if filter == PRODUCT_FILTER {
        parts.extend(
            product::extract(doc)
                .lines()
                .into_iter()
                .map(|l| format!("<div>{}</div>", escape_html(&l))),
        );
        return Ok(());
    }
    let Some(expr) = filter.strip_prefix(XPATH_PREFIX) else {
        let sel = selector(filter)?;
        parts.extend(doc.select(&sel).map(|e| e.html()));
        return Ok(());
    };
    for hit in xpath::select(doc, expr)? {
        match hit {
            Hit::Element(id) => {
                if let Some(el) = doc.tree.get(id).and_then(ElementRef::wrap) {
                    parts.push(el.html());
                }
            }
            Hit::Text(t) => parts.push(format!("<div>{}</div>", escape_html(&t))),
        }
    }
    Ok(())
}

pub fn html_to_text(body: &str, cfg: &EffectiveConfig) -> Result<String, ProcessError> {
    if guard::exceeds_depth(body, guard::MAX_HTML_DEPTH) {
        return Err(ProcessError::TooDeep {
            limit: guard::MAX_HTML_DEPTH,
        });
    }
    let mut doc = Html::parse_document(body);

    for f in cfg.exclude_filters.iter().filter(|f| !is_json_filter(f)) {
        remove_matches(&mut doc, f)?;
    }

    let includes: Vec<&String> = cfg
        .include_filters
        .iter()
        .filter(|f| !is_json_filter(f))
        .collect();
    let html = if includes.is_empty() {
        doc.root_element().html()
    } else {
        let mut parts = Vec::new();
        for f in includes {
            collect_matches(&doc, f, &mut parts)?;
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
    let programs: Vec<&str> = cfg
        .include_filters
        .iter()
        .filter_map(|f| f.strip_prefix(JQ_PREFIX))
        .collect();
    if paths.is_empty() && programs.is_empty() {
        return serde_json::to_string_pretty(&value)
            .map_err(|e| ProcessError::InvalidJson(e.to_string()));
    }

    let mut lines = Vec::new();
    for p in paths {
        let path =
            JsonPath::parse(p).map_err(|e| ProcessError::InvalidJsonPath(format!("{p}: {e}")))?;
        lines.extend(path.query(&value).all().into_iter().map(render));
    }
    for program in programs {
        lines.extend(jq::run(program, body)?);
    }
    Ok(lines.join("\n"))
}
