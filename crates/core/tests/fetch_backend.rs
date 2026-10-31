//! 取得方法の名前 (本家互換) と `use_browser` の相互変換。

use watchtower_core::config::{fetch_backend_name, parse_fetch_backend};

#[test]
fn names_round_trip() {
    for (name, value) in [
        ("system", None),
        ("html_requests", Some(false)),
        ("html_webdriver", Some(true)),
    ] {
        assert_eq!(parse_fetch_backend(name), Some(value), "{name}");
        assert_eq!(fetch_backend_name(value), name);
    }
}

#[test]
fn an_empty_value_means_inherit() {
    assert_eq!(parse_fetch_backend(""), Some(None));
    assert_eq!(parse_fetch_backend("  "), Some(None));
}

#[test]
fn unknown_names_are_rejected() {
    for bad in [
        "chrome",
        "HTML_WEBDRIVER",
        "html_requests ; drop",
        "browser",
        "1",
    ] {
        assert_eq!(parse_fetch_backend(bad), None, "{bad}");
    }
}
