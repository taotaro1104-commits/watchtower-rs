//! Host ヘッダの許可リスト (DNS rebinding 対策)。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;
use watchtower_web::host::{guard, HostPolicy};

fn policy(extra: &[&str]) -> HostPolicy {
    HostPolicy::new("http://watch.example.com:5000", extra).unwrap()
}

fn allows(p: &HostPolicy, host: &str) -> bool {
    p.allows(Some(host))
}

#[test]
fn loopback_names_the_base_url_and_extras_are_allowed() {
    let p = policy(&["Alias.Example.org"]);
    for ok in [
        "localhost",
        "localhost:5000",
        "LOCALHOST",
        "watch.example.com",
        "watch.example.com:5000",
        "watch.example.com.",
        "alias.example.org:8443",
    ] {
        assert!(allows(&p, ok), "{ok}");
    }
}

#[test]
fn ip_literals_are_allowed_because_rebinding_needs_a_name() {
    let p = policy(&[]);
    for ok in [
        "127.0.0.1:5000",
        "192.168.1.10",
        "[::1]:5000",
        "[2001:db8::1]",
    ] {
        assert!(allows(&p, ok), "{ok}");
    }
}

#[test]
fn other_names_are_refused() {
    let p = policy(&[]);
    for bad in [
        "evil.example",
        "evil.example:5000",
        "watch.example.com.evil.example",
        "xwatch.example.com",
        "localhost.evil.example",
        "",
        "user@watch.example.com",
        "watch.example.com:notaport",
    ] {
        assert!(!allows(&p, bad), "{bad}");
    }
}

#[test]
fn a_missing_host_is_allowed_since_browsers_always_send_one() {
    assert!(policy(&[]).allows(None));
}

#[test]
fn bad_configuration_is_rejected_at_startup() {
    assert!(HostPolicy::new("not a url", &[] as &[&str]).is_err());
    for bad in ["", "a b", "http://x", "x/y", "*.example.com", "host:80"] {
        assert!(
            HostPolicy::new("http://localhost", &[bad]).is_err(),
            "{bad}"
        );
    }
}

async fn status(p: HostPolicy, host: Option<&str>) -> (StatusCode, String) {
    let app = Router::new()
        .route("/", get(|| async { "hello" }))
        .layer(axum::middleware::from_fn_with_state(p, guard));
    let mut req = Request::builder().uri("/");
    if let Some(h) = host {
        req = req.header(header::HOST, h);
    }
    let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let st = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1 << 16)
        .await
        .unwrap();
    (st, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn the_middleware_serves_allowed_hosts_and_refuses_others() {
    let (st, body) = status(policy(&[]), Some("localhost:5000")).await;
    assert_eq!((st, body.as_str()), (StatusCode::OK, "hello"));

    let (st, body) = status(policy(&[]), Some("evil.example")).await;
    assert_eq!(st, StatusCode::MISDIRECTED_REQUEST);
    assert!(!body.contains("evil.example"), "{body}");
}

#[tokio::test]
async fn the_uri_authority_is_checked_when_there_is_no_host_header() {
    // HTTP/2 では Host ヘッダの代わりに :authority (URI の authority) が届く
    let app = Router::new()
        .route("/", get(|| async { "hello" }))
        .layer(axum::middleware::from_fn_with_state(policy(&[]), guard));
    for (authority, expected) in [
        ("evil.example", StatusCode::MISDIRECTED_REQUEST),
        ("localhost:5000", StatusCode::OK),
    ] {
        let req = Request::builder()
            .uri(format!("http://{authority}/"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), expected, "{authority}");
    }
}
