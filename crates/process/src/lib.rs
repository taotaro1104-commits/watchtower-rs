//! 抽出 → テキスト化 → 正規化 → 判定 → 差分。I/O を持たない純粋関数群。

mod diff;
mod error;
mod extract;
mod guard;
mod jq;
mod kind;
mod normalize;
pub mod product;
mod xpath;

pub use diff::{diff, DiffResult};
pub use error::ProcessError;
pub use kind::ContentKind;

use watchtower_core::EffectiveConfig;

/// パイプラインの出力。`hash` が前回と同じなら変化なし。
#[derive(Debug, Clone, PartialEq)]
pub struct Processed {
    pub text: String,
    pub hash: [u8; 32],
    pub triggered: bool,
}

/// 構造化データの価格が閾値以下か。本文の抽出方法とは独立に、ページ全体から読む。
fn price_is_below(body: &str, threshold: Option<f64>) -> bool {
    let Some(limit) = threshold else {
        return false;
    };
    let doc = scraper::Html::parse_document(body);
    product::extract(&doc).price.is_some_and(|p| p <= limit)
}

pub fn process(
    body: &str,
    content_type: Option<&str>,
    cfg: &EffectiveConfig,
) -> Result<Processed, ProcessError> {
    let kind = ContentKind::detect(content_type, body);
    let raw = match kind {
        ContentKind::Html => extract::html_to_text(body, cfg)?,
        ContentKind::Json => extract::json_to_text(body, cfg)?,
        ContentKind::Text => body.to_string(),
    };
    let text = normalize::normalize(&raw, &cfg.ignore_text);
    let triggered = normalize::is_triggered(&text, &cfg.trigger_text)
        || (kind == ContentKind::Html && price_is_below(body, cfg.price_below));
    let hash = *blake3::hash(text.as_bytes()).as_bytes();
    Ok(Processed {
        text,
        hash,
        triggered,
    })
}
