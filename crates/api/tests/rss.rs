//! 変更を配信する RSS フィード (`/rss?token=...`)。RSS リーダーはヘッダを送れないので URL のトークンで認証する。

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use sxd_document::parser;
use time::OffsetDateTime;
use uuid::Uuid;
use watchtower_core::WatchState;

fn ts(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap()
}

/// ウォッチを作り、2 つのスナップショットと「変化あり」の状態を入れる。
async fn changed_watch(a: &Api, title: &str, url: &str, old: &str, new: &str, at: i64) -> Uuid {
    let id = Uuid::parse_str(&create_watch(a, json!({"url": url, "title": title})).await).unwrap();
    a.store
        .save_snapshot(id, ts(at - 60), old, [1; 32])
        .await
        .unwrap();
    a.store
        .save_snapshot(id, ts(at), new, [2; 32])
        .await
        .unwrap();
    let state = WatchState {
        last_changed_at: Some(ts(at)),
        ..Default::default()
    };
    a.store.update_state(id, &state).await.unwrap();
    id
}

async fn feed(a: &Api, token: &str) -> Resp {
    send(a, get(&format!("/rss?token={token}"), None)).await
}

fn items(xml: &str) -> Vec<String> {
    // 整形式であることを確かめながら、<item> ごとの title を返す
    let package =
        parser::parse(xml).unwrap_or_else(|e| panic!("not well-formed XML ({e:?}):\n{xml}"));
    let doc = package.as_document();
    let value = sxd_xpath::evaluate_xpath(&doc, "/rss/channel/item/title").unwrap();
    match value {
        sxd_xpath::Value::Nodeset(set) => set
            .document_order()
            .iter()
            .map(|n| n.string_value())
            .collect(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_feed_is_closed_until_a_token_exists_and_rejects_bad_tokens() {
    let a = api().await;
    assert_eq!(
        send(&a, get("/rss", None)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(feed(&a, "x").await.status, StatusCode::SERVICE_UNAVAILABLE);

    let token = a.state.rotate_rss_token().await.unwrap();
    assert_eq!(
        send(&a, get("/rss", None)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(feed(&a, "wrong").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(feed(&a, "").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(feed(&a, &token).await.status, StatusCode::OK);
}

#[tokio::test]
async fn the_api_key_is_not_a_feed_token() {
    let a = api().await;
    a.state.rotate_rss_token().await.unwrap();
    assert_eq!(feed(&a, &a.key).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rotating_invalidates_the_old_token_and_only_a_hash_is_stored() {
    let a = api().await;
    let old = a.state.rotate_rss_token().await.unwrap();
    let new = a.state.rotate_rss_token().await.unwrap();
    assert_ne!(old, new);
    assert_eq!(feed(&a, &old).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(feed(&a, &new).await.status, StatusCode::OK);
    let stored = a
        .store
        .get_setting("rss_token_hash")
        .await
        .unwrap()
        .unwrap()
        .to_string();
    assert!(!stored.contains(&new));
}

#[tokio::test]
async fn the_feed_is_valid_rss_with_the_right_headers() {
    let a = api().await;
    let token = a.state.rotate_rss_token().await.unwrap();
    let r = feed(&a, &token).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("application/rss+xml"));
    assert_eq!(r.headers["cache-control"], "no-store");
    assert!(items(&r.body).is_empty(), "no changes yet");
    assert!(r.body.contains("<rss version=\"2.0\""));
}

#[tokio::test]
async fn changed_watches_become_items_newest_first_with_a_diff() {
    let a = api().await;
    changed_watch(
        &a,
        "Older",
        "https://example.com/a",
        "price: 10",
        "price: 9",
        100,
    )
    .await;
    changed_watch(
        &a,
        "Newer",
        "https://example.com/b",
        "stock: yes",
        "stock: no",
        500,
    )
    .await;
    // 変化したことのないウォッチは載らない
    create_watch(&a, json!({"url": "https://example.com/never"})).await;

    let token = a.state.rotate_rss_token().await.unwrap();
    let r = feed(&a, &token).await;
    assert_eq!(items(&r.body), ["Newer", "Older"]);
    assert!(
        r.body.contains("- stock: yes") && r.body.contains("+ stock: no"),
        "{}",
        r.body
    );
    assert!(r.body.contains("https://example.com/b"));
    assert!(r.body.contains("<pubDate>"), "{}", r.body);
    assert!(
        r.body.contains("<guid isPermaLink=\"false\">"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn untitled_watches_use_their_url_as_the_title() {
    let a = api().await;
    let id = Uuid::parse_str(&create_watch(&a, json!({"url": "https://example.com/plain"})).await)
        .unwrap();
    a.store
        .save_snapshot(id, ts(0), "a", [1; 32])
        .await
        .unwrap();
    a.store
        .save_snapshot(id, ts(1), "b", [2; 32])
        .await
        .unwrap();
    let state = WatchState {
        last_changed_at: Some(ts(1)),
        ..Default::default()
    };
    a.store.update_state(id, &state).await.unwrap();
    let token = a.state.rotate_rss_token().await.unwrap();
    assert_eq!(
        items(&feed(&a, &token).await.body),
        ["https://example.com/plain"]
    );
}

#[tokio::test]
async fn page_content_cannot_break_out_of_the_xml() {
    let a = api().await;
    let evil = "<script>alert(1)</script> & \"q\" ]]> \u{1}";
    changed_watch(
        &a,
        evil,
        "https://example.com/x?a=1&b=2",
        evil,
        &format!("{evil} new"),
        10,
    )
    .await;
    let token = a.state.rotate_rss_token().await.unwrap();
    let r = feed(&a, &token).await;
    let titles = items(&r.body); // 整形式でなければここで失敗する
    assert_eq!(titles.len(), 1);
    assert!(!r.body.contains("<script>"), "{}", r.body);
    assert!(!r.body.contains('\u{1}'));
}

#[tokio::test]
async fn long_diffs_are_truncated_and_the_item_count_is_capped() {
    let a = api().await;
    let big = "x".repeat(50_000);
    changed_watch(&a, "Big", "https://example.com/big", "old", &big, 10).await;
    let token = a.state.rotate_rss_token().await.unwrap();
    let r = feed(&a, &token).await;
    assert!(r.body.len() < 10_000, "{}", r.body.len());

    for i in 0..60 {
        changed_watch(
            &a,
            &format!("W{i}"),
            &format!("https://example.com/{i}"),
            "a",
            "b",
            1000 + i,
        )
        .await;
    }
    assert_eq!(items(&feed(&a, &token).await.body).len(), 50);
}

#[tokio::test]
async fn only_get_is_allowed() {
    let a = api().await;
    let token = a.state.rotate_rss_token().await.unwrap();
    let r = send(
        &a,
        json_req(
            Method::POST,
            &format!("/rss?token={token}"),
            None,
            &json!({}),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn watches_with_unreadable_snapshots_still_appear() {
    let a = api().await;
    let id = changed_watch(&a, "Gone", "https://example.com/g", "a", "b", 10).await;
    for s in a.store.list_snapshots(id).await.unwrap() {
        std::fs::remove_file(&s.path).unwrap();
    }
    let token = a.state.rotate_rss_token().await.unwrap();
    assert_eq!(items(&feed(&a, &token).await.body), ["Gone"]);
}
