//! ウォッチごとのヘッダ・メソッド・ボディ・プロキシが、取得リクエストに反映される。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::{ConfigLayer, Watch};
use watchtower_fetch::{FetchError, FetchRequest, FetchResponse, Fetcher};
use watchtower_scheduler::{CheckOutcome, CheckRunner, CheckerConfig, StoreCheckRunner};
use watchtower_store::Store;

/// 受け取ったリクエストの中身。
#[derive(Debug, PartialEq)]
struct Call {
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    proxy: Option<String>,
}

/// 受け取ったリクエストを記録する。
#[derive(Default)]
struct Recorder(Mutex<Vec<Call>>);

#[async_trait]
impl Fetcher for Recorder {
    async fn fetch(
        &self,
        req: &FetchRequest,
        _c: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        self.0.lock().unwrap().push(Call {
            method: req.method.to_string(),
            headers: req.headers.clone(),
            body: req.body.clone(),
            proxy: req.proxy.clone(),
        });
        Ok(FetchResponse {
            status: 200,
            headers: http::HeaderMap::new(),
            content_type: Some("text/plain".into()),
            body: "ok".into(),
            raw: None,
            screenshot: None,
            final_url: req.url.clone(),
        })
    }
}

async fn run(watch: ConfigLayer, global: ConfigLayer) -> (Arc<Recorder>, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/").unwrap());
    w.config = watch;
    store.add_watch(&w).await.unwrap();
    let (tx, mut rx) = mpsc::channel::<CheckOutcome>(8);
    let rec = Arc::new(Recorder::default());
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        global,
        ..Default::default()
    };
    let runner = StoreCheckRunner::new(store, rec.clone(), cfg, Some(tx));
    runner.run(w.id, CancellationToken::new()).await;
    let out = rx.recv().await.unwrap();
    (rec, out.error)
}

fn h(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

#[tokio::test]
async fn a_plain_watch_sends_a_bare_get() {
    let (rec, err) = run(ConfigLayer::default(), ConfigLayer::default()).await;
    assert!(err.is_none());
    let calls = rec.0.lock().unwrap();
    assert_eq!(
        calls[0],
        Call {
            method: "GET".into(),
            headers: vec![],
            body: None,
            proxy: None
        }
    );
}

#[tokio::test]
async fn method_headers_body_and_proxy_reach_the_fetcher() {
    let watch = ConfigLayer {
        method: Some("POST".into()),
        headers: vec![h("X-Token", "abc")],
        body: Some("a=1".into()),
        proxy: Some("http://proxy.example:8080".into()),
        ..Default::default()
    };
    let global = ConfigLayer {
        headers: vec![h("x-token", "global"), h("Accept-Language", "ja")],
        ..Default::default()
    };
    let (rec, err) = run(watch, global).await;
    assert!(err.is_none());
    let calls = rec.0.lock().unwrap();
    let Call {
        method,
        headers,
        body,
        proxy,
    } = &calls[0];
    assert_eq!(method, "POST");
    assert_eq!(
        headers,
        &vec![h("X-Token", "abc"), h("Accept-Language", "ja")]
    );
    assert_eq!(body.as_deref(), Some(&b"a=1"[..]));
    assert_eq!(proxy.as_deref(), Some("http://proxy.example:8080"));
}

#[tokio::test]
async fn an_unusable_method_is_a_check_error_not_a_silent_get() {
    let watch = ConfigLayer {
        method: Some("not a method".into()),
        ..Default::default()
    };
    let (rec, err) = run(watch, ConfigLayer::default()).await;
    assert!(err.expect("an error").contains("method"));
    assert!(rec.0.lock().unwrap().is_empty());
}
