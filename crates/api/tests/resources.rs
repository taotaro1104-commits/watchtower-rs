//! 履歴・タグ・インポート・systeminfo。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

fn ts(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap()
}

async fn seeded(a: &Api) -> String {
    let id = create_watch(a, json!({"url": "https://example.com/a"})).await;
    let uuid = Uuid::parse_str(&id).unwrap();
    a.store
        .save_snapshot(uuid, ts(0), "price: 10", [1; 32])
        .await
        .unwrap();
    a.store
        .save_snapshot(uuid, ts(60), "price: 20 <b>", [2; 32])
        .await
        .unwrap();
    id
}

// ---- 履歴 ---------------------------------------------------------------

#[tokio::test]
async fn history_lists_snapshot_times_with_links() {
    let a = api().await;
    let id = seeded(&a).await;
    let r = get_ok(&a, &format!("/api/v1/watch/{id}/history")).await;
    assert_eq!(r.status, StatusCode::OK);
    let h = r.json();
    let h = h.as_object().unwrap();
    assert_eq!(h.len(), 2);
    let first = (1_700_000_000i64).to_string();
    assert_eq!(h[&first], format!("/api/v1/watch/{id}/history/{first}"));
    assert!(!r.body.contains("snaps"), "no filesystem paths: {}", r.body);
}

#[tokio::test]
async fn snapshot_bodies_are_served_as_plain_text() {
    let a = api().await;
    let id = seeded(&a).await;
    let r = get_ok(&a, &format!("/api/v1/watch/{id}/history/1700000000")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, "price: 10");
    assert!(r.headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/plain"));

    let latest = get_ok(&a, &format!("/api/v1/watch/{id}/history/latest")).await;
    assert_eq!(latest.body, "price: 20 <b>");
    // 取得内容を HTML として解釈させない
    assert_eq!(latest.headers["x-content-type-options"], "nosniff");
    assert!(latest.headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'none'"));
}

#[tokio::test]
async fn missing_snapshots_and_bad_timestamps_are_client_errors() {
    let a = api().await;
    let id = seeded(&a).await;
    let empty = create_watch(&a, json!({"url": "https://example.com/empty"})).await;
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}/history/1"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{id}/history/abc"))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_ok(&a, &format!("/api/v1/watch/{empty}/history/latest"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_ok(
            &a,
            "/api/v1/watch/00000000-0000-0000-0000-000000000000/history"
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn history_requires_the_key() {
    let a = api().await;
    let id = seeded(&a).await;
    let r = send(&a, get(&format!("/api/v1/watch/{id}/history/latest"), None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

// ---- タグ ---------------------------------------------------------------

#[tokio::test]
async fn tags_can_be_listed_and_created() {
    let a = api().await;
    assert!(get_ok(&a, "/api/v1/tags")
        .await
        .json()
        .as_object()
        .unwrap()
        .is_empty());
    let r = call(&a, Method::POST, "/api/v1/tags", &json!({"title": "Shops"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = r.json()["uuid"].as_str().unwrap().to_string();
    let list = get_ok(&a, "/api/v1/tags").await.json();
    assert_eq!(list[&id]["title"], "Shops");
    assert_eq!(list[&id]["uuid"], id);
}

#[tokio::test]
async fn invalid_tags_are_rejected() {
    let a = api().await;
    for body in [
        json!({}),
        json!({"title": ""}),
        json!({"title": "   "}),
        json!({"title": "x".repeat(101)}),
    ] {
        let r = call(&a, Method::POST, "/api/v1/tags", &body).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
    }
    assert!(a.store.list_tags().await.unwrap().is_empty());
}

// ---- インポート -------------------------------------------------------------

#[tokio::test]
async fn urls_can_be_imported_in_bulk() {
    let mut a = api().await;
    let body = "https://example.com/1\n\n  https://example.com/2  \r\nhttps://example.com/3\n";
    let r = send(&a, text_post("/api/v1/import", Some(&a.key), body)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let ids = r.json();
    assert_eq!(ids.as_array().unwrap().len(), 3);
    assert_eq!(a.store.list_watches().await.unwrap().len(), 3);
    for _ in 0..3 {
        assert!(matches!(
            a.commands.try_recv(),
            Ok(watchtower_scheduler::Command::Upsert(_))
        ));
    }
}

#[tokio::test]
async fn import_can_tag_and_pause_the_new_watches() {
    let a = api().await;
    let r = send(
        &a,
        text_post(
            "/api/v1/import?tag=Shops&paused=true",
            Some(&a.key),
            "https://example.com/1\nhttps://example.com/2",
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let tags = a.store.list_tags().await.unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].title, "Shops");
    let watches = a.store.list_watches().await.unwrap();
    assert!(watches
        .iter()
        .all(|w| w.paused && w.tags == vec![tags[0].id]));

    // 同じタイトルのタグは再利用される
    send(
        &a,
        text_post(
            "/api/v1/import?tag=Shops",
            Some(&a.key),
            "https://example.com/3",
        ),
    )
    .await;
    assert_eq!(a.store.list_tags().await.unwrap().len(), 1);
}

#[tokio::test]
async fn one_bad_line_rejects_the_whole_import() {
    let a = api().await;
    let r = send(
        &a,
        text_post(
            "/api/v1/import",
            Some(&a.key),
            "https://example.com/1\nftp://nope\nhttps://example.com/3",
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.json()["message"].as_str().unwrap().contains("line 2"),
        "{}",
        r.body
    );
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn import_limits_the_number_of_urls_and_the_body_size() {
    let a = api().await;
    let many: String = (0..5001)
        .map(|i| format!("https://example.com/{i}\n"))
        .collect();
    let r = send(&a, text_post("/api/v1/import", Some(&a.key), &many)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("5000"));
    let huge = "x".repeat(8 * 1024 * 1024);
    let r = send(&a, text_post("/api/v1/import", Some(&a.key), &huge)).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(a.store.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn import_with_an_unknown_paused_value_is_rejected() {
    let a = api().await;
    let r = send(
        &a,
        text_post(
            "/api/v1/import?paused=maybe",
            Some(&a.key),
            "https://example.com/1",
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ---- systeminfo ------------------------------------------------------------

#[tokio::test]
async fn systeminfo_reports_counts_and_version() {
    let a = api().await;
    create_watch(&a, json!({"url": "https://example.com/a"})).await;
    create_watch(&a, json!({"url": "https://example.com/b", "paused": true})).await;
    let r = get_ok(&a, "/api/v1/systeminfo").await;
    assert_eq!(r.status, StatusCode::OK);
    let s = r.json();
    assert_eq!(s["watch_count"], 2);
    assert_eq!(s["queue_length"], 1, "paused watches are not queued: {s}");
    assert_eq!(s["pending_notifications"], 0);
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"));
    assert!(s["uptime_seconds"].is_u64());
}
