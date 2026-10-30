//! API のエラー。本文は常に `{"message": "..."}` の JSON。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} not found"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "message": self.message }))).into_response()
    }
}

/// 内部エラーは詳細をログにだけ残し、利用者には汎用の文面を返す。
impl From<watchtower_store::StoreError> for ApiError {
    fn from(e: watchtower_store::StoreError) -> Self {
        match e {
            watchtower_store::StoreError::LimitExceeded(msg) => Self::bad_request(msg),
            other => {
                tracing::error!("api request failed: {other}");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
