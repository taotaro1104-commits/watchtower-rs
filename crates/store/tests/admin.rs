//! update_watch / タグ / 設定 / イベント。

use serde_json::json;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;
use watchtower_core::{ConfigLayer, Tag, Watch};
use watchtower_store::Store;

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

fn watch(u: &str) -> Watch {
    Watch::new(Url::parse(u).unwrap())
}

#[tokio::test]
async fn update_watch_changes_settings_but_keeps_state() {
    let (s, _d) = open().await;
    let mut w = watch("https://example.com/a");
    w.state.consecutive_errors = 3;
    s.add_watch(&w).await.unwrap();

    w.url = Url::parse("https://example.com/b").unwrap();
    w.title = Some("B".into());
    w.paused = true;
    w.config = ConfigLayer {
        check_interval_secs: Some(90),
        ..Default::default()
    };
    w.state.consecutive_errors = 0; // update_watch は状態に触れない
    assert!(s.update_watch(&w).await.unwrap());

    let got = s.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.url.as_str(), "https://example.com/b");
    assert_eq!((got.title.as_deref(), got.paused), (Some("B"), true));
    assert_eq!(got.config.check_interval_secs, Some(90));
    assert_eq!(got.state.consecutive_errors, 3);
}

#[tokio::test]
async fn update_of_unknown_watch_reports_false() {
    let (s, _d) = open().await;
    assert!(!s
        .update_watch(&watch("https://example.com/"))
        .await
        .unwrap());
}

#[tokio::test]
async fn tags_roundtrip_and_list_sorted_by_title() {
    let (s, _d) = open().await;
    let b = Tag::new("Beta");
    let mut a = Tag::new("Alpha");
    a.config.notify_urls = vec!["json://h/x".into()];
    s.add_tag(&b).await.unwrap();
    s.add_tag(&a).await.unwrap();
    let titles: Vec<_> = s
        .list_tags()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, ["Alpha", "Beta"]);
    assert_eq!(s.get_tag(a.id).await.unwrap().unwrap(), a);

    a.title = "Gamma".into();
    assert!(s.update_tag(&a).await.unwrap());
    assert_eq!(s.get_tag(a.id).await.unwrap().unwrap().title, "Gamma");
    assert!(s.get_tag(Uuid::new_v4()).await.unwrap().is_none());
}

#[tokio::test]
async fn deleting_a_tag_detaches_it_from_watches() {
    let (s, _d) = open().await;
    let (keep, gone) = (Tag::new("keep"), Tag::new("gone"));
    s.add_tag(&keep).await.unwrap();
    s.add_tag(&gone).await.unwrap();
    let mut w = watch("https://example.com/");
    w.tags = vec![keep.id, gone.id];
    s.add_watch(&w).await.unwrap();

    assert!(s.delete_tag(gone.id).await.unwrap());
    assert_eq!(
        s.get_watch(w.id).await.unwrap().unwrap().tags,
        vec![keep.id]
    );
    assert!(!s.delete_tag(gone.id).await.unwrap());
}

#[tokio::test]
async fn settings_store_json_values_and_overwrite() {
    let (s, _d) = open().await;
    assert!(s.get_setting("smtp").await.unwrap().is_none());
    s.set_setting("smtp", &json!({"host": "mail"}))
        .await
        .unwrap();
    s.set_setting("smtp", &json!({"host": "mail2"}))
        .await
        .unwrap();
    assert_eq!(
        s.get_setting("smtp").await.unwrap().unwrap()["host"],
        "mail2"
    );
}

#[tokio::test]
async fn events_are_listed_newest_first_and_can_be_filtered() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    s.record_event(Some(w.id), "changed", ts(1), Some("+1"))
        .await
        .unwrap();
    s.record_event(None, "started", ts(2), None).await.unwrap();
    s.record_event(Some(w.id), "error", ts(3), Some("HTTP 500"))
        .await
        .unwrap();

    let all = s.list_events(None, 10).await.unwrap();
    assert_eq!(
        all.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["error", "started", "changed"]
    );
    assert_eq!(all[0].detail.as_deref(), Some("HTTP 500"));
    let mine = s.list_events(Some(w.id), 10).await.unwrap();
    assert_eq!(mine.len(), 2);
    assert_eq!(s.list_events(None, 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn old_events_are_pruned_keeping_the_newest() {
    let (s, _d) = open().await;
    for i in 0..5 {
        s.record_event(None, "e", ts(i), None).await.unwrap();
    }
    assert_eq!(s.prune_events(2).await.unwrap(), 3);
    let left = s.list_events(None, 10).await.unwrap();
    assert_eq!(left.len(), 2);
    assert_eq!(left[0].at, ts(4));
}
