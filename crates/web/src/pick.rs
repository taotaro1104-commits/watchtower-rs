#![allow(clippy::result_large_err)] // 早期リターン用の Response をそのまま Err に載せる (短命な値)

//! ビジュアルセレクタ。ブラウザで描画したページのスクリーンショットを見せ、クリックした要素の
//! セレクタをウォッチの「含める / 除く」フィルタに加える。JavaScript は使わない
//! (スクリーンショットの上に、要素ごとのリンクを SVG で重ねる)。

use askama::Template;
use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::Form;
use base64::Engine;
use serde::Deserialize;
use url::Url;
use watchtower_core::pick::{Inspection, MAX_SELECTOR_CHARS};
use watchtower_core::{BrowserStep, ConfigLayer, EffectiveConfig};
use watchtower_scheduler::Command;

use crate::handlers::{bad_request, check_csrf, error_page, find_watch, internal, render, require};
use crate::AppState;

/// 1 つのフィルタ欄に入れられる件数の上限 (編集画面と同じ)。
const MAX_FILTERS: usize = 50;
/// 画面に埋め込むスクリーンショットの大きさの上限。
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// ブラウザでページを描画して、選べる要素の一覧を返すもの (実体は CDP の取得器)。
#[async_trait]
pub trait PagePicker: Send + Sync {
    /// `steps` を行ったあとの状態を検査する。エラーは画面に出してよい文面にする。
    async fn inspect(&self, url: Url, steps: Vec<BrowserStep>) -> Result<Inspection, String>;
}

struct BoxView {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    query: String,
    label: String,
}

#[derive(Template)]
#[template(path = "pick.html")]
struct PickPage {
    id: String,
    title: String,
    width: u32,
    height: u32,
    image: String,
    boxes: Vec<BoxView>,
}

#[derive(Template)]
#[template(path = "pick_choose.html")]
struct ChoosePage {
    csrf: String,
    id: String,
    selector: String,
}

fn query_escape(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// 画面に出す要素の枠。大きいものを先に (下に)、小さいものを後に (上に) 描くので、重なった所では小さいほうを選べる。
fn boxes(i: &Inspection) -> Vec<BoxView> {
    let mut v: Vec<_> = i.elements.iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(u64::from(e.width) * u64::from(e.height)));
    v.into_iter()
        .map(|e| BoxView {
            x: e.x,
            y: e.y,
            width: e.width,
            height: e.height,
            query: query_escape(&e.selector),
            label: if e.text.is_empty() {
                e.selector.clone()
            } else {
                format!("{} - {}", e.selector, e.text)
            },
        })
        .collect()
}

pub async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = require(&state, &headers).await {
        return r;
    }
    let w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Some(picker) = state.0.picker.clone() else {
        return error_page(
            StatusCode::NOT_FOUND,
            "The visual selector needs a browser. Set browser_ws_url to use it.",
        );
    };
    // 同時に 1 件だけ (ブラウザのタブと画面の組み立てにお金がかかる)
    let Ok(_slot) = state.0.pick_slot.try_acquire() else {
        return error_page(
            StatusCode::TOO_MANY_REQUESTS,
            "Another page is being rendered. Try again in a moment.",
        );
    };
    let tags = match state.0.store.tag_layers(&w.tags).await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    let steps = EffectiveConfig::resolve(&w.config, &tags, &ConfigLayer::default()).browser_steps;
    let inspection = match picker.inspect(w.url.clone(), steps).await {
        Ok(i) => i,
        Err(e) => {
            return error_page(
                StatusCode::BAD_GATEWAY,
                format!("Could not render the page: {e}"),
            )
        }
    };
    if inspection.screenshot.len() > MAX_IMAGE_BYTES {
        return error_page(
            StatusCode::BAD_GATEWAY,
            "The rendered page is too large to show.",
        );
    }
    render(
        StatusCode::OK,
        &PickPage {
            id: w.id.to_string(),
            title: w.title.clone().unwrap_or_else(|| w.url.to_string()),
            width: inspection.width,
            height: inspection.height,
            image: base64::engine::general_purpose::STANDARD.encode(&inspection.screenshot),
            boxes: boxes(&inspection),
        },
    )
}

#[derive(Deserialize)]
pub struct ChooseQuery {
    #[serde(default)]
    selector: String,
}

/// 受け取ったセレクタの形を確かめる (中身が正しい CSS かどうかは、取得時にわかる)。
fn valid_selector(raw: &str) -> Result<String, Response> {
    let s = raw.trim();
    if s.is_empty() || s.chars().count() > MAX_SELECTOR_CHARS || s.chars().any(char::is_control) {
        return Err(bad_request("The selector is empty, too long or malformed."));
    }
    Ok(s.to_string())
}

pub async fn choose_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ChooseQuery>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let selector = match valid_selector(&q.selector) {
        Ok(s) => s,
        Err(r) => return r,
    };
    render(
        StatusCode::OK,
        &ChoosePage {
            csrf: auth.csrf,
            id: w.id.to_string(),
            selector,
        },
    )
}

#[derive(Deserialize)]
pub struct ChooseForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    selector: String,
    #[serde(default)]
    mode: String,
}

pub async fn choose(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<ChooseForm>,
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
    let selector = match valid_selector(&form.selector) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let list = match form.mode.as_str() {
        "include" => &mut w.config.include_filters,
        "exclude" => &mut w.config.exclude_filters,
        _ => return bad_request("Choose whether to include or exclude the element."),
    };
    if !list.contains(&selector) {
        if list.len() >= MAX_FILTERS {
            return bad_request("Too many filters already.");
        }
        list.push(selector);
    }
    match state.0.store.update_watch(&w).await {
        Ok(true) => {}
        Ok(false) => return bad_request("Watch no longer exists."),
        Err(e) => return internal(e),
    }
    state.notify_scheduler(Command::Upsert(w.id)).await;
    Redirect::to(&format!("/watches/{}/edit", w.id)).into_response()
}
