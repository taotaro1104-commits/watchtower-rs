use std::time::{Duration, Instant};

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::{process, ProcessError};

fn cfg() -> EffectiveConfig {
    EffectiveConfig::resolve(&ConfigLayer::default(), &[], &ConfigLayer::default())
}

fn nested(tag: &str, depth: usize) -> String {
    format!(
        "<html><body>{}x{}</body></html>",
        format!("<{tag}>").repeat(depth),
        format!("</{tag}>").repeat(depth)
    )
}

#[test]
fn pathologically_nested_html_is_rejected_quickly() {
    for tag in ["div", "span", "table", "b"] {
        let started = Instant::now();
        let res = process(&nested(tag, 50_000), Some("text/html"), &cfg());
        assert!(
            matches!(res, Err(ProcessError::TooDeep { .. })),
            "{tag}: {res:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{tag} took {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn realistic_nesting_is_accepted() {
    let res = process(&nested("div", 100), Some("text/html"), &cfg()).unwrap();
    assert!(res.text.contains('x'));
}

#[test]
fn many_unclosed_optional_end_tags_are_not_mistaken_for_depth() {
    let body = format!(
        "<html><body><ul>{}</ul>{}</body></html>",
        "<li>item".repeat(3000),
        "<p>para".repeat(3000)
    );
    assert!(process(&body, Some("text/html"), &cfg()).is_ok());
}

#[test]
fn void_and_self_closing_tags_do_not_count() {
    let body = format!(
        "<body>{}</body>",
        "<br><img src=a><input><hr/><meta charset=utf-8>".repeat(2000)
    );
    assert!(process(&body, Some("text/html"), &cfg()).is_ok());
}
