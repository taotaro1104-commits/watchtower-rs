//! 変化の検知 → outbox → 再送つき送信、までの結合テスト (実 Store + wiremock)。

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::{ConfigLayer, Watch};
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_notify::{
    ChangeEvent, Dispatcher, HttpNotifier, OutboxWorker, RetryPolicy, TemplateSet,
};
use watchtower_process::DiffResult;
use watchtower_store::{OutboxStatus, Store};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Env {
    store: Store,
    server: MockServer,
    _dir: tempfile::TempDir,
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("wt.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    Env {
        store,
        server: MockServer::start().await,
        _dir: dir,
    }
}

impl Env {
    fn hook(&self) -> String {
        format!("json://{}/hook", self.server.address())
    }

    async fn watch(&self, cfg: ConfigLayer) -> Watch {
        let mut w = Watch::new(Url::parse("https://example.com/p").unwrap());
        w.title = Some("Example".into());
        w.config = cfg;
        self.store.add_watch(&w).await.unwrap();
        self.store
            .save_snapshot(w.id, now(), "price: 20", [7; 32])
            .await
            .unwrap();
        w
    }

    fn dispatcher(&self, global: ConfigLayer) -> Dispatcher {
        Dispatcher::new(
            self.store.clone(),
            global,
            TemplateSet::default(),
            "http://wt.local".into(),
        )
    }

    fn worker(&self) -> OutboxWorker {
        let fetcher = Arc::new(HttpFetcher::new(SsrfPolicy::allow_loopback()));
        OutboxWorker::new(
            self.store.clone(),
            Arc::new(HttpNotifier::new(fetcher)),
            RetryPolicy {
                max_retries: 3,
                base: Duration::from_secs(30),
            },
        )
    }
}

fn event(w: &Watch, triggered: bool) -> ChangeEvent {
    ChangeEvent {
        watch_id: w.id,
        diff: DiffResult {
            added: vec!["price: 20".into()],
            removed: vec!["price: 10".into()],
        },
        triggered,
    }
}

fn cfg_with(urls: Vec<String>) -> ConfigLayer {
    ConfigLayer {
        notify_urls: urls,
        ..Default::default()
    }
}

async fn mock_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

fn request_count(rs: Option<Vec<wiremock::Request>>) -> usize {
    rs.unwrap().len()
}

#[tokio::test]
async fn change_is_delivered_with_rendered_title_url_and_diff() {
    let e = env().await;
    mock_ok(&e.server).await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;

    let queued = e
        .dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    assert_eq!(queued, 1);
    assert_eq!(
        e.worker()
            .run_once(now(), CancellationToken::new())
            .await
            .unwrap(),
        1
    );

    let reqs = e.server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(body["title"].as_str().unwrap().contains("Example"));
    let text = body["message"].as_str().unwrap();
    assert!(text.contains("https://example.com/p"), "{text}");
    assert!(
        text.contains("- price: 10") && text.contains("+ price: 20"),
        "{text}"
    );
    assert!(e.store.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn global_notify_urls_apply_and_watch_urls_are_added_without_duplicates() {
    let e = env().await;
    mock_ok(&e.server).await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    let queued = e
        .dispatcher(cfg_with(vec![e.hook(), "json://127.0.0.1:1/other".into()]))
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    assert_eq!(queued, 2);
}

#[tokio::test]
async fn muted_watches_and_watches_without_targets_queue_nothing() {
    let e = env().await;
    let mut muted = cfg_with(vec![e.hook()]);
    muted.notify_muted = Some(true);
    let w = e.watch(muted).await;
    let d = e.dispatcher(ConfigLayer::default());
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 0);

    let w = e.watch(ConfigLayer::default()).await;
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 0);
}

#[tokio::test]
async fn trigger_text_gates_notifications() {
    let e = env().await;
    let mut cfg = cfg_with(vec![e.hook()]);
    cfg.trigger_text = vec!["sold out".into()];
    let w = e.watch(cfg).await;
    let d = e.dispatcher(ConfigLayer::default());
    assert_eq!(d.on_change(&event(&w, false), now()).await.unwrap(), 0);
    assert_eq!(d.on_change(&event(&w, true), now()).await.unwrap(), 1);
}

#[tokio::test]
async fn unknown_watch_is_ignored() {
    let e = env().await;
    let ghost = Watch::new(Url::parse("https://example.com/").unwrap());
    let n = e
        .dispatcher(ConfigLayer::default())
        .on_change(&event(&ghost, true), now())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn one_invalid_target_does_not_block_the_others() {
    let e = env().await;
    let w = e.watch(cfg_with(vec!["bogus://x".into(), e.hook()])).await;
    let n = e
        .dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn failures_are_retried_with_exponential_backoff_then_given_up() {
    let e = env().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&e.server)
        .await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    e.dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    let worker = e.worker();
    let c = CancellationToken::new();
    let at = |s: u64| now() + Duration::from_secs(s);

    worker.run_once(at(0), c.clone()).await.unwrap(); // 1 回目: 失敗 → 30 秒後
    assert_eq!(request_count(e.server.received_requests().await), 1);
    worker.run_once(at(29), c.clone()).await.unwrap(); // まだ早い
    assert_eq!(request_count(e.server.received_requests().await), 1);
    worker.run_once(at(30), c.clone()).await.unwrap(); // 2 回目: 失敗 → 60 秒後
    worker.run_once(at(89), c.clone()).await.unwrap(); // まだ早い
    assert_eq!(request_count(e.server.received_requests().await), 2);
    worker.run_once(at(90), c.clone()).await.unwrap(); // 3 回目: 失敗 → 120 秒後
    worker.run_once(at(210), c.clone()).await.unwrap(); // 4 回目 (再送 3 回目): 失敗 → 打ち切り
    assert_eq!(request_count(e.server.received_requests().await), 4);

    worker.run_once(at(100_000), c).await.unwrap();
    assert_eq!(
        request_count(e.server.received_requests().await),
        4,
        "no more attempts"
    );
    let left = e.store.list_notifications().await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].status, OutboxStatus::Failed);
    assert!(left[0].last_error.as_deref().unwrap().contains("500"));
}

#[tokio::test]
async fn a_retry_that_succeeds_clears_the_queue() {
    let e = env().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&e.server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .with_priority(2)
        .mount(&e.server)
        .await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    e.dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    let worker = e.worker();
    worker
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    worker
        .run_once(now() + Duration::from_secs(30), CancellationToken::new())
        .await
        .unwrap();
    assert!(e.store.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_stored_target_that_no_longer_parses_fails_permanently_without_sending() {
    let e = env().await;
    let w = e.watch(ConfigLayer::default()).await;
    e.store
        .enqueue_notification(
            &watchtower_store::NewNotification {
                watch_id: Some(w.id),
                target: "bogus://x".into(),
                title: "t".into(),
                body: "b".into(),
            },
            now(),
        )
        .await
        .unwrap();
    e.worker()
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    let left = e.store.list_notifications().await.unwrap();
    assert_eq!(left[0].status, OutboxStatus::Failed);
    assert_eq!(request_count(e.server.received_requests().await), 0);
}

#[tokio::test]
async fn cancelled_sends_are_not_counted_as_attempts() {
    let e = env().await;
    mock_ok(&e.server).await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    e.dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    e.worker().run_once(now(), cancel).await.unwrap();
    let left = e.store.list_notifications().await.unwrap();
    assert_eq!(
        (left.len(), left[0].attempts, left[0].status),
        (1, 0, OutboxStatus::Pending)
    );
}

#[tokio::test]
async fn run_loop_delivers_and_stops_on_cancel() {
    let e = env().await;
    mock_ok(&e.server).await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    e.dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), OffsetDateTime::now_utc())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let worker = Arc::new(e.worker());
    let task = tokio::spawn({
        let (worker, cancel) = (worker.clone(), cancel.clone());
        async move { worker.run(Duration::from_millis(50), cancel).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while request_count(e.server.received_requests().await) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("delivered");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn notify_urls_from_tags_are_used() {
    let e = env().await;
    let mut tag = watchtower_core::Tag::new("shops");
    tag.config.notify_urls = vec![e.hook()];
    e.store.add_tag(&tag).await.unwrap();
    let mut w = e.watch(ConfigLayer::default()).await;
    w.tags = vec![tag.id];
    e.store.update_watch(&w).await.unwrap();
    let n = e
        .dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn stored_notifications_are_size_capped() {
    let e = env().await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    let mut ev = event(&w, false);
    ev.diff.added = vec!["x".repeat(500_000)];
    e.dispatcher(ConfigLayer::default())
        .on_change(&ev, now())
        .await
        .unwrap();
    let stored = e.store.list_notifications().await.unwrap();
    assert!(
        stored[0].body.chars().count() <= 16_001,
        "{}",
        stored[0].body.len()
    );
    assert!(stored[0].title.chars().count() <= 501);
}

#[tokio::test]
async fn the_worker_prunes_old_failed_rows() {
    let e = env().await;
    let w = e.watch(ConfigLayer::default()).await;
    for _ in 0..600 {
        let id = e
            .store
            .enqueue_notification(
                &watchtower_store::NewNotification {
                    watch_id: Some(w.id),
                    target: "bogus://x".into(),
                    title: "t".into(),
                    body: "b".into(),
                },
                now(),
            )
            .await
            .unwrap();
        e.store.fail_notification(id, "old").await.unwrap();
    }
    e.worker()
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    assert!(e.store.list_notifications().await.unwrap().len() <= 500);
}

#[tokio::test]
async fn delivery_outcomes_are_recorded_as_events() {
    let e = env().await;
    mock_ok(&e.server).await;
    let w = e.watch(cfg_with(vec![e.hook()])).await;
    e.dispatcher(ConfigLayer::default())
        .on_change(&event(&w, false), now())
        .await
        .unwrap();
    e.worker()
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    let ev = e.store.list_events(Some(w.id), 10).await.unwrap();
    assert_eq!(
        ev.iter().map(|x| x.kind.as_str()).collect::<Vec<_>>(),
        ["notification_sent"]
    );

    // 恒久的な失敗
    e.store
        .enqueue_notification(
            &watchtower_store::NewNotification {
                watch_id: Some(w.id),
                target: "bogus://SECRETtoken/x".into(),
                title: "t".into(),
                body: "b".into(),
            },
            now(),
        )
        .await
        .unwrap();
    e.worker()
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    let ev = e.store.list_events(Some(w.id), 10).await.unwrap();
    assert_eq!(ev[0].kind, "notification_failed");
    assert!(!ev[0].detail.as_deref().unwrap_or("").contains("SECRET"));
}
