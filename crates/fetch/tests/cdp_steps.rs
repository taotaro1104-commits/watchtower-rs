//! ブラウザ操作ステップ (click / type / wait_for / wait) を、偽の CDP サーバ相手に確かめる。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::BrowserStep;
use watchtower_fetch::{CdpFetcher, FetchError, FetchRequest, Fetcher, SsrfPolicy};

const HTML: &str = "<html><body>after steps</body></html>";

/// 手順の評価結果を順に返す (尽きたら "ok")。ページ全体の取得 (outerHTML) には HTML を返す。
#[derive(Default)]
struct Script {
    results: Vec<&'static str>,
    /// 手順の評価に対して JavaScript の例外を返す。
    throw: bool,
    /// 要素検査の式 (`/*pick*/`) への返事。
    report: Option<String>,
}

struct Fake {
    ws_url: String,
    /// `Runtime.evaluate` で受け取った式と、全コマンド名。
    expressions: Arc<Mutex<Vec<String>>>,
    methods: Arc<Mutex<Vec<String>>>,
    /// 受け取ったコマンドとパラメータ。
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

async fn start(script: Script) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!(
        "ws://{}/devtools/browser/SECRET-ID",
        listener.local_addr().unwrap()
    );
    let expressions = Arc::new(Mutex::new(Vec::new()));
    let methods = Arc::new(Mutex::new(Vec::new()));
    let (ex, me) = (Arc::clone(&expressions), Arc::clone(&methods));
    let results = Arc::new(Mutex::new(script.results));
    let throw = script.throw;
    let report = script.report.clone();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls2 = Arc::clone(&calls);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (ex, me, results) = (Arc::clone(&ex), Arc::clone(&me), Arc::clone(&results));
            let (report, calls) = (report.clone(), Arc::clone(&calls2));
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(t) = msg else { continue };
                    let m: Value = serde_json::from_str(t.as_str()).unwrap();
                    let method = m["method"].as_str().unwrap_or_default().to_string();
                    me.lock().unwrap().push(method.clone());
                    calls
                        .lock()
                        .unwrap()
                        .push((method.clone(), m["params"].clone()));
                    let result = match method.as_str() {
                        "Target.createTarget" => json!({"targetId": "T1"}),
                        "Target.attachToTarget" => json!({"sessionId": "S1"}),
                        "Runtime.evaluate" => {
                            let expr = m["params"]["expression"].as_str().unwrap_or_default();
                            ex.lock().unwrap().push(expr.to_string());
                            if expr.contains("/*pick*/") {
                                json!({"result": {"type": "string", "value": report.clone().unwrap_or_default()}})
                            } else if expr.contains("outerHTML") {
                                json!({"result": {"type": "string", "value": HTML}})
                            } else if throw {
                                json!({"exceptionDetails": {"text": "boom SECRET-TEXT"}})
                            } else {
                                let mut r = results.lock().unwrap();
                                let v = if r.is_empty() { "ok" } else { r.remove(0) };
                                json!({"result": {"type": "string", "value": v}})
                            }
                        }
                        "Page.captureScreenshot" => json!({"data": "UE5HLWZha2U="}),
                        _ => json!({}),
                    };
                    let mut reply = json!({"id": m["id"], "result": result});
                    if let Some(s) = m.get("sessionId") {
                        reply["sessionId"] = s.clone();
                    }
                    let _ = ws.send(Message::Text(reply.to_string().into())).await;
                    if method == "Page.navigate" {
                        for ev in [
                            json!({"method": "Fetch.requestPaused", "sessionId": "S1", "params": {
                                "requestId": "R0", "request": {"url": "http://127.0.0.1:8080/p"},
                                "resourceType": "Document"}}),
                        ] {
                            let _ = ws.send(Message::Text(ev.to_string().into())).await;
                        }
                        // continueRequest を待ってから文書の応答と読み込み完了を知らせる
                        while let Some(Ok(Message::Text(c))) = ws.next().await {
                            let c: Value = serde_json::from_str(c.as_str()).unwrap();
                            me.lock()
                                .unwrap()
                                .push(c["method"].as_str().unwrap_or_default().into());
                            let _ = ws
                                .send(Message::Text(
                                    json!({"id": c["id"], "sessionId": "S1", "result": {}})
                                        .to_string()
                                        .into(),
                                ))
                                .await;
                            if c["method"] == "Fetch.continueRequest" {
                                break;
                            }
                        }
                        for ev in [
                            json!({"method": "Network.responseReceived", "sessionId": "S1", "params": {
                                "type": "Document",
                                "response": {"url": "http://127.0.0.1:8080/p", "status": 200, "mimeType": "text/html"}}}),
                            json!({"method": "Page.loadEventFired", "sessionId": "S1", "params": {}}),
                        ] {
                            let _ = ws.send(Message::Text(ev.to_string().into())).await;
                        }
                    }
                }
            });
        }
    });
    Fake {
        ws_url,
        expressions,
        methods,
        calls,
    }
}

async fn run(
    f: &Fake,
    steps: Vec<BrowserStep>,
) -> Result<watchtower_fetch::FetchResponse, FetchError> {
    let fetcher =
        CdpFetcher::new(f.ws_url.clone(), SsrfPolicy::allow_loopback()).with_settle(Duration::ZERO);
    let mut req = FetchRequest::get(Url::parse("http://127.0.0.1:8080/p").unwrap());
    req.timeout = Duration::from_secs(5);
    req.browser_steps = steps;
    fetcher.fetch(&req, CancellationToken::new()).await
}

fn click(s: &str) -> BrowserStep {
    BrowserStep::Click { selector: s.into() }
}

fn step_expressions(f: &Fake) -> Vec<String> {
    f.expressions
        .lock()
        .unwrap()
        .iter()
        .filter(|e| !e.contains("outerHTML"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn steps_run_in_order_after_the_page_loaded_and_before_the_html_is_read() {
    let f = start(Script::default()).await;
    let r = run(
        &f,
        vec![
            click("#accept"),
            BrowserStep::Type {
                selector: "input[name=q]".into(),
                text: "hello".into(),
            },
            BrowserStep::WaitFor {
                selector: ".result".into(),
                timeout_ms: None,
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(r.body, HTML);

    let all = f.expressions.lock().unwrap().clone();
    assert_eq!(all.len(), 4, "{all:?}");
    assert!(
        all[0].contains("#accept") && all[0].contains(".click()"),
        "{}",
        all[0]
    );
    assert!(
        all[1].contains("input[name=q]") && all[1].contains("hello"),
        "{}",
        all[1]
    );
    assert!(
        all[1].contains("\"input\"") && all[1].contains("\"change\""),
        "{}",
        all[1]
    );
    assert!(all[2].contains(".result"), "{}", all[2]);
    assert!(all[3].contains("outerHTML"), "the HTML is read last");
}

#[tokio::test]
async fn selectors_and_text_are_embedded_as_string_literals() {
    let f = start(Script::default()).await;
    let evil = r#"a"); window.stolen=1; ("#;
    run(
        &f,
        vec![BrowserStep::Type {
            selector: evil.into(),
            text: "x\"y\\z\n</script>".into(),
        }],
    )
    .await
    .unwrap();
    let exprs = step_expressions(&f);
    let literal = serde_json::to_string(evil).unwrap();
    assert!(exprs[0].contains(&literal), "{}", exprs[0]);
    assert!(
        !exprs[0].contains(&format!("querySelector({evil}")),
        "never spliced in raw: {}",
        exprs[0]
    );
}

#[tokio::test]
async fn a_missing_element_fails_the_fetch_and_the_tab_is_closed() {
    let f = start(Script {
        results: vec!["ok", "missing"],
        ..Default::default()
    })
    .await;
    let err = run(
        &f,
        vec![
            click("#a"),
            BrowserStep::Type {
                selector: "#password-SECRET-SELECTOR".into(),
                text: "hunter2-SECRET-TEXT".into(),
            },
            click("#never"),
        ],
    )
    .await
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("step 2") && msg.contains("type"), "{msg}");
    assert!(!msg.contains("SECRET"), "{msg}");
    assert_eq!(step_expressions(&f).len(), 2, "later steps must not run");
    assert!(f
        .methods
        .lock()
        .unwrap()
        .iter()
        .any(|m| m == "Target.closeTarget"));
}

#[tokio::test]
async fn an_invalid_selector_or_page_exception_is_a_clean_error() {
    let f = start(Script {
        results: vec!["invalid"],
        ..Default::default()
    })
    .await;
    let err = run(&f, vec![click("###")]).await.unwrap_err().to_string();
    assert!(err.contains("step 1") && err.contains("selector"), "{err}");

    let f = start(Script {
        throw: true,
        ..Default::default()
    })
    .await;
    let err = run(&f, vec![click("#a")]).await.unwrap_err().to_string();
    assert!(err.contains("step 1"), "{err}");
    assert!(
        !err.contains("SECRET"),
        "page error text must not leak: {err}"
    );
}

#[tokio::test]
async fn wait_for_polls_until_the_element_appears() {
    let f = start(Script {
        results: vec!["missing", "missing", "ok"],
        ..Default::default()
    })
    .await;
    let r = run(
        &f,
        vec![BrowserStep::WaitFor {
            selector: ".late".into(),
            timeout_ms: Some(5000),
        }],
    )
    .await
    .unwrap();
    assert_eq!(r.body, HTML);
    assert_eq!(step_expressions(&f).len(), 3);
}

#[tokio::test]
async fn wait_for_gives_up_after_its_own_timeout() {
    let f = start(Script {
        results: vec!["missing"; 1000],
        ..Default::default()
    })
    .await;
    let started = Instant::now();
    let err = run(
        &f,
        vec![BrowserStep::WaitFor {
            selector: ".never".into(),
            timeout_ms: Some(300),
        }],
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(err.contains("step 1") && err.contains("wait_for"), "{err}");
    assert!(
        err.contains("timed out") || err.contains("not appear"),
        "{err}"
    );
}

#[tokio::test]
async fn a_wait_step_pauses_without_evaluating_anything() {
    let f = start(Script::default()).await;
    let started = Instant::now();
    run(&f, vec![BrowserStep::Wait { ms: 200 }]).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(190));
    assert!(step_expressions(&f).is_empty());
}

#[tokio::test]
async fn invalid_steps_are_refused_before_the_browser_is_contacted() {
    let f = start(Script::default()).await;
    let err = run(&f, vec![click("")]).await.unwrap_err();
    assert!(matches!(err, FetchError::InvalidRequest(_)), "{err:?}");
    assert!(
        f.methods.lock().unwrap().is_empty(),
        "no connection was made"
    );
}

#[tokio::test]
async fn no_steps_means_no_extra_evaluations() {
    let f = start(Script::default()).await;
    run(&f, vec![]).await.unwrap();
    assert!(step_expressions(&f).is_empty());
}

// ---- ビジュアルセレクタ (inspect) --------------------------------------------

const REPORT: &str = r##"{"w":1280,"h":900,"els":[
  {"s":"body:nth-of-type(1) > div:nth-of-type(1)","x":0,"y":0,"w":1280,"h":900,"t":"page"},
  {"s":"#price","x":100,"y":200,"w":80,"h":30,"t":"¥1,980"}]}"##;

async fn inspect(
    f: &Fake,
    steps: Vec<BrowserStep>,
) -> Result<watchtower_core::pick::Inspection, FetchError> {
    let fetcher =
        CdpFetcher::new(f.ws_url.clone(), SsrfPolicy::allow_loopback()).with_settle(Duration::ZERO);
    let mut req = FetchRequest::get(Url::parse("http://127.0.0.1:8080/p").unwrap());
    req.timeout = Duration::from_secs(5);
    req.browser_steps = steps;
    fetcher.inspect(&req, CancellationToken::new()).await
}

fn script_with_report(r: &str) -> Script {
    Script {
        report: Some(r.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn inspect_returns_the_screenshot_size_and_validated_elements() {
    let f = start(script_with_report(REPORT)).await;
    let i = inspect(&f, vec![]).await.unwrap();
    assert_eq!((i.width, i.height), (1280, 900));
    assert_eq!(i.screenshot, b"PNG-fake");
    assert_eq!(i.elements.len(), 2);
    assert_eq!(i.elements[1].selector, "#price");
    assert_eq!(i.elements[1].text, "¥1,980");
    assert!(f
        .methods
        .lock()
        .unwrap()
        .iter()
        .any(|m| m == "Target.closeTarget"));
}

#[tokio::test]
async fn inspect_renders_at_a_fixed_width_and_captures_the_whole_page() {
    let f = start(script_with_report(REPORT)).await;
    inspect(&f, vec![]).await.unwrap();
    let calls = f.calls.lock().unwrap().clone();
    let pos = |m: &str| calls.iter().position(|c| c.0 == m).unwrap();
    assert!(pos("Emulation.setDeviceMetricsOverride") < pos("Page.navigate"));
    let emu = &calls[pos("Emulation.setDeviceMetricsOverride")].1;
    assert_eq!(emu["width"], 1280);
    let shot = &calls[pos("Page.captureScreenshot")].1;
    assert_eq!(shot["captureBeyondViewport"], true);
    assert_eq!(shot["clip"]["width"], 1280);
    assert_eq!(shot["clip"]["height"], 900);
}

#[tokio::test]
async fn inspect_runs_the_configured_steps_first() {
    let f = start(script_with_report(REPORT)).await;
    inspect(&f, vec![click("#accept")]).await.unwrap();
    let exprs = f.expressions.lock().unwrap().clone();
    assert!(exprs[0].contains("#accept"), "{exprs:?}");
    assert!(exprs[1].contains("/*pick*/"), "{exprs:?}");
}

#[tokio::test]
async fn inspect_refuses_a_malformed_page_report_and_closes_the_tab() {
    for bad in ["not json", "{}", r#"{"w":0,"h":0,"els":[]}"#] {
        let f = start(script_with_report(bad)).await;
        let err = inspect(&f, vec![]).await.unwrap_err().to_string();
        assert!(err.contains("report"), "{bad}: {err}");
        assert!(f
            .methods
            .lock()
            .unwrap()
            .iter()
            .any(|m| m == "Target.closeTarget"));
    }
}

#[tokio::test]
async fn inspect_applies_the_same_address_policy_as_fetch() {
    let f = start(script_with_report(REPORT)).await;
    let fetcher = CdpFetcher::new(f.ws_url.clone(), SsrfPolicy::with_allowed_ips(vec![]));
    let req = FetchRequest::get(Url::parse("http://10.0.0.5/admin").unwrap());
    let err = fetcher
        .inspect(&req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Blocked(_)), "{err:?}");
    assert!(
        f.methods.lock().unwrap().is_empty(),
        "the browser was not contacted"
    );
}
