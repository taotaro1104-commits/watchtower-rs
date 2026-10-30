//! `/api/v1/tags`。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use watchtower_core::Tag;

use crate::error::{ApiError, ApiResult};
use crate::json::ApiJson;
use crate::model::MAX_TAG_TITLE_CHARS;
use crate::ApiState;

const MAX_TAGS: usize = 200;

pub async fn list(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
    let out: Map<String, Value> = state
        .0
        .store
        .list_tags()
        .await?
        .into_iter()
        .map(|t| (t.id.to_string(), json!({ "uuid": t.id, "title": t.title })))
        .collect();
    Ok(Json(Value::Object(out)))
}

#[derive(Deserialize)]
pub struct NewTag {
    title: String,
}

pub(crate) fn valid_title(raw: &str) -> ApiResult<String> {
    let title = raw.trim();
    if title.is_empty() || title.chars().count() > MAX_TAG_TITLE_CHARS {
        return Err(ApiError::bad_request(format!(
            "title must be 1 to {MAX_TAG_TITLE_CHARS} characters"
        )));
    }
    Ok(title.to_string())
}

pub async fn create(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<NewTag>,
) -> ApiResult<Response> {
    let title = valid_title(&body.title)?;
    if state.0.store.list_tags().await?.len() >= MAX_TAGS {
        return Err(ApiError::bad_request("Too many tags"));
    }
    let tag = Tag::new(title);
    state.0.store.add_tag(&tag).await?;
    Ok((StatusCode::CREATED, Json(json!({ "uuid": tag.id }))).into_response())
}
