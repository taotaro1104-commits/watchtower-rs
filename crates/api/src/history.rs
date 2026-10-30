//! `/api/v1/watch/{uuid}/history` 系。スナップショットの時刻一覧と本文。

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{Map, Value};

use crate::error::{ApiError, ApiResult};
use crate::watches::find;
use crate::ApiState;

/// 時刻 (UNIX 秒) → その本文の取得先。ファイルのパスは出さない。
pub async fn list(State(state): State<ApiState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let w = find(&state, &id).await?;
    let snaps = state.0.store.list_snapshots(w.id).await?;
    let mut out = Map::new();
    for s in snaps {
        let ts = s.taken_at.unix_timestamp();
        // 同じ秒に複数あれば、あとのもの (新しい方) を指す
        out.insert(
            ts.to_string(),
            Value::String(format!("/api/v1/watch/{}/history/{ts}", w.id)),
        );
    }
    Ok(Json(Value::Object(out)))
}

pub async fn snapshot(
    State(state): State<ApiState>,
    Path((id, timestamp)): Path<(String, String)>,
) -> ApiResult<Response> {
    let w = find(&state, &id).await?;
    let missing = || ApiError::not_found("Snapshot");
    let text = if timestamp == "latest" {
        state
            .0
            .store
            .latest_snapshot_text(w.id)
            .await?
            .ok_or_else(missing)?
    } else {
        let ts: i64 = timestamp
            .parse()
            .map_err(|_| ApiError::bad_request("timestamp must be a UNIX time or 'latest'"))?;
        let rec = state
            .0
            .store
            .list_snapshots(w.id)
            .await?
            .into_iter()
            .rev()
            .find(|s| s.taken_at.unix_timestamp() == ts)
            .ok_or_else(missing)?;
        state.0.store.read_snapshot(&rec).await?
    };
    let mut res = text.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    // 取得した内容が (万一) HTML として解釈されても何も実行させない
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    Ok(res)
}
