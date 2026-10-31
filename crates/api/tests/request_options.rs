//! API の `method` / `headers` / `body` / `proxy` (ウォッチごとの取得オプション)。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn options_can_be_set_read_changed_and_cleared() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({
            "url": "https://example.com/form",
            "method": "post",
            "headers": {"X-Token": "abc"},
            "body": "a=1",
            "proxy": "http://proxy.example:8080"
        }),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    let v = get_ok(&a, &path).await.json();
    assert_eq!(v["method"], "POST");
    assert_eq!(v["headers"], json!({"X-Token": "abc"}));
    assert_eq!(v["body"], "a=1");
    assert_eq!(v["proxy"], "http://proxy.example:8080");

    // 触れない更新では変わらない
    call(&a, Method::PUT, &path, &json!({"title": "x"})).await;
    assert_eq!(
        get_ok(&a, &path).await.json()["headers"],
        json!({"X-Token": "abc"})
    );

    let r = call(
        &a,
        Method::PUT,
        &path,
        &json!({"headers": {}, "body": null, "proxy": null, "method": "GET"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let v = get_ok(&a, &path).await.json();
    assert_eq!(v["headers"], json!({}));
    assert!(v["body"].is_null() && v["proxy"].is_null());
    assert_eq!(v["method"], "GET");
}

#[tokio::test]
async fn a_new_watch_defaults_to_a_plain_get() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/"})).await;
    let v = get_ok(&a, &format!("/api/v1/watch/{id}")).await.json();
    assert_eq!(v["method"], "GET");
    assert_eq!(v["headers"], json!({}));
}

#[tokio::test]
async fn bad_options_are_a_400_and_never_echo_secrets() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/"})).await;
    let path = format!("/api/v1/watch/{id}");
    let bodies = [
        json!({"method": "CONNECT"}),
        json!({"headers": null}),
        json!({"headers": {"Host": "evil.example"}}),
        json!({"headers": {"X": "a\r\nInjected: 1"}}),
        json!({"proxy": "ftp://user:hunter2@proxy.example"}),
    ];
    for b in bodies {
        let r = call(&a, Method::PUT, &path, &b).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{b}");
        assert!(!r.body.contains("hunter2"), "{}", r.body);
    }
    assert_eq!(get_ok(&a, &path).await.json()["method"], "GET");
}

#[tokio::test]
async fn a_null_time_between_check_goes_back_to_the_default() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/", "time_between_check": {"minutes": 5}}),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    assert_eq!(
        get_ok(&a, &path).await.json()["time_between_check"]["minutes"],
        5
    );

    // 触れない更新では変わらない
    call(&a, Method::PUT, &path, &json!({"title": "x"})).await;
    assert_eq!(
        get_ok(&a, &path).await.json()["time_between_check"]["minutes"],
        5
    );

    let r = call(&a, Method::PUT, &path, &json!({"time_between_check": null})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(get_ok(&a, &path).await.json()["time_between_check"].is_null());
}
