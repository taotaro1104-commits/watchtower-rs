//! 価格の閾値を設定したウォッチは、閾値以下になったときだけ通知する。

use time::OffsetDateTime;
use url::Url;
use watchtower_core::{ConfigLayer, Tag, Watch};
use watchtower_notify::{ChangeEvent, Dispatcher, TemplateSet};
use watchtower_process::DiffResult;
use watchtower_store::Store;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn event(w: &Watch, triggered: bool) -> ChangeEvent {
    ChangeEvent {
        watch_id: w.id,
        diff: DiffResult {
            added: vec!["price: 9".into()],
            removed: vec!["price: 30".into()],
        },
        triggered,
    }
}

async fn setup(watch_cfg: ConfigLayer) -> (Store, Watch, Dispatcher, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/item").unwrap());
    w.config = ConfigLayer {
        notify_urls: vec!["json://h.example.com/x".into()],
        ..watch_cfg
    };
    store.add_watch(&w).await.unwrap();
    let d = Dispatcher::new(
        store.clone(),
        ConfigLayer::default(),
        TemplateSet::default(),
        "http://wt.local".into(),
    );
    (store, w, d, dir)
}

#[tokio::test]
async fn a_price_threshold_gates_notifications() {
    let (_s, w, d, _dir) = setup(ConfigLayer {
        price_below: Some(20.0),
        ..Default::default()
    })
    .await;
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 0);
    assert_eq!(d.on_change(&event(&w, true), now()).await.unwrap(), 1);
}

#[tokio::test]
async fn without_a_threshold_or_trigger_text_every_change_notifies() {
    let (_s, w, d, _dir) = setup(ConfigLayer::default()).await;
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 1);
}

#[tokio::test]
async fn a_threshold_set_on_a_tag_applies_too() {
    let (store, mut w, d, _dir) = setup(ConfigLayer::default()).await;
    let mut tag = Tag::new("deals");
    tag.config.price_below = Some(15.0);
    store.add_tag(&tag).await.unwrap();
    w.tags = vec![tag.id];
    store.update_watch(&w).await.unwrap();
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 0);
    assert_eq!(d.on_change(&event(&w, true), now()).await.unwrap(), 1);
}
