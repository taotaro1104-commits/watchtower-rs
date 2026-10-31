//! `use_browser` のウォッチはブラウザ用の取得器を、そうでないウォッチは通常の取得器を使う。

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::{BrowserStep, Watch};
use watchtower_fetch::{FetchError, FetchRequest, FetchResponse, Fetcher};
use watchtower_scheduler::{CheckOutcome, CheckRunner, CheckerConfig, StoreCheckRunner};
use watchtower_store::Store;

/// 決まった本文を返す。どちらの取得器が使われたかは、本文で見分ける。受け取った操作も記録する。
struct Labeled(&'static str, std::sync::Mutex<Vec<usize>>);

#[async_trait]
impl Fetcher for Labeled {
    async fn fetch(
        &self,
        req: &FetchRequest,
        _c: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        self.1.lock().unwrap().push(req.browser_steps.len());
        Ok(FetchResponse {
            status: 200,
            headers: http::HeaderMap::new(),
            content_type: Some("text/plain".into()),
            body: self.0.to_string(),
            raw: None,
            screenshot: None,
            final_url: req.url.clone(),
        })
    }
}

struct Fixture {
    runner: StoreCheckRunner,
    rx: mpsc::Receiver<CheckOutcome>,
    store: Store,
    id: uuid::Uuid,
    browser: Arc<Labeled>,
    _dir: tempfile::TempDir,
}

async fn fixture(use_browser: Option<bool>, with_browser: bool) -> Fixture {
    fixture_with_steps(use_browser, with_browser, Vec::new()).await
}

async fn fixture_with_steps(
    use_browser: Option<bool>,
    with_browser: bool,
    steps: Vec<BrowserStep>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/").unwrap());
    w.config.use_browser = use_browser;
    w.config.browser_steps = steps;
    store.add_watch(&w).await.unwrap();
    let (tx, rx) = mpsc::channel(8);
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        ..Default::default()
    };
    let browser = Arc::new(Labeled("from-browser", Default::default()));
    let mut runner = StoreCheckRunner::new(
        store.clone(),
        Arc::new(Labeled("from-http", Default::default())),
        cfg,
        Some(tx),
    );
    if with_browser {
        runner = runner.with_browser(browser.clone());
    }
    Fixture {
        runner,
        rx,
        store,
        id: w.id,
        browser,
        _dir: dir,
    }
}

async fn check(f: &mut Fixture) -> CheckOutcome {
    f.runner.run(f.id, CancellationToken::new()).await;
    f.rx.recv().await.unwrap()
}

async fn stored(f: &Fixture) -> String {
    f.store.latest_snapshot_text(f.id).await.unwrap().unwrap()
}

#[tokio::test]
async fn browser_watches_use_the_browser_fetcher() {
    let mut f = fixture(Some(true), true).await;
    check(&mut f).await;
    assert_eq!(stored(&f).await, "from-browser");
}

#[tokio::test]
async fn other_watches_keep_using_plain_http_even_when_a_browser_exists() {
    for flag in [None, Some(false)] {
        let mut f = fixture(flag, true).await;
        check(&mut f).await;
        assert_eq!(stored(&f).await, "from-http", "{flag:?}");
    }
}

#[tokio::test]
async fn a_browser_watch_without_a_configured_browser_reports_a_clear_error() {
    let mut f = fixture(Some(true), false).await;
    let out = check(&mut f).await;
    let err = out.error.expect("an error");
    assert!(err.contains("browser"), "{err}");
    let w = f.store.get_watch(f.id).await.unwrap().unwrap();
    assert_eq!(w.state.consecutive_errors, 1);
    assert!(f.store.latest_snapshot_text(f.id).await.unwrap().is_none());
}

fn steps() -> Vec<BrowserStep> {
    vec![
        BrowserStep::Click {
            selector: "#go".into(),
        },
        BrowserStep::Wait { ms: 10 },
    ]
}

#[tokio::test]
async fn the_configured_steps_are_handed_to_the_browser_fetcher() {
    let mut f = fixture_with_steps(Some(true), true, steps()).await;
    check(&mut f).await;
    assert_eq!(*f.browser.1.lock().unwrap(), vec![2]);
}

#[tokio::test]
async fn steps_on_a_plain_http_watch_are_an_error_not_silently_ignored() {
    let mut f = fixture_with_steps(Some(false), true, steps()).await;
    let out = check(&mut f).await;
    let err = out.error.expect("an error");
    assert!(err.contains("browser") && err.contains("steps"), "{err}");
    assert!(f.store.latest_snapshot_text(f.id).await.unwrap().is_none());
}
