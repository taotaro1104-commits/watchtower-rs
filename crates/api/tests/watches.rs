//! 認証と、ウォッチの CRUD / 即時チェック / 一時停止。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use watchtower_scheduler::Command;

// ---- 認証 ---------------------------------------------------------------

#[tokio::test]
async fn requests_without_a_valid_key_are_refused() {
    let a = api().await;
    assert_eq!(
        send(&a, get("/api/v1/watch", None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&a, get("/api/v1/watch", Some("wrong"))).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(get_ok(&a, "/api/v1/watch").await.status, StatusCode::OK);
}

#[tokio::test]
async fn without_a_configured_key_the_api_is_closed() {
    let a = api_without_key().await;
    for key in [None, Some(""), Some("anything")] {
        let r = send(&a, get("/api/v1/watch", key)).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{key:?}");
    }
}

#[tokio::test]
async fn rotating_the_key_invalidates_the_old_one() {
    let a = api().await;
    let old = a.key.clone();
    let new = a.state.rotate_key().await.unwrap();
    assert_ne!(old, new);
    assert_eq!(
        send(&a, get("/api/v1/watch", Some(&old))).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&a, get("/api/v1/watch", Some(&new))).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn keys_are_stored_only_as_hashes() {
    let a = api().await;
    let stored = a
        .store
        .get_setting("api_key_hash")
        .await
        .unwrap()
        .unwrap()
        .to_string();
    assert!(
        !stored.contains(&a.key),
        "the plaintext key must not be stored"
    );
    assert!(a.key.len() >= 32);
}

#[tokio::test]
async fn responses_are_not_cached_or_sniffed() {
    let a = api().await;
    let r = get_ok(&a, "/api/v1/watch").await;
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(r.headers["x-content-type-options"], "nosniff");
    let denied = send(&a, get("/api/v1/watch", None)).await;
    assert_eq!(denied.headers["cache-control"], "no-store");
}

#[tokio::test]
async fn error_bodies_are_json_messages() {
    let a = api().await;
    let r = send(&a, get("/api/v1/watch", None)).await;
    assert!(r.json()["message"].is_string());
}

// ---- 作成・取得 -----------------------------------------------------------

#[tokio::test]
async fn a_watch_can_be_created_and_read_back() {
    let mut a = api().await;
    let id = create_watch(
        &a,
        json!({
            "url": "https://example.com/p",
            "title": "Shop",
            "time_between_check": {"hours": 1, "minutes": 30},
            "include_filters": [".price"],
            "subtractive_selectors": [".ad"],
            "ignore_text": ["cookie"],
            "trigger_text": ["sold out"],
            "notification_urls": ["json://h.example.com/x"],
            "paused": true
        }),
    )
    .await;
    assert!(matches!(a.commands.try_recv(), Ok(Command::Upsert(_))));

    let r = get_ok(&a, &format!("/api/v1/watch/{id}")).await;
    assert_eq!(r.status, StatusCode::OK);
    let w = r.json();
    assert_eq!(w["uuid"], id);
    assert_eq!(w["url"], "https://example.com/p");
    assert_eq!(w["title"], "Shop");
    assert_eq!(w["paused"], true);
    assert_eq!(w["time_between_check"]["hours"], 1);
    assert_eq!(w["time_between_check"]["minutes"], 30);
    assert_eq!(w["include_filters"], json!([".price"]));
    assert_eq!(w["subtractive_selectors"], json!([".ad"]));
    assert_eq!(w["trigger_text"], json!(["sold out"]));
    assert_eq!(w["notification_urls"], json!(["json://h.example.com/x"]));
    assert_eq!(w["last_checked"], 0);
    assert_eq!(w["last_error"], false);
}

#[tokio::test]
async fn invalid_watches_are_rejected_with_400() {
    let a = api().await;
    let too_many: Vec<String> = (0..60).map(|i| format!("x{i}")).collect();
    for body in [
        json!({}),
        json!({"url": ""}),
        json!({"url": "ftp://example.com/"}),
        json!({"url": "not a url"}),
        json!({"url": format!("https://e.com/{}", "a".repeat(3000))}),
        json!({"url": "https://e.com/", "title": "t".repeat(300)}),
        json!({"url": "https://e.com/", "time_between_check": {"seconds": 0}}),
        json!({"url": "https://e.com/", "time_between_check": {"days": -1}}),
        json!({"url": "https://e.com/", "ignore_text": too_many}),
        json!({"url": "https://e.com/", "notification_urls": ["bogus://SECRETtoken/x"]}),
        json!({"url": "https://e.com/", "tags": ["00000000-0000-0000-0000-000000000000"]}),
        json!({"url": "https://e.com/", "tags": ["not-a-uuid"]}),
        json!({"url": 42}),
    ] {
        let r = call(&a, Method::POST, "/api/v1/watch", &body).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.body);
        assert!(!r.body.contains("SECRET"), "{}", r.body);
        assert!(r.json()["message"].is_string());
    }
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_bodies_and_wrong_content_types_are_client_errors() {
    let a = api().await;
    let bad_json = send(&a, text_post("/api/v1/watch", Some(&a.key), "{not json")).await;
    assert_eq!(bad_json.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/watch")
        .header("x-api-key", &a.key)
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    assert!(send(&a, req).await.status.is_client_error());
}

#[tokio::test]
async fn oversized_json_bodies_are_rejected() {
    let a = api().await;
    let body = json!({"url": "https://e.com/", "title": "x".repeat(2 * 1024 * 1024)});
    let r = call(&a, Method::POST, "/api/v1/watch", &body).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn unknown_or_malformed_ids_are_404() {
    let a = api().await;
    for id in ["00000000-0000-0000-0000-000000000000", "not-a-uuid"] {
        assert_eq!(
            get_ok(&a, &format!("/api/v1/watch/{id}")).await.status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(&a, delete(&format!("/api/v1/watch/{id}"), Some(&a.key)))
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        let r = call(
            &a,
            Method::PUT,
            &format!("/api/v1/watch/{id}"),
            &json!({"title": "x"}),
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
}

// ---- 更新・削除 -----------------------------------------------------------

#[tokio::test]
async fn put_updates_only_the_fields_that_are_present() {
    let mut a = api().await;
    let id = create_watch(
        &a,
        json!({"url": "https://example.com/a", "title": "Old", "ignore_text": ["keep"], "paused": true}),
    )
    .await;
    let _ = a.commands.try_recv();

    let r = call(
        &a,
        Method::PUT,
        &format!("/api/v1/watch/{id}"),
        &json!({"title": "New"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(matches!(a.commands.try_recv(), Ok(Command::Upsert(_))));

    let w = get_ok(&a, &format!("/api/v1/watch/{id}")).await.json();
    assert_eq!(w["title"], "New");
    assert_eq!(w["url"], "https://example.com/a");
    assert_eq!(w["ignore_text"], json!(["keep"]));
    assert_eq!(w["paused"], true);

    // null / 空配列は「消す」
    let r = call(
        &a,
        Method::PUT,
        &format!("/api/v1/watch/{id}"),
        &json!({"ignore_text": [], "paused": false}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let w = get_ok(&a, &format!("/api/v1/watch/{id}")).await.json();
    assert_eq!(w["ignore_text"], json!([]));
    assert_eq!(w["paused"], false);
}

#[tokio::test]
async fn invalid_updates_change_nothing() {
    let a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/a", "title": "Keep"})).await;
    let before = get_ok(&a, &format!("/api/v1/watch/{id}")).await.body;
    for body in [
        json!({"url": "ftp://x"}),
        json!({"title": "ok", "time_between_check": {"seconds": 0}}),
        json!({"title": "ok", "notification_urls": ["bogus://x"]}),
    ] {
        let r = call(&a, Method::PUT, &format!("/api/v1/watch/{id}"), &body).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
    }
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.body,
        before
    );
}

#[tokio::test]
async fn delete_removes_the_watch_and_tells_the_scheduler() {
    let mut a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/a"})).await;
    let _ = a.commands.try_recv();
    let r = send(&a, delete(&format!("/api/v1/watch/{id}"), Some(&a.key))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(matches!(a.commands.try_recv(), Ok(Command::Remove(_))));
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

// ---- recheck / paused ------------------------------------------------------

#[tokio::test]
async fn recheck_requests_an_immediate_check() {
    let mut a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/a"})).await;
    let _ = a.commands.try_recv();
    let r = get_ok(&a, &format!("/api/v1/watch/{id}?recheck=1")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(matches!(a.commands.try_recv(), Ok(Command::CheckNow(i)) if i.to_string() == id));
}

#[tokio::test]
async fn paused_query_toggles_the_flag() {
    let mut a = api().await;
    let id = create_watch(&a, json!({"url": "https://example.com/a"})).await;
    let _ = a.commands.try_recv();
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}?paused=paused"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.json()["paused"],
        true
    );
    assert!(
        a.commands.try_recv().is_ok(),
        "scheduler is told to reload the watch"
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}?paused=unpaused"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}")).await.json()["paused"],
        false
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}?paused=maybe"))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

// ---- 一覧 -----------------------------------------------------------------

#[tokio::test]
async fn the_list_can_be_filtered_by_tag_title_or_id() {
    let a = api().await;
    let tag = call(&a, Method::POST, "/api/v1/tags", &json!({"title": "Shops"})).await;
    assert_eq!(tag.status, StatusCode::CREATED, "{}", tag.body);
    let tag_id = tag.json()["uuid"].as_str().unwrap().to_string();
    let tagged = create_watch(
        &a,
        json!({"url": "https://example.com/1", "tags": [tag_id]}),
    )
    .await;
    let _plain = create_watch(&a, json!({"url": "https://example.com/2"})).await;

    let all = get_ok(&a, "/api/v1/watch").await.json();
    assert_eq!(all.as_object().unwrap().len(), 2);
    for q in ["Shops", &tag_id] {
        let r = get_ok(&a, &format!("/api/v1/watch?tag={q}")).await.json();
        let keys: Vec<_> = r.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec![tagged.clone()], "tag={q}");
    }
    let none = get_ok(&a, "/api/v1/watch?tag=Missing").await.json();
    assert!(none.as_object().unwrap().is_empty());
}
