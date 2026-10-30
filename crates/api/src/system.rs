//! `/api/v1/systeminfo`。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use time::OffsetDateTime;
use watchtower_store::OutboxStatus;

use crate::error::ApiResult;
use crate::ApiState;

pub async fn info(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
    let now = OffsetDateTime::now_utc();
    let watches = state.0.store.list_watches().await?;
    // 時刻が来ている (または未実施の) 一時停止していないウォッチの数
    let queue_length = watches
        .iter()
        .filter(|w| !w.paused && w.state.next_check_at.is_none_or(|t| t <= now))
        .count();
    let pending = state
        .0
        .store
        .list_notifications()
        .await?
        .iter()
        .filter(|n| n.status == OutboxStatus::Pending)
        .count();
    Ok(Json(json!({
        "watch_count": watches.len(),
        "queue_length": queue_length,
        "pending_notifications": pending,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.0.started.elapsed().as_secs(),
    })))
}
