//! API で価格の閾値 (`price_below`) を扱えること。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

#[tokio::test]
async fn the_threshold_can_be_set_read_changed_and_cleared() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/item", "price_below": 20}),
    )
    .await;
    let path = format!("/api/v1/watch/{id}");
    assert_eq!(get_ok(&a, &path).await.json()["price_below"], json!(20.0));

    let r = call(&a, Method::PUT, &path, &json!({"price_below": 12.5})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(get_ok(&a, &path).await.json()["price_below"], json!(12.5));

    // 触れない更新では変わらない
    call(&a, Method::PUT, &path, &json!({"title": "x"})).await;
    assert_eq!(get_ok(&a, &path).await.json()["price_below"], json!(12.5));

    // null で解除
    let r = call(&a, Method::PUT, &path, &json!({"price_below": null})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(get_ok(&a, &path).await.json()["price_below"], Value::Null);
}

#[tokio::test]
async fn watches_without_a_threshold_report_null() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/item"})).await;
    let w = get_ok(&a, &format!("/api/v1/watch/{id}")).await.json();
    assert!(w.get("price_below").is_some());
    assert_eq!(w["price_below"], Value::Null);
}

#[tokio::test]
async fn zero_is_valid_and_bad_values_are_rejected() {
    let a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/item", "price_below": 0}),
    )
    .await;
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.json()["price_below"],
        json!(0.0)
    );
    for bad in [
        json!(-1),
        json!("cheap"),
        json!(1e99),
        json!(true),
        json!([1]),
    ] {
        let r = call(
            &a,
            Method::POST,
            "/api/v1/watch",
            &json!({"url": "https://example.com/x", "price_below": bad}),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    let path = format!("/api/v1/watch/{id}");
    let r = call(&a, Method::PUT, &path, &json!({"price_below": -5})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get_ok(&a, &path).await.json()["price_below"],
        json!(0.0),
        "unchanged"
    );
}
