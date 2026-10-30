#![allow(clippy::result_large_err)] // 早期リターン用の Response をそのまま Err に載せる (短命な値)

//! タグの管理画面。タグは設定の中間層 (Watch > Tag > Global) を持つ。

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use uuid::Uuid;
use watchtower_core::Tag;

use crate::edit::{apply_config, config_text, ConfigInput, ConfigText};
use crate::handlers::{bad_request, check_csrf, error_page, internal, render, require};
use crate::AppState;

const MAX_TAG_TITLE_CHARS: usize = 100;
const MAX_TAGS: usize = 200;

struct TagRow {
    id: String,
    title: String,
    watches: usize,
}

#[derive(Template)]
#[template(path = "tags.html")]
struct TagsPage {
    csrf: String,
    rows: Vec<TagRow>,
}

#[derive(Template)]
#[template(path = "tag_edit.html")]
struct TagEditPage {
    csrf: String,
    id: String,
    title: String,
    interval: String,
    notify_muted: bool,
    price_below: String,
    include_filters: String,
    exclude_filters: String,
    ignore_text: String,
    trigger_text: String,
    notify_urls: String,
}

pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let (tags, watches) =
        match tokio::try_join!(state.0.store.list_tags(), state.0.store.list_watches()) {
            Ok(v) => v,
            Err(e) => return internal(e),
        };
    let mut counts: HashMap<Uuid, usize> = HashMap::new();
    for t in watches.iter().flat_map(|w| &w.tags) {
        *counts.entry(*t).or_default() += 1;
    }
    let rows = tags
        .into_iter()
        .map(|t| TagRow {
            watches: counts.get(&t.id).copied().unwrap_or(0),
            id: t.id.to_string(),
            title: t.title,
        })
        .collect();
    render(
        StatusCode::OK,
        &TagsPage {
            csrf: auth.csrf,
            rows,
        },
    )
}

fn valid_title(raw: &str) -> Result<String, Response> {
    let title = raw.trim();
    if title.is_empty() || title.chars().count() > MAX_TAG_TITLE_CHARS {
        return Err(bad_request(&format!(
            "A tag name of 1 to {MAX_TAG_TITLE_CHARS} characters is required."
        )));
    }
    Ok(title.to_string())
}

#[derive(Deserialize)]
pub struct CreateForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let title = match valid_title(&form.title) {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.0.store.list_tags().await {
        Ok(t) if t.len() >= MAX_TAGS => return bad_request("Too many tags."),
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    match state.0.store.add_tag(&Tag::new(title)).await {
        Ok(()) => Redirect::to("/tags").into_response(),
        Err(e) => internal(e),
    }
}

async fn find_tag(state: &AppState, id: &str) -> Result<Tag, Response> {
    let not_found = || error_page(StatusCode::NOT_FOUND, "Tag not found.");
    let id = Uuid::parse_str(id).map_err(|_| not_found())?;
    match state.0.store.get_tag(id).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(not_found()),
        Err(e) => Err(internal(e)),
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
    let tag = match find_tag(&state, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let ConfigText {
        interval,
        notify_muted,
        price_below,
        include_filters,
        exclude_filters,
        ignore_text,
        trigger_text,
        notify_urls,
    } = config_text(&tag.config);
    render(
        StatusCode::OK,
        &TagEditPage {
            csrf: auth.csrf,
            id: tag.id.to_string(),
            title: tag.title,
            interval,
            notify_muted,
            price_below,
            include_filters,
            exclude_filters,
            ignore_text,
            trigger_text,
            notify_urls,
        },
    )
}

#[derive(Deserialize)]
pub struct EditForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    interval: String,
    notify_muted: Option<String>,
    #[serde(default)]
    price_below: String,
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
    let mut tag = match find_tag(&state, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let title = match valid_title(&form.title) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Err(r) = apply_config(
        &mut tag.config,
        &ConfigInput {
            interval: &form.interval,
            notify_muted: form.notify_muted.is_some(),
            price_below: &form.price_below,
            include_filters: &form.include_filters,
            exclude_filters: &form.exclude_filters,
            ignore_text: &form.ignore_text,
            trigger_text: &form.trigger_text,
            notify_urls: &form.notify_urls,
        },
    ) {
        return r;
    }
    tag.title = title;
    match state.0.store.update_tag(&tag).await {
        Ok(true) => Redirect::to("/tags").into_response(),
        Ok(false) => bad_request("Tag no longer exists."),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct DeleteForm {
    #[serde(default)]
    csrf: String,
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let tag = match find_tag(&state, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match state.0.store.delete_tag(tag.id).await {
        Ok(_) => Redirect::to("/tags").into_response(),
        Err(e) => internal(e),
    }
}
