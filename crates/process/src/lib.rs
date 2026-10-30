//! 抽出 → テキスト化 → 正規化 → 判定 → 差分。I/O を持たない純粋関数群。

mod diff;
mod error;
mod extract;
mod guard;
mod kind;
mod normalize;

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
    let triggered = normalize::is_triggered(&text, &cfg.trigger_text);
    let hash = *blake3::hash(text.as_bytes()).as_bytes();
    Ok(Processed {
        text,
        hash,
        triggered,
    })
}
