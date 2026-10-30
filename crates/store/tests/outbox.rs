//! 通知 outbox (送信待ちキュー)。

use time::OffsetDateTime;
use url::Url;
use watchtower_core::Watch;
use watchtower_store::{NewNotification, OutboxStatus, Store};

async fn open() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("wt.db"), dir.path().join("snapshots"))
        .await
        .unwrap();
    (store, dir)
}

fn ts(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap()
}

fn new_note(watch: &Watch, target: &str) -> NewNotification {
    NewNotification {
        watch_id: Some(watch.id),
        target: target.into(),
        title: "T".into(),
        body: "B".into(),
    }
}

async fn with_watch(s: &Store) -> Watch {
    let w = Watch::new(Url::parse("https://example.com/").unwrap());
    s.add_watch(&w).await.unwrap();
    w
}

#[tokio::test]
async fn enqueued_notifications_are_due_immediately_in_order() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    let a = s
        .enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
        .await
        .unwrap();
    let b = s
        .enqueue_notification(&new_note(&w, "json://b/x"), ts(0))
        .await
        .unwrap();
    let due = s.due_notifications(ts(0), 10).await.unwrap();
    assert_eq!(due.iter().map(|n| n.id).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(due[0].target, "json://a/x");
    assert_eq!((due[0].title.as_str(), due[0].body.as_str()), ("T", "B"));
    assert_eq!(due[0].attempts, 0);
}

#[tokio::test]
async fn limit_is_respected() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    for _ in 0..3 {
        s.enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
            .await
            .unwrap();
    }
    assert_eq!(s.due_notifications(ts(0), 2).await.unwrap().len(), 2);
}

#[tokio::test]
async fn sent_notifications_leave_the_queue() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    let id = s
        .enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
        .await
        .unwrap();
    s.mark_notification_sent(id).await.unwrap();
    assert!(s.due_notifications(ts(1000), 10).await.unwrap().is_empty());
    assert!(s.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn retry_hides_the_item_until_its_time_and_counts_attempts() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    let id = s
        .enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
        .await
        .unwrap();
    s.retry_notification_at(id, "HTTP 500", ts(60))
        .await
        .unwrap();
    assert!(s.due_notifications(ts(59), 10).await.unwrap().is_empty());
    let due = s.due_notifications(ts(60), 10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].attempts, 1);
    assert_eq!(due[0].last_error.as_deref(), Some("HTTP 500"));
}

#[tokio::test]
async fn failed_notifications_are_kept_for_display_but_never_due() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    let id = s
        .enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
        .await
        .unwrap();
    s.fail_notification(id, "HTTP 500").await.unwrap();
    assert!(s
        .due_notifications(ts(10_000), 10)
        .await
        .unwrap()
        .is_empty());
    let all = s.list_notifications().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, OutboxStatus::Failed);
    assert_eq!(all[0].last_error.as_deref(), Some("HTTP 500"));
}

#[tokio::test]
async fn deleting_a_watch_discards_its_pending_notifications() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    s.enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
        .await
        .unwrap();
    s.delete_watch(w.id).await.unwrap();
    assert!(s.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn notifications_without_a_watch_are_allowed() {
    let (s, _d) = open().await;
    let n = NewNotification {
        watch_id: None,
        target: "json://a/x".into(),
        title: "test".into(),
        body: "test".into(),
    };
    s.enqueue_notification(&n, ts(0)).await.unwrap();
    assert_eq!(s.due_notifications(ts(0), 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn queue_survives_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wt.db");
    let snaps = dir.path().join("snapshots");
    let id = {
        let s = Store::open(&db, snaps.clone()).await.unwrap();
        let w = with_watch(&s).await;
        s.enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
            .await
            .unwrap()
    };
    let s = Store::open(&db, snaps).await.unwrap();
    assert_eq!(s.due_notifications(ts(0), 10).await.unwrap()[0].id, id);
}

#[tokio::test]
async fn failed_rows_are_pruned_to_the_newest_but_pending_rows_are_kept() {
    let (s, _d) = open().await;
    let w = with_watch(&s).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            s.enqueue_notification(&new_note(&w, "json://a/x"), ts(0))
                .await
                .unwrap(),
        );
    }
    for id in &ids[..4] {
        s.fail_notification(*id, "HTTP 500").await.unwrap();
    }
    assert_eq!(s.prune_failed_notifications(2).await.unwrap(), 2);
    let left = s.list_notifications().await.unwrap();
    let left_ids: Vec<_> = left.iter().map(|n| n.id).collect();
    assert_eq!(
        left_ids,
        vec![ids[2], ids[3], ids[4]],
        "newest failed + the pending one"
    );
}
