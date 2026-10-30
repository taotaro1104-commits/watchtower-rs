//! SSE: 履歴が増えたことだけを知らせる。詳細は載せず、画面側が再取得する。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::unfold;
use tokio::sync::OwnedSemaphorePermit;

use crate::handlers::{error_page, internal, require};
use crate::AppState;

struct Tick {
    state: AppState,
    sid: String,
    last: i64,
    /// 接続が閉じたら解放される (同時接続数の上限)。
    _permit: OwnedSemaphorePermit,
}

pub async fn stream(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Ok(permit) = Arc::clone(&state.0.stream_slots).try_acquire_owned() else {
        return error_page(StatusCode::SERVICE_UNAVAILABLE, "Too many open streams.");
    };
    let last = match state.0.store.latest_event_id().await {
        Ok(id) => id,
        Err(e) => return internal(e),
    };
    let events = unfold(
        Tick {
            state,
            sid: auth.sid,
            last,
            _permit: permit,
        },
        next,
    );
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// 次の通知まで待つ。セッションが無効になったら `None` で終了する。
async fn next(mut t: Tick) -> Option<(Result<Event, Infallible>, Tick)> {
    loop {
        tokio::select! {
            () = t.state.shutdown_token().cancelled() => return None,
            () = tokio::time::sleep(t.state.0.cfg.sse_poll) => {}
        }
        // ログアウト・期限切れ・パスワード変更のあとは続けない
        let current = t.state.password_hash().await;
        let valid = current.is_some()
            && t.state
                .0
                .sessions
                .get(&t.sid)
                .is_some_and(|s| Some(&s.pw_fingerprint) == current.as_ref());
        if !valid {
            return None;
        }
        let latest = match t.state.0.store.latest_event_id().await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("SSE poll failed: {e}");
                t.last
            }
        };
        if latest > t.last {
            t.last = latest;
            return Some((
                Ok(Event::default().event("change").data(latest.to_string())),
                t,
            ));
        }
    }
}
