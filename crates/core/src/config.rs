use serde::{Deserialize, Serialize};

use crate::steps::BrowserStep;

/// 通知テンプレート (件名・本文) の長さの上限 (文字数)。
pub const MAX_TEMPLATE_CHARS: usize = 10_000;

/// Watch / Tag / Global の各層が持つ設定。未指定は `None` / 空。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigLayer {
    pub check_interval_secs: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub max_body_bytes: Option<usize>,
    pub include_filters: Vec<String>,
    pub exclude_filters: Vec<String>,
    pub ignore_text: Vec<String>,
    pub trigger_text: Vec<String>,
    pub notify_urls: Vec<String>,
    pub notify_muted: Option<bool>,
    /// この価格以下になったら通知する (商品ページの構造化データから読む)。
    pub price_below: Option<f64>,
    /// 取得にブラウザ (JavaScript の描画) を使う。未指定は通常の HTTP。
    pub use_browser: Option<bool>,
    /// ブラウザ取得でページを読み込んだ後に行う操作。最も近い層の指定がそのまま使われる (連結しない)。
    pub browser_steps: Vec<BrowserStep>,
    /// 追加の HTTP ヘッダ (名前の大文字小文字は区別しない)。近い層が同名の遠い層を上書きする。
    pub headers: Vec<(String, String)>,
    /// HTTP メソッド。未指定は GET。
    pub method: Option<String>,
    /// リクエストのボディ。
    pub body: Option<String>,
    /// `http://host:port` 形式のプロキシ。
    pub proxy: Option<String>,
    /// 通知の件名・本文のテンプレート (minijinja)。未指定は既定の文面。
    pub notification_title: Option<String>,
    pub notification_body: Option<String>,
}

// ヘッダの値・ボディ・プロキシには認証情報が入り得るので、Debug には出さない。
impl std::fmt::Debug for ConfigLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigLayer")
            .field("check_interval_secs", &self.check_interval_secs)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("include_filters", &self.include_filters)
            .field("exclude_filters", &self.exclude_filters)
            .field("ignore_text", &self.ignore_text)
            .field("trigger_text", &self.trigger_text)
            .field("notify_urls", &self.notify_urls.len())
            .field("notify_muted", &self.notify_muted)
            .field("price_below", &self.price_below)
            .field("use_browser", &self.use_browser)
            .field("browser_steps", &self.browser_steps)
            .field(
                "headers",
                &self.headers.iter().map(|h| &h.0).collect::<Vec<_>>(),
            )
            .field("method", &self.method)
            .field("body", &self.body.as_ref().map(|_| "<hidden>"))
            .field("proxy", &self.proxy.as_ref().map(|_| "<hidden>"))
            .field("notification_title", &self.notification_title)
            .field("notification_body", &self.notification_body)
            .finish()
    }
}

/// 取得方法の名前 (本家 changedetection.io の `fetch_backend` と同じ)。
/// `None` は「継承する (system)」、`Some(false)` は通常の HTTP、`Some(true)` はブラウザ。
pub fn fetch_backend_name(use_browser: Option<bool>) -> &'static str {
    match use_browser {
        None => "system",
        Some(false) => "html_requests",
        Some(true) => "html_webdriver",
    }
}

/// `fetch_backend_name` の逆。空は「継承」。未知の名前は `None` (= 不正)。
pub fn parse_fetch_backend(name: &str) -> Option<Option<bool>> {
    match name.trim() {
        "" | "system" => Some(None),
        "html_requests" => Some(Some(false)),
        "html_webdriver" => Some(Some(true)),
        _ => None,
    }
}
