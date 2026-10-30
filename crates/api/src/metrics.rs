//! `/metrics`: Prometheus のテキスト形式。

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use time::OffsetDateTime;
use watchtower_metrics::Gauges;
use watchtower_store::OutboxStatus;

use crate::auth::check_key;
use crate::error::ApiResult;
use crate::ApiState;

pub async fn scrape(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult<Response> {
    check_key(&state, &headers, true).await?;
    let now = OffsetDateTime::now_utc();
    let watches = state.0.store.list_watches().await?;
    let notifications = state.0.store.list_notifications().await?;
    let count_status =
        |s: OutboxStatus| notifications.iter().filter(|n| n.status == s).count() as u64;
    let gauges = Gauges {
        watches: watches.len() as u64,
        paused_watches: watches.iter().filter(|w| w.paused).count() as u64,
        queue_length: watches
            .iter()
            .filter(|w| !w.paused && w.state.next_check_at.is_none_or(|t| t <= now))
            .count() as u64,
        outbox_pending: count_status(OutboxStatus::Pending),
        outbox_failed: count_status(OutboxStatus::Failed),
        uptime_seconds: state.0.started.elapsed().as_secs(),
    };
    let mut res = state.0.metrics.render(&gauges).into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    Ok(res)
}
