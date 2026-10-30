//! `/metrics` (Prometheus)。キー認証つき、`Authorization: Bearer` でも受け付ける。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde_json::json;
use tokio::sync::mpsc;
use watchtower_api::{router, ApiState};
use watchtower_metrics::Metrics;
use watchtower_store::Store;

fn with_bearer(token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri("/metrics")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn line_value(body: &str, name: &str) -> f64 {
    body.lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap_or_else(|| panic!("{name} missing in:\n{body}"))
        .parse()
        .unwrap()
}

#[tokio::test]
async fn metrics_require_the_key() {
    let a = api().await;
    assert_eq!(
        send(&a, get("/metrics", None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&a, get("/metrics", Some("wt_wrong"))).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&a, with_bearer("wt_wrong")).await.status,
        StatusCode::UNAUTHORIZED
    );
    let basic = Request::builder()
        .uri("/metrics")
        .header(header::AUTHORIZATION, format!("Basic {}", a.key))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&a, basic).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn metrics_are_closed_until_a_key_exists() {
    let a = api_without_key().await;
    assert_eq!(
        send(&a, get("/metrics", None)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn both_header_styles_are_accepted() {
    let a = api().await;
    let by_header = send(&a, get("/metrics", Some(&a.key))).await;
    assert_eq!(by_header.status, StatusCode::OK);
    let by_bearer = send(&a, with_bearer(&a.key)).await;
    assert_eq!(by_bearer.status, StatusCode::OK);
}

#[tokio::test]
async fn bearer_is_not_accepted_on_the_rest_api() {
    let a = api().await;
    let req = Request::builder()
        .uri("/api/v1/watch")
        .header(header::AUTHORIZATION, format!("Bearer {}", a.key))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&a, req).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_response_is_prometheus_text_and_not_cacheable() {
    let a = api().await;
    let r = send(&a, get("/metrics", Some(&a.key))).await;
    let ct = r.headers[header::CONTENT_TYPE].to_str().unwrap();
    assert!(
        ct.starts_with("text/plain") && ct.contains("version=0.0.4"),
        "{ct}"
    );
    assert_eq!(r.headers["cache-control"], "no-store");
    assert!(r.body.contains("# TYPE watchtower_checks_total counter"));
}

#[tokio::test]
async fn gauges_reflect_the_database_at_scrape_time() {
    let a = api().await;
    create_watch(&a, json!({"url": "https://example.com/a"})).await;
    create_watch(&a, json!({"url": "https://example.com/b", "paused": true})).await;
    a.store
        .enqueue_notification(
            &watchtower_store::NewNotification {
                watch_id: None,
                target: "json://h/x".into(),
                title: "t".into(),
                body: "b".into(),
            },
            time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        )
        .await
        .unwrap();
    let body = send(&a, get("/metrics", Some(&a.key))).await.body;
    assert_eq!(line_value(&body, "watchtower_watches"), 2.0);
    assert_eq!(line_value(&body, "watchtower_watches_paused"), 1.0);
    assert_eq!(line_value(&body, "watchtower_queue_length"), 1.0);
    assert_eq!(line_value(&body, "watchtower_outbox_pending"), 1.0);
    assert_eq!(line_value(&body, "watchtower_outbox_failed"), 0.0);
}

#[tokio::test]
async fn counters_come_from_the_shared_registry() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let metrics = Arc::new(Metrics::new());
    let (tx, _rx) = mpsc::channel(4);
    let state = ApiState::with_metrics(store, Some(tx), Arc::clone(&metrics));
    let key = state.rotate_key().await.unwrap();
    let app = router(state);

    metrics.check_finished(true, false);
    metrics.check_finished(false, true);
    let res = tower::ServiceExt::oneshot(app, get("/metrics", Some(&key)))
        .await
        .unwrap();
    use http_body_util::BodyExt;
    let body =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert_eq!(line_value(&body, "watchtower_checks_total"), 2.0);
    assert_eq!(line_value(&body, "watchtower_check_failures_total"), 1.0);
    assert_eq!(line_value(&body, "watchtower_changes_total"), 1.0);
}
