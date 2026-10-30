//! `/api/v1/watch` 系のハンドラ。

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;
use watchtower_core::Watch;
use watchtower_scheduler::Command;

use crate::error::{ApiError, ApiResult};
use crate::json::ApiJson;
use crate::model::{parse_watch_url, watch_view, WatchBody};
use crate::ApiState;

pub(crate) async fn find(state: &ApiState, id: &str) -> ApiResult<Watch> {
    let not_found = || ApiError::not_found("Watch");
    let id = Uuid::parse_str(id).map_err(|_| not_found())?;
    state.0.store.get_watch(id).await?.ok_or_else(not_found)
}

async fn known_tags(state: &ApiState) -> ApiResult<HashSet<Uuid>> {
    Ok(state
        .0
        .store
        .list_tags()
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect())
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// タグのタイトルまたは UUID。
    tag: Option<String>,
}

pub async fn list(
    State(state): State<ApiState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let watches = state.0.store.list_watches().await?;
    let wanted: Option<HashSet<Uuid>> = match &q.tag {
        None => None,
        Some(t) => Some(
            state
                .0
                .store
                .list_tags()
                .await?
                .into_iter()
                .filter(|tag| tag.title == *t || tag.id.to_string() == *t)
                .map(|tag| tag.id)
                .collect(),
        ),
    };
    let out: Map<String, Value> = watches
        .iter()
        .filter(|w| {
            wanted
                .as_ref()
                .is_none_or(|ids| w.tags.iter().any(|t| ids.contains(t)))
        })
        .map(|w| (w.id.to_string(), watch_view(w)))
        .collect();
    Ok(Json(Value::Object(out)))
}

pub async fn create(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<WatchBody>,
) -> ApiResult<Response> {
    let raw = body
        .url
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("url is required"))?;
    let mut w = Watch::new(parse_watch_url(raw)?);
    body.apply(&mut w, &known_tags(&state).await?)?;
    state.0.store.add_watch(&w).await?;
    state.notify(Command::Upsert(w.id)).await;
    Ok((StatusCode::CREATED, Json(json!({ "uuid": w.id }))).into_response())
}

#[derive(Deserialize)]
pub struct OneQuery {
    recheck: Option<String>,
    paused: Option<String>,
}

pub async fn get_one(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(q): Query<OneQuery>,
) -> ApiResult<Json<Value>> {
    let mut w = find(&state, &id).await?;
    if q.recheck.is_none() && q.paused.is_none() {
        return Ok(Json(watch_view(&w)));
    }
    if let Some(p) = &q.paused {
        w.paused = match p.as_str() {
            "paused" => true,
            "unpaused" => false,
            _ => {
                return Err(ApiError::bad_request(
                    "paused must be 'paused' or 'unpaused'",
                ))
            }
        };
    }
    if let Some(r) = &q.recheck {
        if !matches!(r.as_str(), "1" | "true") {
            return Err(ApiError::bad_request("recheck must be 1 or true"));
        }
    }
    if q.paused.is_some() {
        state.0.store.update_watch(&w).await?;
        state.notify(Command::Upsert(w.id)).await;
    }
    if q.recheck.is_some() {
        state.notify(Command::CheckNow(w.id)).await;
    }
    Ok(Json(json!({ "message": "OK" })))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<WatchBody>,
) -> ApiResult<Json<Value>> {
    let mut w = find(&state, &id).await?;
    body.apply(&mut w, &known_tags(&state).await?)?;
    if !state.0.store.update_watch(&w).await? {
        return Err(ApiError::not_found("Watch"));
    }
    state.notify(Command::Upsert(w.id)).await;
    Ok(Json(json!({ "message": "OK" })))
}

pub async fn remove(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let w = find(&state, &id).await?;
    state.0.store.delete_watch(w.id).await?;
    state.notify(Command::Remove(w.id)).await;
    Ok(StatusCode::NO_CONTENT)
}
