//! ダッシュボード: 件数・失敗中のウォッチ・通知キュー・直近の変化をまとめて見せる。

use askama::Template;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use watchtower_store::OutboxStatus;

use crate::events::EventRow;
use crate::handlers::{internal, render, require};
use crate::AppState;

/// 直近の変化として見せる件数。
const RECENT_CHANGES: usize = 10;
/// 変化を探すために遡るイベントの件数。
const EVENT_SCAN: u32 = 200;
/// 失敗中として並べる上限。
const FAILING_LIMIT: usize = 20;

struct Failing {
    id: String,
    label: String,
    error: String,
    attempts: u32,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    total: usize,
    paused: usize,
    failing_count: usize,
    pending: usize,
    failed_notifications: usize,
    failing: Vec<Failing>,
    changes: Vec<EventRow>,
}

pub async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require(&state, &headers).await {
        return r;
    }
    let store = &state.0.store;
    let (watches, outbox, events) = match tokio::try_join!(
        store.list_watches(),
        store.list_notifications(),
        crate::events::rows(&state, None, EVENT_SCAN),
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let is_failing = |w: &watchtower_core::Watch| w.state.last_error.is_some();
    let failing_count = watches.iter().filter(|w| is_failing(w)).count();
    let failing = watches
        .iter()
        .filter(|w| is_failing(w))
        .take(FAILING_LIMIT)
        .map(|w| Failing {
            id: w.id.to_string(),
            label: w.title.clone().unwrap_or_else(|| w.url.to_string()),
            error: w.state.last_error.clone().unwrap_or_default(),
            attempts: w.state.consecutive_errors,
        })
        .collect();
    let page = DashboardPage {
        total: watches.len(),
        paused: watches.iter().filter(|w| w.paused).count(),
        failing_count,
        pending: outbox
            .iter()
            .filter(|n| n.status == OutboxStatus::Pending)
            .count(),
        failed_notifications: outbox
            .iter()
            .filter(|n| n.status == OutboxStatus::Failed)
            .count(),
        failing,
        changes: events
            .into_iter()
            .filter(|e| e.kind == "changed")
            .take(RECENT_CHANGES)
            .collect(),
    };
    render(StatusCode::OK, &page)
}
