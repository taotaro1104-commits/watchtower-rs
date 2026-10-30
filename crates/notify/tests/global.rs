//! 設定画面で保存した共通の通知先がディスパッチャに効くこと。

use time::OffsetDateTime;
use url::Url;
use watchtower_core::{ConfigLayer, Watch};
use watchtower_notify::{ChangeEvent, Dispatcher, TemplateSet};
use watchtower_process::DiffResult;
use watchtower_store::Store;

#[tokio::test]
async fn global_notify_urls_saved_in_settings_are_used() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let w = Watch::new(Url::parse("https://example.com/").unwrap());
    store.add_watch(&w).await.unwrap();
    let d = Dispatcher::new(
        store.clone(),
        ConfigLayer::default(),
        TemplateSet::default(),
        "http://wt.local".into(),
    );
    let ev = ChangeEvent {
        watch_id: w.id,
        diff: DiffResult::default(),
        triggered: false,
    };
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    assert_eq!(d.on_change(&ev, now).await.unwrap(), 0);

    store
        .set_global_notify_urls(&["json://h.example.com/x".to_string()])
        .await
        .unwrap();
    assert_eq!(d.on_change(&ev, now).await.unwrap(), 1);
}
