//! 未実装のサービスを Apprise API サーバへ転送するかどうか (プロセス全体の設定)。
//!
//! 保存時の検証 (`NotifyTarget::parse`) は Web・API・CLI・ワーカーなど多くの場所で呼ばれるため、
//! 起動時に一度だけ設定して、すべての場所で同じ判断になるようにする。

use std::sync::atomic::{AtomicBool, Ordering};

static ALLOWED: AtomicBool = AtomicBool::new(false);

/// true にすると、未知のスキームを「転送用の宛先」として受け付ける。
pub fn allow_foreign_targets(allow: bool) {
    ALLOWED.store(allow, Ordering::SeqCst);
}

pub(crate) fn foreign_allowed() -> bool {
    ALLOWED.load(Ordering::SeqCst)
}
