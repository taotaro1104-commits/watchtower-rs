//! 履歴 (チェック・通知のイベント) の表示。

use std::collections::HashMap;

use askama::Template;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::handlers::{internal, render, require};
use crate::AppState;

const PAGE_LIMIT: u32 = 200;
const WATCH_PAGE_LIMIT: u32 = 20;

pub(crate) struct EventRow {
    pub at: String,
    pub kind: String,
    pub detail: String,
    pub watch_id: Option<String>,
    pub watch_label: String,
}

#[derive(Template)]
#[template(path = "events.html")]
struct EventsPage {
    rows: Vec<EventRow>,
}

/// 新しい順のイベント行。`watch_id` を渡すとそのウォッチだけ。
pub(crate) async fn rows(
    state: &AppState,
    watch_id: Option<Uuid>,
    limit: u32,
) -> Result<Vec<EventRow>, watchtower_store::StoreError> {
    let events = state.0.store.list_events(watch_id, limit).await?;
    let labels: HashMap<Uuid, String> = state
        .0
        .store
        .list_watches()
        .await?
        .into_iter()
        .map(|w| (w.id, w.title.unwrap_or_else(|| w.url.to_string())))
        .collect();
    Ok(events
        .into_iter()
        .map(|e| EventRow {
            at: e.at.format(&Rfc3339).unwrap_or_default(),
            kind: e.kind,
            detail: e.detail.unwrap_or_default(),
            watch_label: e
                .watch_id
                .and_then(|id| labels.get(&id).cloned())
                .unwrap_or_default(),
            watch_id: e.watch_id.map(|id| id.to_string()),
        })
        .collect())
}

pub(crate) async fn watch_rows(
    state: &AppState,
    watch_id: Uuid,
) -> Result<Vec<EventRow>, watchtower_store::StoreError> {
    rows(state, Some(watch_id), WATCH_PAGE_LIMIT).await
}

pub async fn events_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require(&state, &headers).await {
        return r;
    }
    match rows(&state, None, PAGE_LIMIT).await {
        Ok(rows) => render(StatusCode::OK, &EventsPage { rows }),
        Err(e) => internal(e),
    }
}
