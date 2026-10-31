//! `CdpFetcher`: 外部の Chrome に DevTools Protocol (WebSocket) でつないで描画後の HTML を取る。
//! ここでは偽の CDP サーバを相手に、プロトコルの流れ・SSRF 対策・後始末を確かめる。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{CdpFetcher, FetchError, FetchRequest, Fetcher, SsrfPolicy};

// ---- 偽のブラウザ -----------------------------------------------------------

#[derive(Clone)]
struct Behavior {
    status: u16,
    mime: &'static str,
    html: String,
    /// `Page.navigate` が返すネットワークエラー。
    navigate_error: Option<&'static str>,
    /// true なら、ナビゲーションを始めても読み込み完了を永遠に知らせない。
    hang: bool,
    /// 読み込み中に `Fetch.requestPaused` で止まる追加のリクエスト URL。
    subrequests: Vec<&'static str>,
    screenshot: Vec<u8>,
    /// 接続したまま何も返さずに切る。
    drop_after_connect: bool,
    /// 文書の応答が報告する URL (リダイレクト後の最終 URL を模す)。
    final_url: Option<&'static str>,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            status: 200,
            mime: "text/html",
            html: "<html><body><h1>rendered</h1></body></html>".into(),
            navigate_error: None,
            hang: false,
            subrequests: Vec::new(),
            screenshot: b"\x89PNG-fake".to_vec(),
            drop_after_connect: false,
            final_url: None,
        }
    }
}

#[derive(Default)]
struct Log {
    /// 受け取ったコマンド (method, params)。
    commands: Vec<(String, Value)>,
    open_targets: usize,
    max_open_targets: usize,
    connections: usize,
}

struct FakeBrowser {
    ws_url: String,
    log: Arc<Mutex<Log>>,
}

impl FakeBrowser {
    fn methods(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .commands
            .iter()
            .map(|c| c.0.clone())
            .collect()
    }
    fn params_of(&self, method: &str) -> Vec<Value> {
        let l = self.log.lock().unwrap();
        l.commands
            .iter()
            .filter(|c| c.0 == method)
            .map(|c| c.1.clone())
            .collect()
    }
    fn count(&self, method: &str) -> usize {
        self.methods().iter().filter(|m| *m == method).count()
    }
}

async fn start(b: Behavior) -> FakeBrowser {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!(
        "ws://{}/devtools/browser/fake-id-SECRET",
        listener.local_addr().unwrap()
    );
    let log = Arc::new(Mutex::new(Log::default()));
    let log2 = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (b, log) = (b.clone(), Arc::clone(&log2));
            tokio::spawn(async move {
                log.lock().unwrap().connections += 1;
                if b.drop_after_connect {
                    return;
                }
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                serve(ws, b, log).await;
            });
        }
    });
    FakeBrowser { ws_url, log }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn send(ws: &mut Ws, v: Value) {
    let _ = ws.send(Message::Text(v.to_string().into())).await;
}

async fn recv(ws: &mut Ws) -> Option<Value> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return serde_json::from_str(t.as_str()).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn record(log: &Arc<Mutex<Log>>, m: &Value) {
    let method = m["method"].as_str().unwrap_or_default().to_string();
    log.lock()
        .unwrap()
        .commands
        .push((method, m["params"].clone()));
}

fn reply(m: &Value, result: Value) -> Value {
    let mut v = json!({"id": m["id"], "result": result});
    if let Some(s) = m.get("sessionId") {
        v["sessionId"] = s.clone();
    }
    v
}

async fn serve(mut ws: Ws, b: Behavior, log: Arc<Mutex<Log>>) {
    let session = "S1";
    while let Some(m) = recv(&mut ws).await {
        record(&log, &m);
        match m["method"].as_str().unwrap_or_default() {
            "Target.createTarget" => {
                {
                    let mut l = log.lock().unwrap();
                    l.open_targets += 1;
                    l.max_open_targets = l.max_open_targets.max(l.open_targets);
                }
                send(&mut ws, reply(&m, json!({"targetId": "T1"}))).await;
            }
            "Target.attachToTarget" => {
                send(&mut ws, reply(&m, json!({"sessionId": session}))).await
            }
            "Target.closeTarget" => {
                log.lock().unwrap().open_targets -= 1;
                send(&mut ws, reply(&m, json!({"success": true}))).await;
            }
            "Page.navigate" => navigate(&mut ws, &m, &b, session, &log).await,
            "Runtime.evaluate" => {
                send(
                    &mut ws,
                    reply(&m, json!({"result": {"type": "string", "value": b.html}})),
                )
                .await
            }
            "Page.captureScreenshot" => {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&b.screenshot);
                send(&mut ws, reply(&m, json!({"data": data}))).await
            }
            _ => send(&mut ws, reply(&m, json!({}))).await,
        }
    }
}

async fn navigate(ws: &mut Ws, m: &Value, b: &Behavior, session: &str, log: &Arc<Mutex<Log>>) {
    let url = m["params"]["url"].as_str().unwrap_or_default().to_string();
    if let Some(err) = b.navigate_error {
        send(
            ws,
            reply(
                m,
                json!({"frameId": "F1", "loaderId": "L1", "errorText": err}),
            ),
        )
        .await;
        return;
    }
    send(ws, reply(m, json!({"frameId": "F1", "loaderId": "L1"}))).await;
    if b.hang {
        return;
    }
    // 文書本体と追加のリクエストを、Chrome のように「止めて」クライアントの判断を待つ
    let mut paused = vec![(url.clone(), "Document")];
    paused.extend(b.subrequests.iter().map(|u| (u.to_string(), "Image")));
    for (i, (u, kind)) in paused.iter().enumerate() {
        send(
            ws,
            json!({"method": "Fetch.requestPaused", "sessionId": session, "params": {
                "requestId": format!("R{i}"), "request": {"url": u}, "resourceType": kind}}),
        )
        .await;
    }
    let mut answered = 0;
    while answered < paused.len() {
        let Some(c) = recv(ws).await else { return };
        record(log, &c);
        let method = c["method"].as_str().unwrap_or_default();
        if method == "Fetch.continueRequest" || method == "Fetch.failRequest" {
            answered += 1;
        }
        send(ws, reply(&c, json!({}))).await;
    }
    let reported = b.final_url.map_or_else(|| url.clone(), str::to_string);
    send(
        ws,
        json!({"method": "Network.responseReceived", "sessionId": session, "params": {
            "loaderId": "L1", "type": "Document",
            "response": {"url": reported, "status": b.status, "mimeType": b.mime}}}),
    )
    .await;
    send(ws, json!({"method": "Page.loadEventFired", "sessionId": session, "params": {"timestamp": 1.0}})).await;
}

// ---- 補助 -------------------------------------------------------------------

fn request(url: &str) -> FetchRequest {
    let mut r = FetchRequest::get(Url::parse(url).unwrap());
    r.timeout = Duration::from_secs(5);
    r
}

fn fetcher(b: &FakeBrowser) -> CdpFetcher {
    CdpFetcher::new(b.ws_url.clone(), SsrfPolicy::allow_loopback()).with_settle(Duration::ZERO)
}

const PAGE: &str = "http://127.0.0.1:8080/page";

async fn run(f: &CdpFetcher, url: &str) -> Result<watchtower_fetch::FetchResponse, FetchError> {
    f.fetch(&request(url), CancellationToken::new()).await
}

// ---- 正常系 -----------------------------------------------------------------

#[tokio::test]
async fn the_rendered_html_is_returned_with_status_and_content_type() {
    let b = start(Behavior::default()).await;
    let r = run(&fetcher(&b), PAGE).await.unwrap();
    assert_eq!(r.status, 200);
    assert!(r.body.contains("<h1>rendered</h1>"), "{}", r.body);
    assert_eq!(r.content_type.as_deref(), Some("text/html"));
    assert_eq!(r.final_url.as_str(), PAGE);
    assert!(r.raw.is_none() && r.screenshot.is_none());
}

#[tokio::test]
async fn the_tab_is_always_closed_afterwards() {
    let b = start(Behavior::default()).await;
    run(&fetcher(&b), PAGE).await.unwrap();
    assert_eq!(b.count("Target.createTarget"), 1);
    assert_eq!(b.count("Target.closeTarget"), 1);
    assert_eq!(b.log.lock().unwrap().open_targets, 0);
}

#[tokio::test]
async fn a_screenshot_is_returned_only_when_enabled() {
    let b = start(Behavior::default()).await;
    let r = run(&fetcher(&b).with_screenshots(true), PAGE)
        .await
        .unwrap();
    assert_eq!(r.screenshot.as_deref(), Some(&b"\x89PNG-fake"[..]));
    let b2 = start(Behavior::default()).await;
    run(&fetcher(&b2), PAGE).await.unwrap();
    assert_eq!(b2.count("Page.captureScreenshot"), 0);
}

#[tokio::test]
async fn custom_request_headers_are_passed_to_the_browser() {
    let b = start(Behavior::default()).await;
    let mut req = request(PAGE);
    req.headers = vec![("X-Token".into(), "abc".into())];
    fetcher(&b)
        .fetch(&req, CancellationToken::new())
        .await
        .unwrap();
    let params = b.params_of("Network.setExtraHTTPHeaders");
    assert_eq!(params[0]["headers"]["X-Token"], "abc");
}

// ---- ステータスとエラー -----------------------------------------------------

#[tokio::test]
async fn error_statuses_fail_unless_accepted_and_the_tab_is_still_closed() {
    let b = start(Behavior {
        status: 404,
        ..Default::default()
    })
    .await;
    let err = run(&fetcher(&b), PAGE).await.unwrap_err();
    assert!(matches!(err, FetchError::Status(404)), "{err:?}");
    assert_eq!(b.count("Target.closeTarget"), 1);

    let mut req = request(PAGE);
    req.accepted_statuses = vec![404];
    assert!(fetcher(&b)
        .fetch(&req, CancellationToken::new())
        .await
        .is_ok());
}

#[tokio::test]
async fn navigation_errors_are_reported_without_leaking_details() {
    let b = start(Behavior {
        navigate_error: Some("net::ERR_NAME_NOT_RESOLVED"),
        ..Default::default()
    })
    .await;
    let err = run(&fetcher(&b), PAGE).await.unwrap_err();
    assert!(matches!(err, FetchError::Http(_)), "{err:?}");
    assert!(!err.to_string().contains("SECRET"), "{err}");
    assert_eq!(b.count("Target.closeTarget"), 1);
}

#[tokio::test]
async fn a_page_that_never_finishes_loading_times_out_and_is_cleaned_up() {
    let b = start(Behavior {
        hang: true,
        ..Default::default()
    })
    .await;
    let mut req = request(PAGE);
    req.timeout = Duration::from_millis(400);
    let started = Instant::now();
    let err = fetcher(&b)
        .fetch(&req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Timeout), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        b.count("Target.closeTarget"),
        1,
        "the tab must be closed after a timeout"
    );
}

#[tokio::test]
async fn cancellation_stops_the_fetch_and_closes_the_tab() {
    let b = start(Behavior {
        hang: true,
        ..Default::default()
    })
    .await;
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let err = fetcher(&b).fetch(&request(PAGE), cancel).await.unwrap_err();
    assert!(matches!(err, FetchError::Cancelled), "{err:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(b.count("Target.closeTarget"), 1);
}

#[tokio::test]
async fn an_unreachable_browser_is_an_error_that_does_not_reveal_the_endpoint() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let f = CdpFetcher::new(
        format!("ws://127.0.0.1:{port}/devtools/browser/TOKEN-SECRET"),
        SsrfPolicy::allow_loopback(),
    );
    let err = run(&f, PAGE).await.unwrap_err();
    assert!(matches!(err, FetchError::Http(_)), "{err:?}");
    assert!(
        !err.to_string().contains("SECRET") && !err.to_string().contains(&port.to_string()),
        "{err}"
    );
}

#[tokio::test]
async fn a_browser_that_drops_the_connection_does_not_hang_the_fetch() {
    let b = start(Behavior {
        drop_after_connect: true,
        ..Default::default()
    })
    .await;
    let started = Instant::now();
    let err = run(&fetcher(&b), PAGE).await.unwrap_err();
    assert!(
        matches!(err, FetchError::Http(_) | FetchError::Timeout),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[tokio::test]
async fn oversized_documents_are_rejected() {
    let b = start(Behavior {
        html: "x".repeat(5_000),
        ..Default::default()
    })
    .await;
    let mut req = request(PAGE);
    req.max_body_bytes = 1_000;
    let err = fetcher(&b)
        .fetch(&req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::TooLarge { .. }), "{err:?}");
}

#[tokio::test]
async fn only_get_requests_are_supported() {
    let b = start(Behavior::default()).await;
    let mut req = request(PAGE);
    req.method = http::Method::POST;
    let err = fetcher(&b)
        .fetch(&req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::InvalidRequest(_)), "{err:?}");
    assert_eq!(
        b.log.lock().unwrap().connections,
        0,
        "rejected before connecting"
    );
}

// ---- SSRF -------------------------------------------------------------------

#[tokio::test]
async fn private_addresses_are_blocked_before_the_browser_is_even_contacted() {
    let b = start(Behavior::default()).await;
    let strict = CdpFetcher::new(b.ws_url.clone(), SsrfPolicy::default());
    for url in [
        "http://127.0.0.1/",
        "http://10.0.0.5/x",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/",
        "http://localhost/",
    ] {
        let err = strict
            .fetch(&request(url), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Blocked(_)), "{url}: {err:?}");
    }
    assert_eq!(b.log.lock().unwrap().connections, 0);
}

#[tokio::test]
async fn non_http_schemes_are_refused() {
    let b = start(Behavior::default()).await;
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/",
        "chrome://version",
        "javascript:alert(1)",
    ] {
        let err = fetcher(&b)
            .fetch(&request(url), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, FetchError::InvalidRequest(_) | FetchError::Blocked(_)),
            "{url}: {err:?}"
        );
    }
}

#[tokio::test]
async fn subrequests_and_redirects_are_checked_one_by_one() {
    let b = start(Behavior {
        subrequests: vec![
            "http://127.0.0.1:9000/allowed.png",
            "http://169.254.169.254/latest/meta-data",
            "file:///etc/passwd",
            "http://10.1.2.3/internal",
        ],
        ..Default::default()
    })
    .await;
    // 127.0.0.1 だけを許可したポリシー
    run(&fetcher(&b), PAGE).await.unwrap();

    let continued: Vec<String> = b
        .params_of("Fetch.continueRequest")
        .iter()
        .map(|p| p["requestId"].as_str().unwrap().to_string())
        .collect();
    let failed = b.params_of("Fetch.failRequest");
    assert_eq!(
        continued,
        ["R0", "R1"],
        "the document and the allowed image"
    );
    let failed_ids: Vec<&str> = failed
        .iter()
        .map(|p| p["requestId"].as_str().unwrap())
        .collect();
    assert_eq!(failed_ids, ["R2", "R3", "R4"]);
    assert!(failed.iter().all(|p| p["errorReason"] == "BlockedByClient"));
}

#[tokio::test]
async fn a_document_that_ended_up_on_an_internal_address_is_refused() {
    // 事前検査は通るが、リダイレクトの末に内部アドレスの文書を受け取った場合は、中身を返さない
    let b = start(Behavior {
        final_url: Some("http://10.9.9.9/secret"),
        html: "TOP-SECRET".into(),
        ..Default::default()
    })
    .await;
    let err = run(&fetcher(&b), PAGE).await.unwrap_err();
    assert!(matches!(err, FetchError::Blocked(_)), "{err:?}");
    assert!(!err.to_string().contains("TOP-SECRET"));
    assert_eq!(b.count("Runtime.evaluate"), 0, "the content is never read");
    assert_eq!(b.count("Target.closeTarget"), 1);
}

// ---- 同時実行 ---------------------------------------------------------------

#[tokio::test]
async fn the_number_of_open_tabs_is_limited() {
    let b = start(Behavior {
        html: "ok".into(),
        ..Default::default()
    })
    .await;
    let counter = Arc::new(AtomicUsize::new(0));
    let f = Arc::new(fetcher(&b).with_max_tabs(1));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let (f, counter) = (Arc::clone(&f), Arc::clone(&counter));
        tasks.push(tokio::spawn(async move {
            let r = f.fetch(&request(PAGE), CancellationToken::new()).await;
            counter.fetch_add(1, Ordering::SeqCst);
            r
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 4);
    assert_eq!(b.log.lock().unwrap().max_open_targets, 1);
}
