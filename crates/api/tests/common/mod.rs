//! 結合テスト共通のヘルパー。

#![allow(dead_code)] // テストファイルごとに使うものが異なる

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tower::ServiceExt;
use watchtower_api::{router, ApiState};
use watchtower_scheduler::Command;
use watchtower_store::Store;

pub struct Api {
    pub router: Router,
    pub state: ApiState,
    pub store: Store,
    pub key: String,
    pub commands: mpsc::Receiver<Command>,
    pub _dir: tempfile::TempDir,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
}

/// キー設定済みの API を用意する。
pub async fn api() -> Api {
    let mut a = api_without_key().await;
    a.key = a.state.rotate_key().await.unwrap();
    a
}

pub async fn api_without_key() -> Api {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let (tx, commands) = mpsc::channel(64);
    let state = ApiState::new(store.clone(), Some(tx));
    Api {
        router: router(state.clone()),
        state,
        store,
        key: String::new(),
        commands,
        _dir: dir,
    }
}

pub async fn send(a: &Api, req: Request<Body>) -> Resp {
    let res = a.router.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn builder(method: Method, path: &str, key: Option<&str>) -> axum::http::request::Builder {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(k) = key {
        b = b.header("x-api-key", k);
    }
    b
}

pub fn get(path: &str, key: Option<&str>) -> Request<Body> {
    builder(Method::GET, path, key).body(Body::empty()).unwrap()
}

pub fn delete(path: &str, key: Option<&str>) -> Request<Body> {
    builder(Method::DELETE, path, key)
        .body(Body::empty())
        .unwrap()
}

pub fn json_req(method: Method, path: &str, key: Option<&str>, body: &Value) -> Request<Body> {
    builder(method, path, key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn text_post(path: &str, key: Option<&str>, body: &str) -> Request<Body> {
    builder(Method::POST, path, key)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 認証済みで JSON を POST / PUT する。
pub async fn call(a: &Api, method: Method, path: &str, body: &Value) -> Resp {
    send(a, json_req(method, path, Some(&a.key), body)).await
}

pub async fn get_ok(a: &Api, path: &str) -> Resp {
    send(a, get(path, Some(&a.key))).await
}

/// ウォッチを作って uuid を返す。
pub async fn create_watch(a: &Api, body: Value) -> String {
    let r = call(a, Method::POST, "/api/v1/watch", &body).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.json()["uuid"].as_str().unwrap().to_string()
}
