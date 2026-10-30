//! 結合テスト共通のヘルパー。

#![allow(dead_code)] // テストファイルごとに使うものが異なる

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use watchtower_store::Store;
use watchtower_web::{router, AppState, WebConfig};

pub const PASSWORD: &str = "correct horse battery";

pub struct App {
    pub router: Router,
    pub state: AppState,
    pub store: Store,
    pub _dir: tempfile::TempDir,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Resp {
    pub fn set_cookie_pair(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
    }
}

pub async fn app() -> App {
    app_with(WebConfig::default()).await
}

pub async fn app_with(cfg: WebConfig) -> App {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let state = AppState::new(store.clone(), cfg);
    state.set_password(PASSWORD).await.unwrap();
    App {
        router: router(state.clone()),
        state,
        store,
        _dir: dir,
    }
}

pub async fn send(app: &App, req: Request<Body>) -> Resp {
    let res = app.router.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

pub fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(path);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

pub fn post(path: &str, cookie: Option<&str>, form: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::from(form.to_string())).unwrap()
}

pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub async fn login(app: &App) -> String {
    let r = send(
        app,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    r.set_cookie_pair().unwrap()
}

pub async fn csrf(app: &App, cookie: &str) -> String {
    let html = send(app, get("/", Some(cookie))).await.body;
    let marker = "name=\"csrf\" value=\"";
    let start = html.find(marker).expect("csrf field") + marker.len();
    html[start..].split('"').next().unwrap().to_string()
}

/// `<tag>` が名前付き (&lt;) または数値 (&#60;) の文字参照でエスケープされている。
pub fn is_escaped(html: &str, tag: &str) -> bool {
    html.contains(&format!("&lt;{tag}&gt;")) || html.contains(&format!("&#60;{tag}&#62;"))
}
