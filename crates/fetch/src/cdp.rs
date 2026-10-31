//! `CdpFetcher`: 外部の Chrome に DevTools Protocol (WebSocket) でつなぎ、描画後の HTML を取得する。
//!
//! 必要なコマンドは十数個だけなので、巨大なプロトコル定義の crate は使わず、最小のクライアントを持つ。
//!
//! 安全のための方針:
//! - 取得先は、ブラウザに渡す前に SSRF ポリシーで検査する (IP リテラルと、名前解決後のアドレス)。
//! - ページが読み込むサブリソースやリダイレクトは `Fetch` ドメインで 1 件ずつ止め、同じ検査を通ったものだけ続ける。
//! - 検査を通らない最終 URL の文書は、中身を読まずに失敗にする。
//! - タブは取得ごとに作り、成功・失敗・時間切れ・中断のいずれでも閉じる。
//! - 接続先 (トークンを含み得る) はエラーに載せない。
//!
//! 限界: ブラウザ自身の名前解決は再度行われる (DNS リバインディングは防げない)。ページの JavaScript が張る
//! WebSocket は `Fetch` では止められない。ブラウザは、内部ネットワークから切り離したコンテナで動かすこと。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, Method};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

use watchtower_core::pick::{parse_page_report, Inspection};
use watchtower_core::steps::validate_steps;
use watchtower_core::BrowserStep;

use crate::step_script;
use crate::{FetchError, FetchRequest, FetchResponse, Fetcher, SsrfPolicy};

/// 読み込み完了後、JavaScript の描画を待つ既定の時間。
const DEFAULT_SETTLE: Duration = Duration::from_millis(500);
/// 同時に開くタブ数の既定値。
const DEFAULT_MAX_TABS: usize = 2;
/// `wait_for` の既定の待ち時間と、確かめ直す間隔。
const DEFAULT_WAIT_FOR: Duration = Duration::from_secs(5);
const WAIT_FOR_POLL: Duration = Duration::from_millis(100);
/// ビジュアルセレクタで描画するときの画面の大きさ。
const INSPECT_VIEWPORT: (u32, u32) = (1280, 800);
/// スクリーンショットの大きさの上限 (デコード後)。
const MAX_SCREENSHOT_BYTES: usize = 15 * 1024 * 1024;
/// 後始末 (タブを閉じる) にかける時間。
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct CdpFetcher {
    ws_url: String,
    policy: Arc<SsrfPolicy>,
    tabs: Arc<Semaphore>,
    settle: Duration,
    screenshots: bool,
}

impl CdpFetcher {
    /// `ws_url` は `ws://host:9222/devtools/browser/<id>` の形。
    pub fn new(ws_url: String, policy: SsrfPolicy) -> Self {
        Self {
            ws_url,
            policy: Arc::new(policy),
            tabs: Arc::new(Semaphore::new(DEFAULT_MAX_TABS)),
            settle: DEFAULT_SETTLE,
            screenshots: false,
        }
    }

    /// 読み込み完了後に、描画を待つ時間。
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    pub fn with_screenshots(mut self, on: bool) -> Self {
        self.screenshots = on;
        self
    }

    /// 同時に開くタブの数。
    pub fn with_max_tabs(mut self, n: usize) -> Self {
        self.tabs = Arc::new(Semaphore::new(n.max(1)));
        self
    }
}

fn http_err(msg: &str) -> FetchError {
    FetchError::Http(msg.to_string())
}

/// URL をブラウザに触らせてよいか。スキームと、IP リテラル・名前解決後のアドレスで判定する。
async fn url_allowed(policy: &SsrfPolicy, url: &Url) -> Result<(), FetchError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::InvalidRequest(
            "only http(s) URLs are allowed".into(),
        ));
    }
    policy.check_url(url)?;
    if let Some(Host::Domain(name)) = url.host() {
        let port = url.port_or_known_default().unwrap_or(80);
        let addrs = tokio::net::lookup_host((name, port))
            .await
            .map_err(|_| http_err("could not resolve the host"))?;
        for a in addrs {
            if !policy.is_allowed(a.ip()) {
                return Err(FetchError::Blocked(a.ip().to_string()));
            }
        }
    }
    Ok(())
}

/// 1 回の取得の間の状態 (ブラウザが報告したこと)。
#[derive(Default)]
struct PageState {
    loaded: bool,
    status: Option<u16>,
    mime: Option<String>,
    final_url: Option<String>,
}

struct Session {
    ws: Ws,
    next_id: u64,
    policy: Arc<SsrfPolicy>,
    session_id: Option<String>,
    target_id: Option<String>,
    state: PageState,
    /// このホストを許可した/しなかった、の記録 (同じ取得の中で繰り返し名前解決しない)。
    decisions: HashMap<String, bool>,
}

impl Session {
    async fn send(&mut self, msg: Value) -> Result<(), FetchError> {
        self.ws
            .send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|_| http_err("the browser closed the connection"))
    }

    async fn read(&mut self) -> Result<Value, FetchError> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                        return Ok(v);
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    return Err(http_err("the browser closed the connection"));
                }
                Some(Ok(_)) => {}
            }
        }
    }

    /// コマンドを送り、その応答を待つ。待つあいだに届くイベントはその場で処理する
    /// (止めたリクエストを続けないと、ナビゲーションの応答が返らないため)。
    async fn call(
        &mut self,
        session: bool,
        method: &str,
        params: Value,
    ) -> Result<Value, FetchError> {
        let id = self.send_command(session, method, params).await?;
        loop {
            let m = self.read().await?;
            match m.get("id").and_then(Value::as_u64) {
                Some(got) if got == id => {
                    if m.get("error").is_some() {
                        return Err(http_err("the browser rejected a command"));
                    }
                    return Ok(m.get("result").cloned().unwrap_or(Value::Null));
                }
                Some(_) => {} // 応答を待たずに送ったコマンドの返事
                None => self.on_event(&m).await?,
            }
        }
    }

    async fn send_command(
        &mut self,
        session: bool,
        method: &str,
        params: Value,
    ) -> Result<u64, FetchError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if session {
            if let Some(s) = &self.session_id {
                msg["sessionId"] = json!(s);
            }
        }
        self.send(msg).await?;
        Ok(id)
    }

    async fn on_event(&mut self, m: &Value) -> Result<(), FetchError> {
        let params = &m["params"];
        match m["method"].as_str().unwrap_or_default() {
            "Fetch.requestPaused" => {
                let request_id = params["requestId"].as_str().unwrap_or_default().to_string();
                let url = params["request"]["url"].as_str().unwrap_or_default();
                if self.request_allowed(url).await {
                    self.send_command(
                        true,
                        "Fetch.continueRequest",
                        json!({"requestId": request_id}),
                    )
                    .await?;
                } else {
                    self.send_command(
                        true,
                        "Fetch.failRequest",
                        json!({"requestId": request_id, "errorReason": "BlockedByClient"}),
                    )
                    .await?;
                }
            }
            "Network.responseReceived" if params["type"] == "Document" => {
                let r = &params["response"];
                self.state.status = r["status"].as_u64().and_then(|s| u16::try_from(s).ok());
                self.state.mime = r["mimeType"].as_str().map(str::to_string);
                self.state.final_url = r["url"].as_str().map(str::to_string);
            }
            "Page.loadEventFired" => self.state.loaded = true,
            _ => {}
        }
        Ok(())
    }

    /// ページが読み込もうとしている URL を続けてよいか。
    async fn request_allowed(&mut self, raw: &str) -> bool {
        let Ok(url) = Url::parse(raw) else {
            return false;
        };
        // データ URL はネットワークに出ない
        if url.scheme() == "data" {
            return true;
        }
        let key = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
        if let Some(&known) = self.decisions.get(&key) {
            return known;
        }
        let ok = url_allowed(&self.policy, &url).await.is_ok();
        self.decisions.insert(key, ok);
        ok
    }

    /// 条件が満たされるまでイベントを処理し続ける。
    async fn pump_until(&mut self, done: impl Fn(&PageState) -> bool) -> Result<(), FetchError> {
        while !done(&self.state) {
            let m = self.read().await?;
            if m.get("id").is_none() {
                self.on_event(&m).await?;
            }
        }
        Ok(())
    }

    /// `d` のあいだ、届くイベントを処理する (描画中の追加リクエストも検査するため)。
    async fn pump_for(&mut self, d: Duration) -> Result<(), FetchError> {
        let deadline = Instant::now() + d;
        loop {
            match tokio::time::timeout_at(deadline, self.read()).await {
                Err(_) => return Ok(()),
                Ok(m) => {
                    let m = m?;
                    if m.get("id").is_none() {
                        self.on_event(&m).await?;
                    }
                }
            }
        }
    }

    /// タブを作ってページを開き、読み込み・操作・検査まで終える。(状態コード, 最終 URL) を返す。
    async fn load(
        &mut self,
        req: &FetchRequest,
        settle: Duration,
        viewport: Option<(u32, u32)>,
    ) -> Result<(u16, Url), FetchError> {
        let target = self
            .call(false, "Target.createTarget", json!({"url": "about:blank"}))
            .await?;
        self.target_id = target["targetId"].as_str().map(str::to_string);
        let target_id = self
            .target_id
            .clone()
            .ok_or_else(|| http_err("unexpected browser reply"))?;
        let attached = self
            .call(
                false,
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
            )
            .await?;
        self.session_id = attached["sessionId"].as_str().map(str::to_string);
        if self.session_id.is_none() {
            return Err(http_err("unexpected browser reply"));
        }

        self.call(true, "Page.enable", json!({})).await?;
        self.call(true, "Network.enable", json!({})).await?;
        if let Some((w, h)) = viewport {
            self.call(
                true,
                "Emulation.setDeviceMetricsOverride",
                json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": false}),
            )
            .await?;
        }
        if !req.headers.is_empty() {
            let headers: serde_json::Map<String, Value> = req
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            self.call(
                true,
                "Network.setExtraHTTPHeaders",
                json!({"headers": headers}),
            )
            .await?;
        }
        self.call(
            true,
            "Fetch.enable",
            json!({"patterns": [{"urlPattern": "*"}]}),
        )
        .await?;

        let nav = self
            .call(true, "Page.navigate", json!({"url": req.url.as_str()}))
            .await?;
        if nav.get("errorText").is_some() {
            return Err(http_err("navigation failed"));
        }
        self.pump_until(|s| s.loaded).await?;
        self.pump_for(settle).await?;
        self.run_steps(&req.browser_steps, settle).await?;

        // 最終的に着いた文書も、同じ基準で確かめる。通らなければ中身は読まない。
        let final_url = match &self.state.final_url {
            Some(u) => Url::parse(u).map_err(|_| http_err("unexpected browser reply"))?,
            None => req.url.clone(),
        };
        if final_url != req.url {
            url_allowed(&self.policy, &final_url).await?;
        }
        let status = self
            .state
            .status
            .ok_or_else(|| http_err("the browser reported no response"))?;
        let accepted = if req.accepted_statuses.is_empty() {
            (200..300).contains(&status)
        } else {
            req.accepted_statuses.contains(&status)
        };
        if !accepted {
            return Err(FetchError::Status(status));
        }
        Ok((status, final_url))
    }

    async fn run(
        &mut self,
        req: &FetchRequest,
        settle: Duration,
        screenshot: bool,
    ) -> Result<FetchResponse, FetchError> {
        let (status, final_url) = self.load(req, settle, None).await?;

        let evaluated = self
            .call(
                true,
                "Runtime.evaluate",
                json!({"expression": "document.documentElement.outerHTML", "returnByValue": true}),
            )
            .await?;
        let html = evaluated["result"]["value"]
            .as_str()
            .ok_or_else(|| http_err("unexpected browser reply"))?
            .to_string();
        if html.len() > req.max_body_bytes {
            return Err(FetchError::TooLarge {
                limit: req.max_body_bytes,
            });
        }

        let shot = if screenshot {
            let r = self
                .call(true, "Page.captureScreenshot", json!({"format": "png"}))
                .await?;
            let data = r["data"]
                .as_str()
                .ok_or_else(|| http_err("unexpected browser reply"))?;
            let png = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| http_err("unexpected browser reply"))?;
            Some(Bytes::from(png))
        } else {
            None
        };

        Ok(FetchResponse {
            status,
            headers: HeaderMap::new(),
            content_type: self.state.mime.clone(),
            body: html,
            raw: None,
            screenshot: shot,
            final_url,
        })
    }

    /// ページを開き、クリックできる要素の一覧と全体のスクリーンショットを取る。
    async fn run_inspect(
        &mut self,
        req: &FetchRequest,
        settle: Duration,
    ) -> Result<Inspection, FetchError> {
        self.load(req, settle, Some(INSPECT_VIEWPORT)).await?;
        let r = self
            .call(
                true,
                "Runtime.evaluate",
                json!({"expression": step_script::PICK_SCRIPT, "returnByValue": true}),
            )
            .await?;
        let (width, height, elements) = r["result"]["value"]
            .as_str()
            .and_then(parse_page_report)
            .ok_or_else(|| http_err("unexpected page report"))?;
        let shot = self
            .call(
                true,
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "captureBeyondViewport": true,
                    "clip": {"x": 0, "y": 0, "width": width, "height": height, "scale": 1},
                }),
            )
            .await?;
        let png = shot["data"]
            .as_str()
            .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
            .filter(|p| p.len() <= MAX_SCREENSHOT_BYTES)
            .ok_or_else(|| http_err("unexpected browser reply"))?;
        Ok(Inspection {
            width,
            height,
            screenshot: png,
            elements,
        })
    }

    /// 設定された操作を順に行う。失敗したら残りは行わない (エラーには入力文字列もセレクタも載せない)。
    async fn run_steps(
        &mut self,
        steps: &[BrowserStep],
        settle: Duration,
    ) -> Result<(), FetchError> {
        for (i, step) in steps.iter().enumerate() {
            let fail =
                |why: &str| http_err(&format!("step {} ({}) failed: {why}", i + 1, step.name()));
            match step {
                BrowserStep::Wait { ms } => self.pump_for(Duration::from_millis(*ms)).await?,
                BrowserStep::WaitFor { timeout_ms, .. } => {
                    let limit = timeout_ms.map_or(DEFAULT_WAIT_FOR, Duration::from_millis);
                    let deadline = Instant::now() + limit;
                    loop {
                        match self.eval_step(step).await? {
                            Some("ok") => break,
                            Some("missing") if Instant::now() < deadline => {}
                            Some("missing") => {
                                return Err(fail("the element did not appear in time"))
                            }
                            Some(_) => return Err(fail("invalid selector")),
                            None => return Err(fail("the page reported an error")),
                        }
                        self.pump_for(WAIT_FOR_POLL).await?;
                    }
                }
                _ => match self.eval_step(step).await? {
                    Some("ok") => self.pump_for(settle).await?,
                    Some("missing") => return Err(fail("element not found")),
                    Some(_) => return Err(fail("invalid selector")),
                    None => return Err(fail("the page reported an error")),
                },
            }
        }
        Ok(())
    }

    /// 1 つの操作を評価し、ページが返した結果 ("ok" / "missing" / "invalid") を返す。例外なら `None`。
    async fn eval_step(&mut self, step: &BrowserStep) -> Result<Option<&'static str>, FetchError> {
        let Some(expression) = step_script::expression(step) else {
            return Ok(Some("ok"));
        };
        let r = self
            .call(
                true,
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true}),
            )
            .await?;
        Ok(match r["result"]["value"].as_str() {
            Some("ok") => Some("ok"),
            Some("missing") => Some("missing"),
            Some(_) => Some("invalid"),
            None => None,
        })
    }

    /// タブを閉じて接続を終える。どの結果でも必ず呼ぶ (失敗しても無視する)。
    async fn cleanup(&mut self) {
        let Some(target_id) = self.target_id.take() else {
            let _ = self.ws.close(None).await;
            return;
        };
        let _ = tokio::time::timeout(CLEANUP_TIMEOUT, async {
            let _ = self
                .call(false, "Target.closeTarget", json!({"targetId": target_id}))
                .await;
            let _ = self.ws.close(None).await;
        })
        .await;
    }
}

enum Job {
    Fetch,
    Inspect,
}

enum Done {
    Page(Box<FetchResponse>),
    Inspected(Inspection),
}

impl CdpFetcher {
    /// 取得先と操作を検査してから、タブを 1 つ使って `job` を行う。どの結果でもタブは閉じる。
    async fn drive(
        &self,
        req: &FetchRequest,
        cancel: CancellationToken,
        job: Job,
    ) -> Result<Done, FetchError> {
        if req.method != Method::GET {
            return Err(FetchError::InvalidRequest(
                "the browser fetcher only supports GET".into(),
            ));
        }
        validate_steps(&req.browser_steps)
            .map_err(|e| FetchError::InvalidRequest(format!("browser steps: {e}")))?;
        // 取得先は、ブラウザに渡す前に検査する (接続もしない)
        url_allowed(&self.policy, &req.url).await?;

        let overall = async {
            let _permit = self
                .tabs
                .acquire()
                .await
                .map_err(|_| http_err("the browser is shutting down"))?;
            let (ws, _) = tokio_tungstenite::connect_async(self.ws_url.as_str())
                .await
                .map_err(|_| http_err("the browser is not reachable"))?;
            let mut session = Session {
                ws,
                next_id: 1,
                policy: Arc::clone(&self.policy),
                session_id: None,
                target_id: None,
                state: PageState::default(),
                decisions: HashMap::new(),
            };
            let work = async {
                match job {
                    Job::Fetch => session
                        .run(req, self.settle, self.screenshots)
                        .await
                        .map(|r| Done::Page(Box::new(r))),
                    Job::Inspect => session
                        .run_inspect(req, self.settle)
                        .await
                        .map(Done::Inspected),
                }
            };
            let result = tokio::select! {
                () = cancel.cancelled() => Err(FetchError::Cancelled),
                r = tokio::time::timeout(req.timeout, work) => r.unwrap_or(Err(FetchError::Timeout)),
            };
            session.cleanup().await;
            result
        };
        // 接続の確立や空きタブ待ちも、全体の制限時間に含める
        tokio::time::timeout(req.timeout + CLEANUP_TIMEOUT, overall)
            .await
            .unwrap_or(Err(FetchError::Timeout))
    }

    /// ビジュアルセレクタ用: 描画したページの全体のスクリーンショットと、選べる要素の一覧。
    pub async fn inspect(
        &self,
        req: &FetchRequest,
        cancel: CancellationToken,
    ) -> Result<Inspection, FetchError> {
        match self.drive(req, cancel, Job::Inspect).await? {
            Done::Inspected(i) => Ok(i),
            Done::Page(_) => Err(http_err("unexpected browser reply")),
        }
    }
}

#[async_trait]
impl Fetcher for CdpFetcher {
    async fn fetch(
        &self,
        req: &FetchRequest,
        cancel: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        match self.drive(req, cancel, Job::Fetch).await? {
            Done::Page(p) => Ok(*p),
            Done::Inspected(_) => Err(http_err("unexpected browser reply")),
        }
    }
}
