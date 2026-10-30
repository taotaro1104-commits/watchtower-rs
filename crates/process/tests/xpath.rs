//! XPath 1.0 による抽出 (`xpath:` 接頭辞)。HTML は html5ever で解釈してから評価する。

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::{process, ProcessError};

fn cfg(f: impl FnOnce(&mut ConfigLayer)) -> EffectiveConfig {
    let mut l = ConfigLayer::default();
    f(&mut l);
    EffectiveConfig::resolve(&l, &[], &ConfigLayer::default())
}

fn run(html: &str, filters: &[&str]) -> Result<String, ProcessError> {
    let filters: Vec<String> = filters.iter().map(|s| (*s).to_string()).collect();
    process(
        html,
        Some("text/html"),
        &cfg(|l| l.include_filters = filters),
    )
    .map(|p| p.text)
}

const SHOP: &str = r#"<!doctype html><html><body>
<ul id="items">
  <li class="item" data-sku="a1"><span class="name">Widget</span> <b class="price">$10</b></li>
  <li class="item sale" data-sku="b2"><span class="name">Gadget</span> <b class="price">$25</b></li>
  <li class="item" data-sku="c3"><span class="name">Doohickey</span> <b class="price">$7</b></li>
</ul>
<p>Footer &amp; legal</p>
</body></html>"#;

#[test]
fn selects_elements_by_path_and_renders_them_as_text() {
    let t = run(SHOP, &["xpath://li[@data-sku='b2']"]).unwrap();
    assert!(t.contains("Gadget") && t.contains("$25"), "{t}");
    assert!(!t.contains("Widget"), "{t}");
}

#[test]
fn predicates_and_functions_work() {
    let t = run(SHOP, &["xpath://li[contains(@class,'sale')]//b"]).unwrap();
    assert!(t.contains("$25") && !t.contains("$10"), "{t}");
    let t = run(SHOP, &["xpath:(//li)[position() < 3]//span"]).unwrap();
    assert!(
        t.contains("Widget") && t.contains("Gadget") && !t.contains("Doohickey"),
        "{t}"
    );
}

#[test]
fn text_and_attribute_nodes_are_returned_as_values() {
    let t = run(SHOP, &["xpath://li/@data-sku"]).unwrap();
    assert_eq!(t.lines().collect::<Vec<_>>(), ["a1", "b2", "c3"], "{t:?}");
    let t = run(SHOP, &["xpath://b[@class='price']/text()"]).unwrap();
    assert_eq!(t.lines().collect::<Vec<_>>(), ["$10", "$25", "$7"], "{t:?}");
}

#[test]
fn scalar_results_are_rendered() {
    assert_eq!(run(SHOP, &["xpath:count(//li)"]).unwrap().trim(), "3");
    assert_eq!(
        run(SHOP, &["xpath:string(//li[1]/span)"]).unwrap().trim(),
        "Widget"
    );
    assert_eq!(
        run(SHOP, &["xpath:boolean(//li[@data-sku='zz'])"])
            .unwrap()
            .trim(),
        "false"
    );
}

#[test]
fn entities_in_the_page_are_decoded_before_matching() {
    assert!(run(SHOP, &["xpath://p"])
        .unwrap()
        .contains("Footer & legal"));
}

#[test]
fn css_and_xpath_filters_can_be_mixed() {
    let t = run(SHOP, &[".sale .name", "xpath://li[@data-sku='c3']/span"]).unwrap();
    assert!(
        t.contains("Gadget") && t.contains("Doohickey") && !t.contains("Widget"),
        "{t}"
    );
}

#[test]
fn xpath_exclusions_remove_nodes_before_extraction() {
    let t = process(
        SHOP,
        Some("text/html"),
        &cfg(|l| l.exclude_filters = vec!["xpath://li[contains(@class,'sale')]".into()]),
    )
    .unwrap()
    .text;
    assert!(
        t.contains("Widget") && t.contains("Doohickey") && !t.contains("Gadget"),
        "{t}"
    );
}

#[test]
fn an_expression_matching_nothing_gives_empty_text() {
    assert_eq!(run(SHOP, &["xpath://table"]).unwrap().trim(), "");
}

#[test]
fn invalid_expressions_are_reported() {
    for bad in ["xpath://li[", "xpath:", "xpath:///", "xpath:unknown-fn()"] {
        assert!(
            matches!(run(SHOP, &[bad]), Err(ProcessError::InvalidXPath(_))),
            "{bad}"
        );
    }
}

#[test]
fn messy_html_does_not_break_evaluation() {
    // 閉じタグ省略・不正な属性名・制御文字・カスタム要素
    let messy = "<html><body><div @click='x' :class=y data-ok=1><p>one<p>two\u{0}\u{1}<my-el>c</my-el></div>";
    let t = run(messy, &["xpath://p"]).unwrap();
    assert!(t.contains("one") && t.contains("two"), "{t}");
    let t = run(messy, &["xpath://my-el/text()"]).unwrap();
    assert_eq!(t.trim(), "c");
}

#[test]
fn page_content_cannot_forge_the_internal_node_marker() {
    let evil = r#"<html><body><div wt-node-id="0">x</div><i>real</i></body></html>"#;
    let t = run(evil, &["xpath://i"]).unwrap();
    assert!(t.contains("real") && !t.contains('x'), "{t}");
}

#[test]
fn huge_result_sets_are_capped() {
    let many: String = (0..30_000).map(|i| format!("<li>{i}</li>")).collect();
    let html = format!("<html><body><ul>{many}</ul></body></html>");
    let t = run(&html, &["xpath://li/text()"]).unwrap();
    assert!(t.lines().count() <= 10_000, "{}", t.lines().count());
}

#[test]
fn xpath_filters_are_ignored_for_non_html_content() {
    let out = process(
        "{\"a\": 1}",
        Some("application/json"),
        &cfg(|l| l.include_filters = vec!["xpath://a".into()]),
    )
    .unwrap();
    assert!(out.text.contains("\"a\""), "{}", out.text);
}
