#![allow(clippy::result_large_err)] // 早期リターン用の Response をそのまま Err に載せる (短命な値)

//! ウォッチの編集画面。設定項目 (間隔・フィルタ・通知先) の入力処理はタグ画面と共有する。

use std::collections::HashSet;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::Form;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;
use watchtower_core::ConfigLayer;
use watchtower_notify::NotifyTarget;
use watchtower_scheduler::Command;

use crate::handlers::{
    bad_request, check_csrf, find_watch, internal, parse_interval, render, require,
    MAX_TITLE_CHARS, MAX_URL_CHARS,
};
use crate::AppState;

/// 1 項目あたりの行数・1 行あたりの文字数の上限。
const MAX_LINES: usize = 50;
const MAX_LINE_CHARS: usize = 500;
/// 1 つのウォッチに付けられるタグ数の上限。
const MAX_TAGS_PER_WATCH: usize = 50;

pub(crate) struct TagOption {
    pub id: String,
    pub title: String,
    pub checked: bool,
}

#[derive(Template)]
#[template(path = "edit.html")]
struct EditPage {
    csrf: String,
    id: String,
    url: String,
    title: String,
    paused: bool,
    tags: Vec<TagOption>,
    interval: String,
    notify_muted: bool,
    include_filters: String,
    exclude_filters: String,
    ignore_text: String,
    trigger_text: String,
    notify_urls: String,
}

/// 設定項目のテキストエリア表示 (ウォッチとタグで共通)。
pub(crate) struct ConfigText {
    pub interval: String,
    pub notify_muted: bool,
    pub include_filters: String,
    pub exclude_filters: String,
    pub ignore_text: String,
    pub trigger_text: String,
    pub notify_urls: String,
}

pub(crate) fn config_text(c: &ConfigLayer) -> ConfigText {
    ConfigText {
        interval: c
            .check_interval_secs
            .map(|n| n.to_string())
            .unwrap_or_default(),
        notify_muted: c.notify_muted == Some(true),
        include_filters: c.include_filters.join("\n"),
        exclude_filters: c.exclude_filters.join("\n"),
        ignore_text: c.ignore_text.join("\n"),
        trigger_text: c.trigger_text.join("\n"),
        notify_urls: c.notify_urls.join("\n"),
    }
}

pub async fn edit_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let all_tags = match state.0.store.list_tags().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    let tags = all_tags
        .into_iter()
        .map(|t| TagOption {
            checked: w.tags.contains(&t.id),
            id: t.id.to_string(),
            title: t.title,
        })
        .collect();
    let c = config_text(&w.config);
    render(
        StatusCode::OK,
        &EditPage {
            csrf: auth.csrf,
            id: w.id.to_string(),
            url: w.url.to_string(),
            title: w.title.clone().unwrap_or_default(),
            paused: w.paused,
            tags,
            interval: c.interval,
            notify_muted: c.notify_muted,
            include_filters: c.include_filters,
            exclude_filters: c.exclude_filters,
            ignore_text: c.ignore_text,
            trigger_text: c.trigger_text,
            notify_urls: c.notify_urls,
        },
    )
}

/// フォームから届く設定項目 (ウォッチとタグで共通)。
pub(crate) struct ConfigInput<'a> {
    pub interval: &'a str,
    pub notify_muted: bool,
    pub include_filters: &'a str,
    pub exclude_filters: &'a str,
    pub ignore_text: &'a str,
    pub trigger_text: &'a str,
    pub notify_urls: &'a str,
}

/// 1 行 1 項目のテキストエリアを一覧にする (空行は無視)。
fn lines(raw: &str, label: &str) -> Result<Vec<String>, Response> {
    let items: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if items.len() > MAX_LINES {
        return Err(bad_request(&format!(
            "{label}: at most {MAX_LINES} entries."
        )));
    }
    if items.iter().any(|l| l.chars().count() > MAX_LINE_CHARS) {
        return Err(bad_request(&format!(
            "{label}: entries are limited to {MAX_LINE_CHARS} characters."
        )));
    }
    Ok(items)
}

/// 入力を検証して `c` に反映する。どれか 1 つでも不正なら `c` は変更しない。
pub(crate) fn apply_config(c: &mut ConfigLayer, input: &ConfigInput) -> Result<(), Response> {
    let mut next = c.clone();
    next.check_interval_secs = parse_interval(input.interval)?;
    next.notify_muted = input.notify_muted.then_some(true);
    next.include_filters = lines(input.include_filters, "Include filters")?;
    next.exclude_filters = lines(input.exclude_filters, "Exclude filters")?;
    next.ignore_text = lines(input.ignore_text, "Ignore text")?;
    next.trigger_text = lines(input.trigger_text, "Trigger text")?;
    let urls = lines(input.notify_urls, "Notification URLs")?;
    for u in &urls {
        // 資格情報を含み得るので、URL そのものは応答に含めず理由だけ返す
        if let Err(e) = NotifyTarget::parse(u) {
            return Err(bad_request(&format!("Invalid notification target: {e}")));
        }
    }
    next.notify_urls = urls;
    *c = next;
    Ok(())
}

#[derive(Deserialize)]
pub struct EditForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    interval: String,
    paused: Option<String>,
    notify_muted: Option<String>,
    #[serde(default)]
    include_filters: String,
    #[serde(default)]
    exclude_filters: String,
    #[serde(default)]
    ignore_text: String,
    #[serde(default)]
    trigger_text: String,
    #[serde(default)]
    notify_urls: String,
    #[serde(default)]
    tags: Vec<String>,
}

pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<EditForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let mut w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let known: HashSet<Uuid> = match state.0.store.list_tags().await {
        Ok(t) => t.into_iter().map(|t| t.id).collect(),
        Err(e) => return internal(e),
    };
    if let Err(r) = apply(&mut w, &form, &known) {
        return r;
    }
    match state.0.store.update_watch(&w).await {
        Ok(true) => {}
        Ok(false) => return bad_request("Watch no longer exists."),
        Err(e) => return internal(e),
    }
    state.notify_scheduler(Command::Upsert(w.id)).await;
    Redirect::to(&format!("/watches/{}", w.id)).into_response()
}

fn parse_tags(raw: &[String], known: &HashSet<Uuid>) -> Result<Vec<Uuid>, Response> {
    let mut out: Vec<Uuid> = Vec::new();
    for r in raw {
        let id = Uuid::parse_str(r).ok().filter(|id| known.contains(id));
        let Some(id) = id else {
            return Err(bad_request("Unknown tag."));
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_TAGS_PER_WATCH {
        return Err(bad_request("Too many tags."));
    }
    Ok(out)
}

fn apply(
    w: &mut watchtower_core::Watch,
    form: &EditForm,
    known: &HashSet<Uuid>,
) -> Result<(), Response> {
    let raw = form.url.trim();
    let url = match Url::parse(raw) {
        Ok(u) if raw.chars().count() <= MAX_URL_CHARS && matches!(u.scheme(), "http" | "https") => {
            u
        }
        _ => return Err(bad_request("A valid http(s) URL is required.")),
    };
    let title = form.title.trim();
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(bad_request("Title is too long."));
    }
    let tags = parse_tags(&form.tags, known)?;
    let mut config = w.config.clone();
    apply_config(
        &mut config,
        &ConfigInput {
            interval: &form.interval,
            notify_muted: form.notify_muted.is_some(),
            include_filters: &form.include_filters,
            exclude_filters: &form.exclude_filters,
            ignore_text: &form.ignore_text,
            trigger_text: &form.trigger_text,
            notify_urls: &form.notify_urls,
        },
    )?;
    // すべて通ってから反映する
    w.url = url;
    w.title = (!title.is_empty()).then(|| title.to_string());
    w.paused = form.paused.is_some();
    w.tags = tags;
    w.config = config;
    Ok(())
}
