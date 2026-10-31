//! ウォッチ / タグごとの通知テンプレート (件名・本文)。

use time::OffsetDateTime;
use url::Url;
use watchtower_core::{ConfigLayer, Tag, Watch};
use watchtower_notify::{ChangeEvent, Dispatcher, NotifyError, TemplateSet};
use watchtower_process::DiffResult;
use watchtower_store::Store;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

struct Env {
    store: Store,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("wt.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    Env { store, _dir: dir }
}

impl Env {
    async fn queued(&self, w: &Watch, global: ConfigLayer) -> (String, String) {
        let d = Dispatcher::new(
            self.store.clone(),
            global,
            TemplateSet::default(),
            "http://wt.local".into(),
        );
        let ev = ChangeEvent {
            watch_id: w.id,
            diff: DiffResult {
                added: vec!["new".into()],
                removed: vec!["old".into()],
            },
            triggered: false,
        };
        assert_eq!(d.on_change(&ev, now()).await.unwrap(), 1);
        let n = self.store.list_notifications().await.unwrap();
        (n[0].title.clone(), n[0].body.clone())
    }

    async fn watch(&self, cfg: ConfigLayer) -> Watch {
        let mut w = Watch::new(Url::parse("https://example.com/p").unwrap());
        w.title = Some("Example".into());
        w.config = ConfigLayer {
            notify_urls: vec!["json://h.example.com/x".into()],
            ..cfg
        };
        self.store.add_watch(&w).await.unwrap();
        w
    }
}

fn custom(title: Option<&str>, body: Option<&str>) -> ConfigLayer {
    ConfigLayer {
        notification_title: title.map(Into::into),
        notification_body: body.map(Into::into),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_watch_template_replaces_the_default() {
    let e = env().await;
    let w = e
        .watch(custom(
            Some("[{{ watch_title }}]"),
            Some("now: {{ diff_added }}"),
        ))
        .await;
    let (title, body) = e.queued(&w, ConfigLayer::default()).await;
    assert_eq!(title, "[Example]");
    assert_eq!(body, "now: new");
}

#[tokio::test]
async fn setting_only_one_part_keeps_the_default_for_the_other() {
    let e = env().await;
    let w = e.watch(custom(Some("Custom"), None)).await;
    let (title, body) = e.queued(&w, ConfigLayer::default()).await;
    assert_eq!(title, "Custom");
    assert!(
        body.contains("https://example.com/p") && body.contains("+ new"),
        "{body}"
    );
}

#[tokio::test]
async fn a_tag_template_applies_unless_the_watch_has_its_own() {
    let e = env().await;
    let mut tag = Tag::new("shop");
    tag.config = custom(Some("tag: {{ watch_title }}"), Some("tag body"));
    e.store.add_tag(&tag).await.unwrap();

    let mut w = e.watch(custom(None, Some("watch body"))).await;
    w.tags = vec![tag.id];
    e.store.update_watch(&w).await.unwrap();

    let (title, body) = e.queued(&w, ConfigLayer::default()).await;
    assert_eq!(title, "tag: Example");
    assert_eq!(body, "watch body");
}

#[tokio::test]
async fn a_broken_template_falls_back_to_the_default_and_still_notifies() {
    let e = env().await;
    let w = e
        .watch(custom(Some("{{ no_such_variable }}"), Some("{% for %}")))
        .await;
    let (title, body) = e.queued(&w, ConfigLayer::default()).await;
    assert!(title.contains("Example"), "{title}");
    assert!(body.contains("+ new"), "{body}");
}

#[test]
fn overrides_are_validated_like_any_template() {
    assert!(TemplateSet::with_overrides(Some("{{ watch_title }}"), None).is_ok());
    assert!(matches!(
        TemplateSet::with_overrides(None, Some("{{ nope }}")),
        Err(NotifyError::Template(_))
    ));
}
