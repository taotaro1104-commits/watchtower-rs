//! 実 Store + 偽 Fetcher でチェック 1 回分の挙動を確認する。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::{Watch, WatchId};
use watchtower_fetch::{FetchError, FetchRequest, FetchResponse, Fetcher};
use watchtower_scheduler::{CheckOutcome, CheckRunner, CheckerConfig, StoreCheckRunner};
use watchtower_store::Store;

struct FakeFetcher(Mutex<VecDeque<Result<String, FetchError>>>);

impl FakeFetcher {
    fn new(items: Vec<Result<&str, FetchError>>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            items.into_iter().map(|r| r.map(str::to_string)).collect(),
        )))
    }
}

#[async_trait]
impl Fetcher for FakeFetcher {
    async fn fetch(
        &self,
        req: &FetchRequest,
        _c: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        let next = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("fetcher script exhausted");
        next.map(|body| FetchResponse {
            status: 200,
            headers: http::HeaderMap::new(),
            content_type: Some("text/plain".into()),
            body,
            raw: None,
            screenshot: None,
            final_url: req.url.clone(),
        })
    }
}

struct Fixture {
    store: Store,
    runner: StoreCheckRunner,
    rx: mpsc::Receiver<CheckOutcome>,
    id: WatchId,
    _dir: tempfile::TempDir,
}

async fn fixture(script: Vec<Result<&str, FetchError>>, tweak: impl FnOnce(&mut Watch)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/page").unwrap());
    w.config.check_interval_secs = Some(600);
    tweak(&mut w);
    store.add_watch(&w).await.unwrap();
    let (tx, rx) = mpsc::channel(16);
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        ..Default::default()
    };
    let runner = StoreCheckRunner::new(store.clone(), FakeFetcher::new(script), cfg, Some(tx));
    Fixture {
        store,
        runner,
        rx,
        id: w.id,
        _dir: dir,
    }
}

async fn run(f: &mut Fixture) -> (Option<Duration>, CheckOutcome) {
    let report = f.runner.run(f.id, CancellationToken::new()).await;
    (report.next_delay, f.rx.recv().await.unwrap())
}

#[tokio::test]
async fn first_check_stores_baseline_without_reporting_change() {
    let mut f = fixture(vec![Ok("hello")], |_| {}).await;
    let (delay, out) = run(&mut f).await;
    assert_eq!(delay, Some(Duration::from_secs(600)));
    assert!(out.first_snapshot);
    assert!(!out.changed);
    assert_eq!(out.error, None);
    assert_eq!(f.store.list_snapshots(f.id).await.unwrap().len(), 1);
    let st = f.store.get_watch(f.id).await.unwrap().unwrap().state;
    assert!(st.last_checked_at.is_some() && st.next_check_at.is_some());
    assert_eq!(st.consecutive_errors, 0);
}

#[tokio::test]
async fn detects_change_and_produces_diff() {
    let mut f = fixture(vec![Ok("a\nb"), Ok("a\nc")], |_| {}).await;
    run(&mut f).await;
    let (_, out) = run(&mut f).await;
    assert!(out.changed);
    let d = out.diff.unwrap();
    assert_eq!(d.added, vec!["c"]);
    assert_eq!(d.removed, vec!["b"]);
    let st = f.store.get_watch(f.id).await.unwrap().unwrap().state;
    assert!(st.last_changed_at.is_some());
}

#[tokio::test]
async fn unchanged_content_adds_no_snapshot() {
    let mut f = fixture(vec![Ok("same"), Ok("same")], |_| {}).await;
    run(&mut f).await;
    let (_, out) = run(&mut f).await;
    assert!(!out.changed && !out.first_snapshot);
    assert_eq!(f.store.list_snapshots(f.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn errors_back_off_and_success_resets() {
    let script = vec![
        Err(FetchError::Status(500)),
        Err(FetchError::Status(500)),
        Ok("fine"),
    ];
    let mut f = fixture(script, |_| {}).await;

    let (d1, o1) = run(&mut f).await;
    assert_eq!(d1, Some(Duration::from_secs(30)));
    assert!(o1.error.is_some());
    let st = f.store.get_watch(f.id).await.unwrap().unwrap().state;
    assert_eq!(st.consecutive_errors, 1);
    assert!(st.last_error.unwrap().contains("500"));

    let (d2, _) = run(&mut f).await;
    assert_eq!(d2, Some(Duration::from_secs(60)));

    let (d3, o3) = run(&mut f).await;
    assert_eq!(d3, Some(Duration::from_secs(600)));
    assert!(o3.error.is_none());
    let st = f.store.get_watch(f.id).await.unwrap().unwrap().state;
    assert_eq!((st.consecutive_errors, st.last_error), (0, None));
}

#[tokio::test]
async fn trigger_text_is_reported() {
    let mut f = fixture(vec![Ok("in stock now")], |w| {
        w.config.trigger_text = vec!["in stock".into()]
    })
    .await;
    let (_, out) = run(&mut f).await;
    assert!(out.triggered);
}

#[tokio::test]
async fn paused_or_missing_watches_are_not_rescheduled() {
    let f = fixture(vec![], |w| w.paused = true).await;
    let r = f.runner.run(f.id, CancellationToken::new()).await;
    assert_eq!(r.next_delay, None);
    assert_eq!(f.runner.describe(f.id).await.unwrap(), None);
    let r = f
        .runner
        .run(uuid::Uuid::new_v4(), CancellationToken::new())
        .await;
    assert_eq!(r.next_delay, None);
}

#[tokio::test]
async fn load_describes_active_watches_with_host() {
    let f = fixture(vec![], |_| {}).await;
    let all = f.runner.load().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].host, "example.com");
    assert_eq!(all[0].initial_delay, Duration::ZERO);
}

#[tokio::test]
async fn old_snapshots_are_garbage_collected() {
    let script: Vec<_> = (0..5)
        .map(|i| Ok(["v0", "v1", "v2", "v3", "v4"][i]))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let w = Watch::new(Url::parse("https://example.com/").unwrap());
    store.add_watch(&w).await.unwrap();
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        snapshots_keep: 2,
        ..Default::default()
    };
    let runner = StoreCheckRunner::new(store.clone(), FakeFetcher::new(script), cfg, None);
    for _ in 0..5 {
        runner.run(w.id, CancellationToken::new()).await;
    }
    assert_eq!(store.list_snapshots(w.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn missing_previous_snapshot_file_is_healed_instead_of_wedging_the_watch() {
    let mut f = fixture(vec![Ok("one"), Ok("two"), Ok("three")], |_| {}).await;
    run(&mut f).await;
    // 直近のスナップショットファイルが消えた状況
    let rec = f.store.latest_snapshot(f.id).await.unwrap().unwrap();
    std::fs::remove_file(&rec.path).unwrap();

    let (_, out) = run(&mut f).await;
    assert_eq!(
        out.error, None,
        "an unreadable baseline must not fail the check"
    );
    assert!(out.first_snapshot, "baseline is re-established");
    assert_eq!(
        f.store.latest_snapshot_text(f.id).await.unwrap().as_deref(),
        Some("two")
    );

    // 以降は通常どおり差分が取れる
    let (_, out) = run(&mut f).await;
    assert!(out.changed && !out.first_snapshot);
}

#[tokio::test]
async fn absurd_intervals_do_not_panic_or_spin() {
    for secs in [0u64, u64::MAX] {
        let mut f = fixture(vec![Err(FetchError::Status(500)), Ok("ok")], |w| {
            w.config.check_interval_secs = Some(secs)
        })
        .await;
        let (d, _) = run(&mut f).await;
        assert!(
            d.unwrap() >= Duration::from_secs(1),
            "error path floor for {secs}"
        );
        let (d, _) = run(&mut f).await;
        assert!(d.unwrap() >= Duration::from_secs(1));
        let st = f.store.get_watch(f.id).await.unwrap().unwrap().state;
        assert!(
            st.next_check_at.is_some(),
            "state must be persisted for {secs}"
        );
    }
}

async fn attach_tag(f: &Fixture, tag: watchtower_core::Tag) {
    f.store.add_tag(&tag).await.unwrap();
    let mut w = f.store.get_watch(f.id).await.unwrap().unwrap();
    w.tags = vec![tag.id, uuid::Uuid::new_v4()]; // 2 つ目は存在しないタグ (無視される)
    f.store.update_watch(&w).await.unwrap();
}

#[tokio::test]
async fn tag_settings_apply_to_the_check() {
    let mut f = fixture(vec![Ok("a\nnoise 1"), Ok("a\nnoise 2")], |_| {}).await;
    let mut tag = watchtower_core::Tag::new("t");
    tag.config.ignore_text = vec!["noise".into()];
    attach_tag(&f, tag).await;
    run(&mut f).await;
    let (_, out) = run(&mut f).await;
    assert!(
        !out.changed,
        "ignore_text from the tag must hide the noise line"
    );
}

#[tokio::test]
async fn watch_settings_beat_tag_settings_and_tag_beats_global() {
    let mut f = fixture(vec![Ok("x")], |w| w.config.check_interval_secs = None).await;
    let mut tag = watchtower_core::Tag::new("t");
    tag.config.check_interval_secs = Some(120);
    attach_tag(&f, tag).await;
    let (delay, _) = run(&mut f).await;
    assert_eq!(delay, Some(Duration::from_secs(120)));

    let mut f = fixture(vec![Ok("x")], |_| {}).await; // watch の 600 が優先
    let mut tag = watchtower_core::Tag::new("t");
    tag.config.check_interval_secs = Some(120);
    attach_tag(&f, tag).await;
    let (delay, _) = run(&mut f).await;
    assert_eq!(delay, Some(Duration::from_secs(600)));
}

fn metric(m: &watchtower_metrics::Metrics, name: &str) -> f64 {
    m.render(&watchtower_metrics::Gauges::default())
        .lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap_or_else(|| panic!("{name} not found"))
        .parse()
        .unwrap()
}

#[tokio::test]
async fn checks_failures_changes_and_fetch_time_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let w = Watch::new(Url::parse("https://example.com/page").unwrap());
    store.add_watch(&w).await.unwrap();
    let metrics = Arc::new(watchtower_metrics::Metrics::new());
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        metrics: Arc::clone(&metrics),
        ..Default::default()
    };
    // 基準 → 変化 → 変化なし → エラー
    let script = vec![Ok("v1"), Ok("v2"), Ok("v2"), Err(FetchError::Status(500))];
    let runner = StoreCheckRunner::new(store.clone(), FakeFetcher::new(script), cfg, None);
    for _ in 0..4 {
        runner.run(w.id, CancellationToken::new()).await;
    }
    assert_eq!(metric(&metrics, "watchtower_checks_total"), 4.0);
    assert_eq!(metric(&metrics, "watchtower_check_failures_total"), 1.0);
    assert_eq!(metric(&metrics, "watchtower_changes_total"), 1.0);
    assert_eq!(
        metric(&metrics, "watchtower_fetch_seconds_count"),
        4.0,
        "every fetch attempt is timed, including the failed one"
    );
}

#[tokio::test]
async fn a_cancelled_check_is_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let w = Watch::new(Url::parse("https://example.com/page").unwrap());
    store.add_watch(&w).await.unwrap();
    let metrics = Arc::new(watchtower_metrics::Metrics::new());
    let cfg = CheckerConfig {
        metrics: Arc::clone(&metrics),
        ..Default::default()
    };
    let runner = StoreCheckRunner::new(store, FakeFetcher::new(vec![Ok("x")]), cfg, None);
    let cancel = CancellationToken::new();
    cancel.cancel();
    runner.run(w.id, cancel).await;
    assert_eq!(metric(&metrics, "watchtower_checks_total"), 0.0);
}
