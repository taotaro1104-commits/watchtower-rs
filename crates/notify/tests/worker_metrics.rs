//! 通知の成功・恒久的な失敗がメトリクスに数えられること。

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_metrics::{Gauges, Metrics};
use watchtower_notify::{HttpNotifier, OutboxWorker, RetryPolicy};
use watchtower_store::{NewNotification, Store};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

fn metric(m: &Metrics, name: &str) -> f64 {
    m.render(&Gauges::default())
        .lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap()
        .parse()
        .unwrap()
}

async fn setup(
    status: u16,
    max_retries: u32,
) -> (
    Store,
    MockServer,
    OutboxWorker,
    Arc<Metrics>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    let metrics = Arc::new(Metrics::new());
    let fetcher = Arc::new(HttpFetcher::new(SsrfPolicy::allow_loopback()));
    let worker = OutboxWorker::new(
        store.clone(),
        Arc::new(HttpNotifier::new(fetcher)),
        RetryPolicy {
            max_retries,
            base: Duration::from_secs(30),
        },
    )
    .with_metrics(Arc::clone(&metrics));
    (store, server, worker, metrics, dir)
}

async fn enqueue(store: &Store, server: &MockServer) {
    store
        .enqueue_notification(
            &NewNotification {
                watch_id: None,
                target: format!("json://{}/hook", server.address()),
                title: "t".into(),
                body: "b".into(),
            },
            now(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_delivered_notification_is_counted() {
    let (store, server, worker, metrics, _d) = setup(200, 3).await;
    enqueue(&store, &server).await;
    worker
        .run_once(now(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(metric(&metrics, "watchtower_notifications_sent_total"), 1.0);
    assert_eq!(
        metric(&metrics, "watchtower_notifications_failed_total"),
        0.0
    );
}

#[tokio::test]
async fn retries_are_not_failures_but_giving_up_is() {
    let (store, server, worker, metrics, _d) = setup(500, 1).await;
    enqueue(&store, &server).await;
    let c = CancellationToken::new();
    worker.run_once(now(), c.clone()).await.unwrap(); // 1 回目の失敗 → 再送待ち
    assert_eq!(
        metric(&metrics, "watchtower_notifications_failed_total"),
        0.0
    );
    worker
        .run_once(now() + Duration::from_secs(30), c)
        .await
        .unwrap(); // 再送も失敗 → 打ち切り
    assert_eq!(
        metric(&metrics, "watchtower_notifications_failed_total"),
        1.0
    );
    assert_eq!(metric(&metrics, "watchtower_notifications_sent_total"), 0.0);
}
