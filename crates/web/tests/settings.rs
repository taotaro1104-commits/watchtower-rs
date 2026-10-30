//! 設定画面 (共通の通知先・パスワード変更) とインポート画面。

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use watchtower_store::Store;
use watchtower_web::{router, AppState, WebConfig};

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "brand new password";

struct App {
    router: Router,
    state: AppState,
    store: Store,
    _dir: tempfile::TempDir,
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Resp {
    fn set_cookie_pair(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
    }
}

async fn app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let state = AppState::new(store.clone(), WebConfig::default());
    state.set_password(PASSWORD).await.unwrap();
    App {
        router: router(state.clone()),
        state,
        store,
        _dir: dir,
    }
}

async fn send(app: &App, req: Request<Body>) -> Resp {
    let res = app.router.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(path);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

fn post(path: &str, cookie: Option<&str>, form: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::from(form.to_string())).unwrap()
}

fn enc(s: &str) -> String {
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

async fn login(app: &App) -> String {
    let r = send(
        app,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    r.set_cookie_pair().unwrap()
}

async fn csrf(app: &App, cookie: &str) -> String {
    let html = send(app, get("/", Some(cookie))).await.body;
    let marker = "name=\"csrf\" value=\"";
    let start = html.find(marker).expect("csrf field") + marker.len();
    html[start..].split('"').next().unwrap().to_string()
}

fn multipart(path: &str, cookie: &str, csrf: &str, file: Option<(&str, &str)>) -> Request<Body> {
    let b = "XBOUNDARYX";
    let mut body =
        format!("--{b}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n");
    if let Some((name, content)) = file {
        body.push_str(&format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
             Content-Type: application/json\r\n\r\n{content}\r\n"
        ));
    }
    body.push_str(&format!("--{b}--\r\n"));
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={b}"),
        )
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

// ---- 共通の通知先 ----------------------------------------------------------

#[tokio::test]
async fn settings_page_requires_login_and_shows_saved_urls_escaped() {
    let a = app().await;
    assert_eq!(
        send(&a, get("/settings", None)).await.status,
        StatusCode::SEE_OTHER
    );
    a.store
        .set_global_notify_urls(&["json://h.example.com/<x>".to_string()])
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get("/settings", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("json://h.example.com/"));
    assert!(!r.body.contains("/<x>"), "{}", r.body);
}

#[tokio::test]
async fn global_notify_urls_are_validated_and_saved() {
    let a = app().await;
    let cookie = login(&a).await;
    let token = csrf(&a, &cookie).await;
    let ok = format!(
        "csrf={}&notify_urls={}",
        enc(&token),
        enc("json://h.example.com/x\r\ndiscord://1/tok")
    );
    let r = send(&a, post("/settings/notify", Some(&cookie), &ok)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(a.store.global_notify_urls().await.unwrap().len(), 2);

    let bad = format!(
        "csrf={}&notify_urls={}",
        enc(&token),
        enc("bogus://SECRETtoken/x")
    );
    let r = send(&a, post("/settings/notify", Some(&cookie), &bad)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!r.body.contains("SECRET"));
    assert_eq!(
        a.store.global_notify_urls().await.unwrap().len(),
        2,
        "unchanged"
    );

    let forged = "notify_urls=json%3A%2F%2Fh%2Fx";
    let r = send(&a, post("/settings/notify", Some(&cookie), forged)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

// ---- パスワード変更 --------------------------------------------------------

fn pw_form(csrf: &str, current: &str, new: &str, confirm: &str) -> String {
    format!(
        "csrf={}&current={}&new={}&confirm={}",
        enc(csrf),
        enc(current),
        enc(new),
        enc(confirm)
    )
}

async fn change(a: &App, cookie: &str, form: String) -> Resp {
    send(a, post("/settings/password", Some(cookie), &form)).await
}

#[tokio::test]
async fn password_change_requires_the_current_password_and_a_valid_new_one() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    let wrong = change(
        &a,
        &cookie,
        pw_form(&t, "wrong", NEW_PASSWORD, NEW_PASSWORD),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    let short = change(&a, &cookie, pw_form(&t, PASSWORD, "short", "short")).await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    let mismatch = change(
        &a,
        &cookie,
        pw_form(&t, PASSWORD, NEW_PASSWORD, "different one"),
    )
    .await;
    assert_eq!(mismatch.status, StatusCode::BAD_REQUEST);
    // どれも反映されていない
    let again = send(
        &a,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(again.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn password_change_keeps_this_session_and_drops_the_others() {
    let a = app().await;
    let mine = login(&a).await;
    let other = login(&a).await;
    let t = csrf(&a, &mine).await;
    let r = change(&a, &mine, pw_form(&t, PASSWORD, NEW_PASSWORD, NEW_PASSWORD)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let fresh = r.set_cookie_pair().expect("a fresh session cookie");

    assert_eq!(
        send(&a, get("/", Some(&fresh))).await.status,
        StatusCode::OK
    );
    assert_eq!(
        send(&a, get("/", Some(&other))).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&a, get("/", Some(&mine))).await.status,
        StatusCode::SEE_OTHER
    );
    let login_new = post("/login", None, &format!("password={}", enc(NEW_PASSWORD)));
    assert_eq!(send(&a, login_new).await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn password_change_needs_csrf() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = change(
        &a,
        &cookie,
        pw_form("wrong", PASSWORD, NEW_PASSWORD, NEW_PASSWORD),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn wrong_current_passwords_count_against_the_login_budget() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    for _ in 0..5 {
        let r = change(&a, &cookie, pw_form(&t, "nope", NEW_PASSWORD, NEW_PASSWORD)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = change(
        &a,
        &cookie,
        pw_form(&t, PASSWORD, NEW_PASSWORD, NEW_PASSWORD),
    )
    .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // state は未使用だが、パスワードが変わっていないことを確認する
    assert!(a.state.set_password(PASSWORD).await.is_ok());
}

// ---- インポート ------------------------------------------------------------

const IMPORT_JSON: &str = r#"{"watching": {
  "11111111-1111-4111-8111-111111111111": {"url": "https://example.com/a", "title": "Imported <b>"},
  "22222222-2222-4222-8222-222222222222": {"url": "ftp://bad"}
}}"#;

#[tokio::test]
async fn importing_a_changedetection_file_creates_watches_and_reports() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    let req = multipart(
        "/settings/import",
        &cookie,
        &t,
        Some(("url-watches.json", IMPORT_JSON)),
    );
    let r = send(&a, req).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        r.body.contains("Imported 1") && r.body.contains("skipped"),
        "{}",
        r.body
    );
    assert_eq!(a.store.list_watches().await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_rejects_missing_csrf_bad_json_and_missing_file() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    let forged = multipart(
        "/settings/import",
        &cookie,
        "wrong",
        Some(("f.json", IMPORT_JSON)),
    );
    assert_eq!(send(&a, forged).await.status, StatusCode::FORBIDDEN);
    let bad = multipart(
        "/settings/import",
        &cookie,
        &t,
        Some(("f.json", "{not json")),
    );
    assert_eq!(send(&a, bad).await.status, StatusCode::BAD_REQUEST);
    let none = multipart("/settings/import", &cookie, &t, None);
    assert_eq!(send(&a, none).await.status, StatusCode::BAD_REQUEST);
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn import_accepts_large_files_but_not_unbounded_ones() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    let big = format!("{}{IMPORT_JSON}", " ".repeat(2 * 1024 * 1024));
    let r = send(
        &a,
        multipart("/settings/import", &cookie, &t, Some(("f.json", &big))),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "2 MiB must be accepted");
    let huge = " ".repeat(40 * 1024 * 1024);
    let r = send(
        &a,
        multipart("/settings/import", &cookie, &t, Some(("f.json", &huge))),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}
