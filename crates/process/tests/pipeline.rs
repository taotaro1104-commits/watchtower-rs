use watchtower_core::ConfigLayer;
use watchtower_core::EffectiveConfig;
use watchtower_process::{diff, process, ContentKind};

fn cfg(f: impl FnOnce(&mut ConfigLayer)) -> EffectiveConfig {
    let mut l = ConfigLayer::default();
    f(&mut l);
    EffectiveConfig::resolve(&l, &[], &ConfigLayer::default())
}

const HTML: &str = r#"<html><body>
<div id="ad">BUY NOW</div>
<div class="price">  Price:   $10  </div>
<p>Updated: 12:00:01</p>
<p>Hello</p></body></html>"#;

#[test]
fn html_to_normalized_text() {
    let out = process(HTML, Some("text/html"), &cfg(|_| {})).unwrap();
    assert!(out.text.contains("Price: $10"));
    assert!(out.text.contains("Hello"));
}

#[test]
fn exclude_filter_removes_elements() {
    let out = process(
        HTML,
        Some("text/html"),
        &cfg(|l| l.exclude_filters = vec!["#ad".into()]),
    )
    .unwrap();
    assert!(!out.text.contains("BUY NOW"));
}

#[test]
fn include_css_filter_extracts_only_matches() {
    let out = process(
        HTML,
        Some("text/html"),
        &cfg(|l| l.include_filters = vec![".price".into()]),
    )
    .unwrap();
    assert_eq!(out.text.trim(), "Price: $10");
}

#[test]
fn ignore_text_drops_matching_lines() {
    let out = process(
        HTML,
        Some("text/html"),
        &cfg(|l| l.ignore_text = vec!["Updated:".into()]),
    )
    .unwrap();
    assert!(!out.text.contains("Updated"));
    assert!(out.text.contains("Hello"));
}

#[test]
fn json_path_filter() {
    let body = r#"{"items":[{"p":1},{"p":2}],"x":"y"}"#;
    let out = process(
        body,
        Some("application/json"),
        &cfg(|l| l.include_filters = vec!["json:$.items[*].p".into()]),
    )
    .unwrap();
    assert_eq!(out.text.trim(), "1\n2");
}

#[test]
fn same_content_same_hash_and_trigger_detected() {
    let a = process(HTML, Some("text/html"), &cfg(|_| {})).unwrap();
    let b = process(HTML, Some("text/html"), &cfg(|_| {})).unwrap();
    assert_eq!(a.hash, b.hash);
    let t = process(
        HTML,
        Some("text/html"),
        &cfg(|l| l.trigger_text = vec!["Hello".into()]),
    )
    .unwrap();
    assert!(t.triggered);
    let n = process(
        HTML,
        Some("text/html"),
        &cfg(|l| l.trigger_text = vec!["nope".into()]),
    )
    .unwrap();
    assert!(!n.triggered);
}

#[test]
fn kind_detection_sniffs_body() {
    assert_eq!(ContentKind::detect(None, "{\"a\":1}"), ContentKind::Json);
    assert_eq!(
        ContentKind::detect(None, "<!doctype html><p>x"),
        ContentKind::Html
    );
    assert_eq!(
        ContentKind::detect(Some("text/plain"), "hi"),
        ContentKind::Text
    );
}

#[test]
fn diff_reports_added_and_removed_lines() {
    let d = diff("a\nb\nc\n", "a\nc\nd\n");
    assert_eq!(d.removed, vec!["b"]);
    assert_eq!(d.added, vec!["d"]);
    assert!(!d.is_empty());
    assert!(diff("x\n", "x\n").is_empty());
}
