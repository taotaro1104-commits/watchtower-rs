//! API キーの生成・保存・照合。保存するのはハッシュだけ。

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::error::ApiError;
use crate::ApiState;

const KEY_SETTING: &str = "api_key_hash";
const KEY_HEADER: &str = "x-api-key";

/// 推測できない 256 bit のトークン (`prefix` + 16 進)。
pub fn random_token(prefix: &str) -> String {
    let hex: String = rand::random::<[u8; 32]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{prefix}{hex}")
}

/// 推測できない 256 bit の API キー (`wt_` + 16 進)。
pub fn generate_key() -> String {
    random_token("wt_")
}

pub fn hash_key(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
}

/// 定数時間の比較 (ハッシュ同士なので長さは固定)。
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl ApiState {
    /// 新しいキーを作って保存し、平文を返す (表示するのはこの 1 回だけ)。
    pub async fn rotate_key(&self) -> Result<String, watchtower_store::StoreError> {
        let key = generate_key();
        self.0
            .store
            .set_setting(KEY_SETTING, &Value::String(hash_key(&key)))
            .await?;
        Ok(key)
    }

    async fn stored_hash(&self) -> Result<Option<String>, watchtower_store::StoreError> {
        Ok(match self.0.store.get_setting(KEY_SETTING).await? {
            Some(Value::String(h)) => Some(h),
            _ => None,
        })
    }
}

/// 提示されたキーを検証する。キー未設定の間は API 全体を閉じる。
/// `allow_bearer` のときは `Authorization: Bearer <key>` も受け付ける (Prometheus 用)。
pub(crate) async fn check_key(
    state: &ApiState,
    headers: &HeaderMap,
    allow_bearer: bool,
) -> Result<(), ApiError> {
    let Some(stored) = state.stored_hash().await? else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "No API key is configured. Run `watchtower api-key` first.",
        ));
    };
    let from_header = headers.get(KEY_HEADER).and_then(|v| v.to_str().ok());
    let from_bearer = allow_bearer
        .then(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
        .flatten();
    let supplied = from_header.or(from_bearer).unwrap_or_default();
    if supplied.is_empty() || !constant_time_eq(&hash_key(supplied), &stored) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing x-api-key",
        ));
    }
    Ok(())
}

/// `/api/v1` 用のミドルウェア。
pub async fn require_key(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    match check_key(&state, req.headers(), false).await {
        Ok(()) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}
