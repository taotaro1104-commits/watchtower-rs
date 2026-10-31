//! API の `browser_steps` (ブラウザ操作ステップ)。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

fn steps() -> serde_json::Value {
    json!([
        {"op": "click", "selector": "#accept"},
        {"op": "type", "selector": "input[name=q]", "text": "hello"},
        {"op": "wait_for", "selector": ".result", "timeout_ms": 3000},
        {"op": "wait", "ms": 100}
    ])
}

#[tokio::test]
async fn steps_can_be_set_read_replaced_and_cleared() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/js", "fetch_backend": "html_webdriver", "browser_steps": steps()}),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    assert_eq!(get_ok(&a, &path).await.json()["browser_steps"], steps());

    // 触れない更新では変わらない
    call(&a, Method::PUT, &path, &json!({"title": "x"})).await;
    assert_eq!(get_ok(&a, &path).await.json()["browser_steps"], steps());

    let one = json!([{"op": "click", "selector": "#only"}]);
    let r = call(&a, Method::PUT, &path, &json!({"browser_steps": one})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(get_ok(&a, &path).await.json()["browser_steps"], one);

    let r = call(&a, Method::PUT, &path, &json!({"browser_steps": []})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(get_ok(&a, &path).await.json()["browser_steps"], json!([]));
}

#[tokio::test]
async fn watches_have_no_steps_by_default() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/"})).await;
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.json()["browser_steps"],
        json!([])
    );
}

#[tokio::test]
async fn invalid_steps_are_rejected_and_change_nothing() {
    let a = api().await;
    let good = json!([{"op": "wait", "ms": 5}]);
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/", "fetch_backend": "html_webdriver", "browser_steps": good}),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    let too_many: Vec<_> = (0..21).map(|_| json!({"op": "wait", "ms": 1})).collect();
    for bad in [
        json!([{"op": "eval", "code": "1"}]),
        json!([{"op": "click"}]),
        json!([{"op": "click", "selector": ""}]),
        json!([{"op": "click", "selector": "a", "extra": 1}]),
        json!([{"op": "wait", "ms": 999999}]),
        json!(too_many),
        json!("click #a"),
        json!(null),
    ] {
        let r = call(&a, Method::PUT, &path, &json!({"browser_steps": bad})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    assert_eq!(get_ok(&a, &path).await.json()["browser_steps"], good);
}

#[tokio::test]
async fn steps_are_refused_for_an_explicit_plain_http_watch() {
    let a = api().await;
    let r = call(
        &a,
        Method::POST,
        "/api/v1/watch",
        &json!({"url": "https://example.com/", "fetch_backend": "html_requests", "browser_steps": steps()}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
}

#[tokio::test]
async fn typed_text_is_not_echoed_in_errors() {
    let a = api().await;
    let r = call(
        &a,
        Method::POST,
        "/api/v1/watch",
        &json!({"url": "https://example.com/", "browser_steps": [
            {"op": "type", "selector": "", "text": "hunter2-SECRET-TEXT"}]}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!r.body.contains("SECRET"), "{}", r.body);
}
