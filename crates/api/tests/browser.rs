//! API の `fetch_backend` (本家互換): system / html_requests / html_webdriver。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn the_backend_can_be_set_read_and_changed() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/js", "fetch_backend": "html_webdriver"}),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    assert_eq!(
        get_ok(&a, &path).await.json()["fetch_backend"],
        "html_webdriver"
    );

    let r = call(
        &a,
        Method::PUT,
        &path,
        &json!({"fetch_backend": "html_requests"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        get_ok(&a, &path).await.json()["fetch_backend"],
        "html_requests"
    );

    // 触れない更新では変わらない
    call(&a, Method::PUT, &path, &json!({"title": "x"})).await;
    assert_eq!(
        get_ok(&a, &path).await.json()["fetch_backend"],
        "html_requests"
    );

    let r = call(&a, Method::PUT, &path, &json!({"fetch_backend": "system"})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(get_ok(&a, &path).await.json()["fetch_backend"], "system");
}

#[tokio::test]
async fn watches_default_to_system() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/"})).await;
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.json()["fetch_backend"],
        "system"
    );
}

#[tokio::test]
async fn unknown_backends_are_rejected_and_change_nothing() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/", "fetch_backend": "html_webdriver"}),
    )
    .await;
    for bad in [
        json!("chrome"),
        json!(""),
        json!(1),
        json!(null),
        json!(true),
    ] {
        let r = call(
            &a,
            Method::POST,
            "/api/v1/watch",
            &json!({"url": "https://example.com/x", "fetch_backend": bad}),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let path = format!("/api/v1/watch/{id}");
    let r = call(&a, Method::PUT, &path, &json!({"fetch_backend": "nope"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get_ok(&a, &path).await.json()["fetch_backend"],
        "html_webdriver"
    );
}
