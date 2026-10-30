//! Web UI の結合テスト。axum の Router を `oneshot` で直接叩く (ネットワーク不要)。

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use time::OffsetDateTime;
use tower::ServiceExt;
use url::Url;
use watchtower_core::Watch;
use watchtower_store::Store;
use watchtower_web::{router, AppState, WebConfig};

const PASSWORD: &str = "correct horse battery";

struct App {
    router: Router,
    state: AppState,
    store: Store,
    _dir: tempfile::TempDir,
}

async fn app_with(cfg: WebConfig, password: Option<&str>) -> App {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let state = AppState::new(store.clone(), cfg);
    if let Some(p) = password {
        state.set_password(p).await.unwrap();
    }
    App {
        router: router(state.clone()),
        state,
        store,
        _dir: dir,
    }
}

async fn app() -> App {
    app_with(WebConfig::default(), Some(PASSWORD)).await
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
    fn set_cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string())
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
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// ログインして `name=value` 形式のセッション Cookie を返す。
async fn login(app: &App) -> String {
    let r = send(
        app,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    r.set_cookie()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// ページ内の hidden フィールド `csrf` の値を取り出す。
fn csrf_of(html: &str) -> String {
    let marker = "name=\"csrf\" value=\"";
    let start = html.find(marker).expect("csrf field") + marker.len();
    html[start..].split('"').next().unwrap().to_string()
}

async fn csrf(app: &App, cookie: &str) -> String {
    csrf_of(&send(app, get("/", Some(cookie))).await.body)
}

/// `<tag>` が名前付き (&lt;) または数値 (&#60;) の文字参照でエスケープされている。
fn is_escaped(html: &str, tag: &str) -> bool {
    html.contains(&format!("&lt;{tag}&gt;")) || html.contains(&format!("&#60;{tag}&#62;"))
}

fn watch(u: &str) -> Watch {
    Watch::new(Url::parse(u).unwrap())
}

// ---- 認証 ---------------------------------------------------------------

#[tokio::test]
async fn without_a_configured_password_everything_but_healthz_is_refused() {
    let a = app_with(WebConfig::default(), None).await;
    assert_eq!(
        send(&a, get("/", None)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let r = send(&a, get("/login", None)).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.body.contains("set-password"), "{}", r.body);
    assert_eq!(send(&a, get("/healthz", None)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn unauthenticated_requests_are_redirected_to_login() {
    let a = app().await;
    for path in ["/", "/watches/00000000-0000-0000-0000-000000000000"] {
        let r = send(&a, get(path, None)).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(r.header("location"), "/login");
    }
    let r = send(
        &a,
        post("/watches", None, "url=https%3A%2F%2Fe.com%2F&csrf=x"),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn wrong_password_is_rejected_without_a_session() {
    let a = app().await;
    let r = send(&a, post("/login", None, "password=nope")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.set_cookie().is_none());
}

#[tokio::test]
async fn login_sets_a_hardened_cookie_and_grants_access() {
    let a = app_with(
        WebConfig {
            secure_cookies: true,
            ..Default::default()
        },
        Some(PASSWORD),
    )
    .await;
    let r = send(
        &a,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let cookie = r.set_cookie().unwrap();
    for attr in ["HttpOnly", "SameSite=Lax", "Secure", "Path=/"] {
        assert!(cookie.contains(attr), "{attr} missing in {cookie}");
    }
    let pair = cookie.split(';').next().unwrap();
    assert_eq!(send(&a, get("/", Some(pair))).await.status, StatusCode::OK);
}

#[tokio::test]
async fn session_tokens_are_unpredictable() {
    let a = app().await;
    assert_ne!(login(&a).await, login(&a).await);
}

#[tokio::test]
async fn cross_origin_login_posts_are_refused() {
    let a = app().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::HOST, "wt.local:5000")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::from(format!("password={}", enc(PASSWORD))))
        .unwrap();
    let r = send(&a, req).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.set_cookie().is_none());

    let same = Request::builder()
        .method(Method::POST)
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::HOST, "wt.local:5000")
        .header(header::ORIGIN, "http://wt.local:5000")
        .body(Body::from(format!("password={}", enc(PASSWORD))))
        .unwrap();
    assert_eq!(send(&a, same).await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn repeated_failures_lock_out_even_the_right_password() {
    let a = app().await;
    for _ in 0..5 {
        assert_eq!(
            send(&a, post("/login", None, "password=bad")).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let r = send(
        &a,
        post("/login", None, &format!("password={}", enc(PASSWORD))),
    )
    .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.set_cookie().is_none());
}

#[tokio::test]
async fn expired_sessions_are_not_accepted() {
    let a = app_with(
        WebConfig {
            session_ttl: Duration::ZERO,
            ..Default::default()
        },
        Some(PASSWORD),
    )
    .await;
    let cookie = login(&a).await;
    assert_eq!(
        send(&a, get("/", Some(&cookie))).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn logout_invalidates_the_session() {
    let a = app().await;
    let cookie = login(&a).await;
    let token = csrf(&a, &cookie).await;
    let r = send(
        &a,
        post("/logout", Some(&cookie), &format!("csrf={}", enc(&token))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        send(&a, get("/", Some(&cookie))).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn forged_or_garbage_cookies_are_ignored() {
    let a = app().await;
    for c in [
        "wt_session=deadbeef",
        "wt_session=",
        "wt_session=%00%ff",
        "other=1",
    ] {
        assert_eq!(
            send(&a, get("/", Some(c))).await.status,
            StatusCode::SEE_OTHER,
            "{c}"
        );
    }
}

// ---- 画面 ---------------------------------------------------------------

#[tokio::test]
async fn responses_carry_security_headers() {
    let a = app().await;
    let r = send(&a, get("/login", None)).await;
    assert!(r
        .header("content-security-policy")
        .contains("default-src 'self'"));
    assert!(r
        .header("content-security-policy")
        .contains("frame-ancestors 'none'"));
    assert_eq!(r.header("x-content-type-options"), "nosniff");
    assert_eq!(r.header("x-frame-options"), "DENY");
    assert_eq!(r.header("referrer-policy"), "same-origin");
}

#[tokio::test]
async fn watch_titles_and_urls_are_html_escaped() {
    let a = app().await;
    let mut w = watch("https://example.com/?q=<b>");
    w.title = Some("<script>alert(1)</script>".into());
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get("/", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("<script>alert(1)</script>"), "{}", r.body);
    assert!(is_escaped(&r.body, "script"), "{}", r.body);
}

#[tokio::test]
async fn adding_a_watch_requires_a_valid_csrf_token() {
    let a = app().await;
    let cookie = login(&a).await;
    let form = "url=https%3A%2F%2Fexample.com%2F";
    assert_eq!(
        send(&a, post("/watches", Some(&cookie), form)).await.status,
        StatusCode::FORBIDDEN
    );
    let bad = format!("{form}&csrf=wrong");
    assert_eq!(
        send(&a, post("/watches", Some(&cookie), &bad)).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(a.store.list_watches().await.unwrap().is_empty());

    let token = csrf(&a, &cookie).await;
    let ok = format!("{form}&title=Ex&interval=120&csrf={}", enc(&token));
    let r = send(&a, post("/watches", Some(&cookie), &ok)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let ws = a.store.list_watches().await.unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].title.as_deref(), Some("Ex"));
    assert_eq!(ws[0].config.check_interval_secs, Some(120));
}

#[tokio::test]
async fn csrf_tokens_are_bound_to_the_session() {
    let a = app().await;
    let first = login(&a).await;
    let second = login(&a).await;
    let token_of_first = csrf(&a, &first).await;
    let form = format!("url=https%3A%2F%2Fe.com%2F&csrf={}", enc(&token_of_first));
    assert_eq!(
        send(&a, post("/watches", Some(&second), &form))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn invalid_watch_input_is_rejected() {
    let a = app().await;
    let cookie = login(&a).await;
    let token = enc(&csrf(&a, &cookie).await);
    for form in [
        format!("url=ftp%3A%2F%2Fe.com%2F&csrf={token}"),
        format!("url=not+a+url&csrf={token}"),
        format!("url=https%3A%2F%2Fe.com%2F&interval=0&csrf={token}"),
        format!("url=https%3A%2F%2Fe.com%2F&interval=abc&csrf={token}"),
        format!("url=&csrf={token}"),
    ] {
        let r = send(&a, post("/watches", Some(&cookie), &form)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{form}");
    }
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_watch_needs_csrf_and_removes_it() {
    let a = app().await;
    let w = watch("https://example.com/");
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/delete", w.id);
    assert_eq!(
        send(&a, post(&path, Some(&cookie), "")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(a.store.list_watches().await.unwrap().len(), 1);

    let token = csrf(&a, &cookie).await;
    let r = send(
        &a,
        post(&path, Some(&cookie), &format!("csrf={}", enc(&token))),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn watch_page_shows_the_latest_diff_escaped() {
    let a = app().await;
    let w = watch("https://example.com/");
    a.store.add_watch(&w).await.unwrap();
    let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .save_snapshot(w.id, t, "price: 10", [1; 32])
        .await
        .unwrap();
    a.store
        .save_snapshot(
            w.id,
            t + Duration::from_secs(60),
            "price: <i>20</i>",
            [2; 32],
        )
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}", w.id), Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("price: 10"), "{}", r.body);
    assert!(
        r.body.contains("price: &lt;i&gt;20&lt;/i&gt;")
            || r.body.contains("price: &#60;i&#62;20&#60;/i&#62;"),
        "{}",
        r.body
    );
    assert!(!r.body.contains("<i>20</i>"));
}

#[tokio::test]
async fn unknown_or_malformed_watch_ids_are_404() {
    let a = app().await;
    let cookie = login(&a).await;
    for p in [
        "/watches/00000000-0000-0000-0000-000000000000",
        "/watches/not-a-uuid",
    ] {
        assert_eq!(
            send(&a, get(p, Some(&cookie))).await.status,
            StatusCode::NOT_FOUND,
            "{p}"
        );
    }
}

#[tokio::test]
async fn oversized_forms_are_rejected() {
    let a = app().await;
    let big = format!("password={}", "a".repeat(200_000));
    let r = send(&a, post("/login", None, &big)).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn changing_the_password_replaces_the_old_one() {
    let a = app().await;
    a.state.set_password("new password 123").await.unwrap();
    assert_eq!(
        send(
            &a,
            post("/login", None, &format!("password={}", enc(PASSWORD)))
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&a, post("/login", None, "password=new+password+123"))
            .await
            .status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn short_passwords_are_refused_at_set_time() {
    let a = app_with(WebConfig::default(), None).await;
    assert!(a.state.set_password("short").await.is_err());
}

// ---- 編集 ---------------------------------------------------------------

async fn edit_form(app: &App, cookie: &str, extra: &str) -> String {
    format!("csrf={}&{extra}", enc(&csrf(app, cookie).await))
}

#[tokio::test]
async fn edit_page_shows_current_settings_escaped() {
    let a = app().await;
    let mut w = watch("https://example.com/a");
    w.title = Some("<b>T</b>".into());
    w.config.include_filters = vec![".price".into(), "#main".into()];
    w.config.notify_urls = vec!["json://hook.example.com/x".into()];
    w.config.check_interval_secs = Some(90);
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/edit", w.id), Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body.contains(".price\n#main") || r.body.contains(".price\r\n#main"),
        "{}",
        r.body
    );
    assert!(r.body.contains("json://hook.example.com/x"));
    assert!(r.body.contains("value=\"90\""));
    assert!(!r.body.contains("<b>T</b>"));
}

#[tokio::test]
async fn editing_updates_every_field_and_wakes_the_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("s"))
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let state = AppState::with_scheduler(store.clone(), WebConfig::default(), Some(tx));
    state.set_password(PASSWORD).await.unwrap();
    let a = App {
        router: router(state.clone()),
        state,
        store: store.clone(),
        _dir: dir,
    };
    let w = watch("https://example.com/a");
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;

    let body = edit_form(
        &a,
        &cookie,
        &format!(
            "url={}&title=New&interval=300&paused=on&notify_muted=on\
             &include_filters={}&exclude_filters=.ad&ignore_text=cookie&trigger_text=sold+out\
             &notify_urls={}",
            enc("https://example.com/b"),
            enc(".price\r\n#main\r\n\r\n"),
            enc("json://hook.example.com/x\r\ndiscord://1/tok")
        ),
    )
    .await;
    let r = send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &body),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);

    let got = a.store.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.url.as_str(), "https://example.com/b");
    assert_eq!(got.title.as_deref(), Some("New"));
    assert!(got.paused);
    assert_eq!(got.config.check_interval_secs, Some(300));
    assert_eq!(got.config.notify_muted, Some(true));
    assert_eq!(got.config.include_filters, vec![".price", "#main"]);
    assert_eq!(got.config.exclude_filters, vec![".ad"]);
    assert_eq!(got.config.ignore_text, vec!["cookie"]);
    assert_eq!(got.config.trigger_text, vec!["sold out"]);
    assert_eq!(got.config.notify_urls.len(), 2);
    assert!(matches!(rx.try_recv(), Ok(watchtower_scheduler::Command::Upsert(id)) if id == w.id));
}

#[tokio::test]
async fn unchecked_boxes_clear_flags_and_blank_interval_means_default() {
    let a = app().await;
    let mut w = watch("https://example.com/a");
    w.paused = true;
    w.config.check_interval_secs = Some(60);
    w.config.notify_muted = Some(true);
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let body = edit_form(&a, &cookie, "url=https%3A%2F%2Fexample.com%2Fa&interval=").await;
    let r = send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &body),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let got = a.store.get_watch(w.id).await.unwrap().unwrap();
    assert!(!got.paused);
    assert_eq!(got.config.check_interval_secs, None);
    assert_ne!(got.config.notify_muted, Some(true));
}

#[tokio::test]
async fn editing_needs_csrf_and_authentication() {
    let a = app().await;
    let w = watch("https://example.com/a");
    a.store.add_watch(&w).await.unwrap();
    let path = format!("/watches/{}/edit", w.id);
    let form = "url=https%3A%2F%2Fevil.example%2F";
    assert_eq!(
        send(&a, post(&path, None, form)).await.status,
        StatusCode::SEE_OTHER
    );
    let cookie = login(&a).await;
    assert_eq!(
        send(&a, post(&path, Some(&cookie), form)).await.status,
        StatusCode::FORBIDDEN
    );
    let got = a.store.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.url.as_str(), "https://example.com/a");
}

#[tokio::test]
async fn invalid_edits_are_rejected_without_changing_anything_or_echoing_secrets() {
    let a = app().await;
    let w = watch("https://example.com/a");
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let base = "url=https%3A%2F%2Fexample.com%2Fa";
    let too_many = enc(&"x\n".repeat(60));
    for extra in [
        "url=ftp%3A%2F%2Fx".to_string(),
        format!("{base}&interval=0"),
        format!("{base}&notify_urls={}", enc("bogus://SECRETtoken/x")),
        format!("{base}&include_filters={too_many}"),
        format!("{base}&ignore_text={}", enc(&"y".repeat(600))),
    ] {
        let body = edit_form(&a, &cookie, &extra).await;
        let r = send(&a, post(&path, Some(&cookie), &body)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{extra}");
        assert!(!r.body.contains("SECRET"), "{}", r.body);
    }
    assert_eq!(a.store.get_watch(w.id).await.unwrap().unwrap(), w);
}

#[tokio::test]
async fn editing_does_not_disturb_check_state() {
    let a = app().await;
    let mut w = watch("https://example.com/a");
    w.state.consecutive_errors = 4;
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let body = edit_form(&a, &cookie, "url=https%3A%2F%2Fexample.com%2Fa&title=X").await;
    send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &body),
    )
    .await;
    let got = a.store.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.state.consecutive_errors, 4);
}

// ---- レビュー指摘への対応 -----------------------------------------------

fn login_from(ip: &str, password: &str) -> Request<Body> {
    let mut req = post("/login", None, &format!("password={}", enc(password)));
    let addr: std::net::SocketAddr = format!("{ip}:40000").parse().unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    req
}

#[tokio::test]
async fn parallel_guesses_cannot_exceed_the_failure_budget() {
    let a = app().await;
    let mut tasks = Vec::new();
    for _ in 0..30 {
        let router = a.router.clone();
        tasks.push(tokio::spawn(async move {
            router
                .oneshot(post("/login", None, "password=wrong"))
                .await
                .unwrap()
                .status()
        }));
    }
    let mut verified = 0;
    for t in tasks {
        if t.await.unwrap() == StatusCode::UNAUTHORIZED {
            verified += 1;
        }
    }
    assert!(
        verified <= 5,
        "{verified} guesses were verified; the budget is 5"
    );
}

#[tokio::test]
async fn one_client_cannot_lock_out_the_owner() {
    let a = app().await;
    for _ in 0..5 {
        let r = a
            .router
            .clone()
            .oneshot(login_from("203.0.113.9", "bad"))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    let blocked = a
        .router
        .clone()
        .oneshot(login_from("203.0.113.9", PASSWORD))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    let owner = a
        .router
        .clone()
        .oneshot(login_from("198.51.100.7", PASSWORD))
        .await
        .unwrap();
    assert_eq!(owner.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn a_successful_login_does_not_consume_the_failure_budget() {
    let a = app().await;
    for _ in 0..20 {
        let r = a
            .router
            .clone()
            .oneshot(login_from("198.51.100.7", PASSWORD))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
    }
}

#[tokio::test]
async fn changing_the_password_invalidates_existing_sessions() {
    let a = app().await;
    let cookie = login(&a).await;
    assert_eq!(
        send(&a, get("/", Some(&cookie))).await.status,
        StatusCode::OK
    );
    a.state.set_password("another long password").await.unwrap();
    assert_eq!(
        send(&a, get("/", Some(&cookie))).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn hsts_is_sent_only_when_cookies_are_secure() {
    let plain = app().await;
    assert_eq!(
        send(&plain, get("/login", None))
            .await
            .header("strict-transport-security"),
        ""
    );
    let tls = app_with(
        WebConfig {
            secure_cookies: true,
            ..Default::default()
        },
        Some(PASSWORD),
    )
    .await;
    assert!(send(&tls, get("/login", None))
        .await
        .header("strict-transport-security")
        .contains("max-age="));
}

// ---- 履歴 ---------------------------------------------------------------

#[tokio::test]
async fn events_page_lists_recent_events_newest_first_with_escaping() {
    let a = app().await;
    let mut w = watch("https://example.com/");
    w.title = Some("Shop".into());
    a.store.add_watch(&w).await.unwrap();
    let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .record_event(Some(w.id), "changed", t, Some("+1 -1"))
        .await
        .unwrap();
    a.store
        .record_event(
            Some(w.id),
            "check_failed",
            t + Duration::from_secs(60),
            Some("<img src=x>"),
        )
        .await
        .unwrap();
    a.store
        .record_event(None, "started", t + Duration::from_secs(120), None)
        .await
        .unwrap();

    let cookie = login(&a).await;
    let r = send(&a, get("/events", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    let (s, f, c) = (
        r.body.find("started").unwrap(),
        r.body.find("check_failed").unwrap(),
        r.body.find("changed").unwrap(),
    );
    assert!(s < f && f < c, "newest first");
    assert!(r.body.contains("Shop"));
    assert!(!r.body.contains("<img src=x>"), "{}", r.body);
    assert!(is_escaped(&r.body, "img src=x"), "{}", r.body);
}

#[tokio::test]
async fn events_page_requires_login() {
    let a = app().await;
    assert_eq!(
        send(&a, get("/events", None)).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn watch_page_shows_only_its_own_events() {
    let a = app().await;
    let (w1, w2) = (
        watch("https://example.com/1"),
        watch("https://example.com/2"),
    );
    a.store.add_watch(&w1).await.unwrap();
    a.store.add_watch(&w2).await.unwrap();
    let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .record_event(Some(w1.id), "mine_event", t, None)
        .await
        .unwrap();
    a.store
        .record_event(Some(w2.id), "other_event", t, None)
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}", w1.id), Some(&cookie))).await;
    assert!(r.body.contains("mine_event") && !r.body.contains("other_event"));
}
