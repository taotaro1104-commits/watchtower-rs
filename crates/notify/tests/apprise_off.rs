//! Apprise への転送が無効 (既定) のとき、未知のスキームは保存時に拒否される。

use watchtower_notify::{NotifyError, NotifyTarget};

#[test]
fn unknown_schemes_are_rejected_by_default() {
    assert!(matches!(
        NotifyTarget::parse("pbul://o.SECRETtoken"),
        Err(NotifyError::UnsupportedScheme(s)) if s == "pbul"
    ));
}

#[test]
fn the_error_never_contains_the_credentials() {
    let err = NotifyTarget::parse("pbul://o.SECRETtoken").unwrap_err();
    assert!(!err.to_string().contains("SECRET"), "{err}");
}
