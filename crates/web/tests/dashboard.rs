//! ダッシュボード: 件数・失敗中のウォッチ・通知キュー・直近の変化を一画面にまとめる。

mod common;

use axum::http::StatusCode;
use common::*;
use time::OffsetDateTime;
use url::Url;
use watchtower_core::Watch;
use watchtower_store::NewNotification;

fn watch(u: &str) -> Watch {
    Watch::new(Url::parse(u).unwrap())
}

#[tokio::test]
async fn dashboard_requires_login() {
    let a = app().await;
    let r = send(&a, get("/dashboard", None)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn empty_dashboard_renders_zero_counts() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = send(&a, get("/dashboard", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Dashboard"), "{}", r.body);
    assert!(r.body.contains("No failing watches"), "{}", r.body);
}

#[tokio::test]
async fn counts_failing_paused_and_total_watches() {
    let a = app().await;
    let ok = watch("https://ok.example/");
    let mut paused = watch("https://paused.example/");
    paused.paused = true;
    let mut bad = watch("https://bad.example/");
    bad.state.last_error = Some("connection refused".into());
    bad.state.consecutive_errors = 3;
    for w in [&ok, &paused, &bad] {
        a.store.add_watch(w).await.unwrap();
        a.store.update_state(w.id, &w.state).await.unwrap();
    }
    let cookie = login(&a).await;
    let r = send(&a, get("/dashboard", Some(&cookie))).await;
    assert!(r.body.contains(r#"data-stat="total">3<"#), "{}", r.body);
    assert!(r.body.contains(r#"data-stat="paused">1<"#), "{}", r.body);
    assert!(r.body.contains(r#"data-stat="failing">1<"#), "{}", r.body);
    assert!(r.body.contains("connection refused"));
    assert!(r.body.contains(&format!("/watches/{}", bad.id)));
}

#[tokio::test]
async fn failing_watch_error_text_is_escaped() {
    let a = app().await;
    let mut bad = watch("https://bad.example/");
    bad.state.last_error = Some("<script>x</script>".into());
    a.store.add_watch(&bad).await.unwrap();
    a.store.update_state(bad.id, &bad.state).await.unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get("/dashboard", Some(&cookie))).await;
    assert!(!r.body.contains("<script>x</script>"), "{}", r.body);
}

#[tokio::test]
async fn shows_pending_notifications_and_recent_changes() {
    let a = app().await;
    let w = watch("https://shop.example/");
    a.store.add_watch(&w).await.unwrap();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .record_event(Some(w.id), "changed", now, Some("+2 -1"))
        .await
        .unwrap();
    a.store
        .enqueue_notification(
            &NewNotification {
                watch_id: Some(w.id),
                target: "json://h".into(),
                title: "t".into(),
                body: "b".into(),
            },
            now,
        )
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get("/dashboard", Some(&cookie))).await;
    assert!(r.body.contains(r#"data-stat="pending">1<"#), "{}", r.body);
    assert!(r.body.contains("+2 -1"), "{}", r.body);
    assert!(r.body.contains("https://shop.example/"));
}

#[tokio::test]
async fn index_links_to_the_dashboard() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = send(&a, get("/", Some(&cookie))).await;
    assert!(r.body.contains(r#"href="/dashboard""#), "{}", r.body);
}
