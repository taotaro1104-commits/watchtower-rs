use serde::{Deserialize, Serialize};

/// Watch / Tag / Global の各層が持つ設定。未指定は `None` / 空。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
}
