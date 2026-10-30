//! `/api/v1/import`: URL を 1 行ずつ書いたテキストの一括取り込み。

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use watchtower_core::{Tag, Watch};
use watchtower_scheduler::Command;

use crate::error::{ApiError, ApiResult};
use crate::model::parse_watch_url;
use crate::tags::valid_title;
use crate::ApiState;

const MAX_URLS: usize = 5000;

#[derive(Deserialize)]
pub struct ImportQuery {
    /// 付けるタグのタイトル (無ければ作る)。
    tag: Option<String>,
    paused: Option<String>,
}

pub async fn bulk(
    State(state): State<ApiState>,
    Query(q): Query<ImportQuery>,
    body: String,
) -> ApiResult<Json<Value>> {
    let paused = match q.paused.as_deref() {
        None | Some("false") | Some("0") => false,
        Some("true") | Some("1") => true,
        Some(_) => return Err(ApiError::bad_request("paused must be true or false")),
    };
    // 先にすべて検証する。1 行でも不正なら何も作らない (行番号だけ返し、内容は出さない)。
    let mut urls = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if urls.len() >= MAX_URLS {
            return Err(ApiError::bad_request(format!(
                "Too many URLs (at most {MAX_URLS})"
            )));
        }
        let url = parse_watch_url(line).map_err(|_| {
            ApiError::bad_request(format!("line {}: not a valid http(s) URL", i + 1))
        })?;
        urls.push(url);
    }
    let tag_id = match &q.tag {
        None => None,
        Some(t) => Some(tag_for(&state, &valid_title(t)?).await?),
    };

    let mut ids = Vec::with_capacity(urls.len());
    for url in urls {
        let mut w = Watch::new(url);
        w.paused = paused;
        w.tags = tag_id.into_iter().collect();
        state.0.store.add_watch(&w).await?;
        ids.push(w.id);
    }
    for id in &ids {
        state.notify(Command::Upsert(*id)).await;
    }
    Ok(Json(json!(ids)))
}

/// 同じタイトルのタグがあればそれを、無ければ作って返す。
async fn tag_for(state: &ApiState, title: &str) -> ApiResult<uuid::Uuid> {
    let store = &state.0.store;
    if let Some(t) = store
        .list_tags()
        .await?
        .into_iter()
        .find(|t| t.title == title)
    {
        return Ok(t.id);
    }
    let tag = Tag::new(title);
    store.add_tag(&tag).await?;
    Ok(tag.id)
}
