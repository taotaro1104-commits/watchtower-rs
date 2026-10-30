//! 本家 changedetection.io の url-watches.json のインポート。

use uuid::Uuid;
use watchtower_store::Store;

async fn open() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("wt.db"), dir.path().join("snapshots"))
        .await
        .unwrap();
    (store, dir)
}

const ID1: &str = "11111111-1111-4111-8111-111111111111";
const ID2: &str = "22222222-2222-4222-8222-222222222222";

fn sample() -> String {
    format!(
        r#"{{
  "settings": {{"application": {{"tags": {{"t1": {{"title": "Shops"}}}}}}}},
  "watching": {{
    "{ID1}": {{
      "url": "https://example.com/a",
      "title": "Shop A",
      "paused": true,
      "tags": ["t1"],
      "time_between_check": {{"weeks": null, "days": 1, "hours": 2, "minutes": 3, "seconds": 4}},
      "include_filters": [".price"],
      "subtractive_selectors": [".ad"],
      "ignore_text": ["cookie"],
      "trigger_text": ["sold out"],
      "notification_urls": ["json://hook.example.com/x"],
      "headers": {{"X-Test": "1"}},
      "method": "GET"
    }},
    "{ID2}": {{"url": "https://example.com/b"}}
  }}
}}"#
    )
}

#[tokio::test]
async fn watches_are_imported_with_mapped_settings() {
    let (s, _d) = open().await;
    let r = s.import_changedetection(&sample()).await.unwrap();
    assert_eq!((r.imported, r.skipped.len()), (2, 0));

    let w = s
        .get_watch(Uuid::parse_str(ID1).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.url.as_str(), "https://example.com/a");
    assert_eq!(w.title.as_deref(), Some("Shop A"));
    assert!(w.paused);
    assert_eq!(w.config.check_interval_secs, Some(86_400 + 7_200 + 180 + 4));
    assert_eq!(w.config.include_filters, vec![".price"]);
    assert_eq!(w.config.exclude_filters, vec![".ad"]);
    assert_eq!(w.config.ignore_text, vec!["cookie"]);
    assert_eq!(w.config.trigger_text, vec!["sold out"]);
    assert_eq!(w.config.notify_urls, vec!["json://hook.example.com/x"]);

    let w2 = s
        .get_watch(Uuid::parse_str(ID2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        w2.config.check_interval_secs, None,
        "unset interval uses the default"
    );
    assert!(!w2.paused);
}

#[tokio::test]
async fn unsupported_features_are_reported_not_silently_dropped() {
    let (s, _d) = open().await;
    let r = s.import_changedetection(&sample()).await.unwrap();
    let all = r.warnings.join("\n");
    assert!(all.contains("headers"), "{all}");
    assert!(all.contains("tags"), "{all}");
}

#[tokio::test]
async fn bad_entries_are_skipped_and_the_rest_imported() {
    let (s, _d) = open().await;
    let json = format!(
        r#"{{"watching": {{
          "{ID1}": {{"url": "ftp://example.com/x"}},
          "not-a-uuid": {{"url": "https://example.com/new-id"}},
          "{ID2}": {{"title": "no url"}}
        }}}}"#
    );
    let r = s.import_changedetection(&json).await.unwrap();
    assert_eq!(r.imported, 1, "{r:?}");
    assert_eq!(r.skipped.len(), 2);
    assert_eq!(s.list_watches().await.unwrap().len(), 1);
}

#[tokio::test]
async fn importing_twice_skips_existing_ids() {
    let (s, _d) = open().await;
    s.import_changedetection(&sample()).await.unwrap();
    let r = s.import_changedetection(&sample()).await.unwrap();
    assert_eq!((r.imported, r.skipped.len()), (0, 2));
    assert_eq!(s.list_watches().await.unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_json_is_an_error_and_changes_nothing() {
    let (s, _d) = open().await;
    assert!(s.import_changedetection("{not json").await.is_err());
    assert!(s
        .import_changedetection(r#"{"settings": {}}"#)
        .await
        .is_err());
    assert!(s.list_watches().await.unwrap().is_empty());
}

#[tokio::test]
async fn absurd_intervals_are_clamped_not_rejected() {
    let (s, _d) = open().await;
    let json = format!(
        r#"{{"watching": {{"{ID1}": {{"url": "https://e.com/", "time_between_check": {{"weeks": 99999999999}}}}}}}}"#
    );
    let r = s.import_changedetection(&json).await.unwrap();
    assert_eq!(r.imported, 1, "{r:?}");
}
