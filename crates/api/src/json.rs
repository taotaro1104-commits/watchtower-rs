//! JSON 本文の取り出し。失敗はすべて `{"message": ...}` で返す。

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{header, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::ApiError;

pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
        if !is_json {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            ));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|e| {
            ApiError::new(e.status(), "The request body is too large or unreadable")
        })?;
        serde_json::from_slice(&bytes)
            .map(ApiJson)
            .map_err(|e| ApiError::bad_request(format!("Invalid JSON: {e}")))
    }
}
