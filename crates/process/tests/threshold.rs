//! 価格の閾値: 指定した価格以下になったら `triggered`。

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::process;

fn page(price: &str) -> String {
    format!(
        r#"<html><head><script type="application/ld+json">
        {{"@type":"Product","offers":{{"price":"{price}","priceCurrency":"USD"}}}}
        </script></head><body>Gadget</body></html>"#
    )
}

fn cfg(price_below: Option<f64>, trigger: &[&str], include: &[&str]) -> EffectiveConfig {
    let l = ConfigLayer {
        price_below,
        trigger_text: trigger.iter().map(|s| (*s).to_string()).collect(),
        include_filters: include.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    EffectiveConfig::resolve(&l, &[], &ConfigLayer::default())
}

fn triggered(html: &str, c: &EffectiveConfig) -> bool {
    process(html, Some("text/html"), c).unwrap().triggered
}

#[test]
fn at_or_below_the_threshold_triggers() {
    let c = cfg(Some(20.0), &[], &["product:"]);
    assert!(triggered(&page("19.99"), &c));
    assert!(triggered(&page("20"), &c), "equal counts");
    assert!(!triggered(&page("20.01"), &c));
}

#[test]
fn the_threshold_works_even_without_the_product_filter() {
    // 本文を別の方法で抽出していても、ページの構造化データから価格を読む
    let c = cfg(Some(20.0), &[], &[]);
    assert!(triggered(&page("10"), &c));
    let c = cfg(Some(20.0), &[], &["body"]);
    assert!(triggered(&page("10"), &c));
}

#[test]
fn pages_without_a_price_never_trigger_on_price() {
    let c = cfg(Some(20.0), &[], &[]);
    assert!(!triggered("<html><body>No price here</body></html>", &c));
    assert!(!triggered("plain text", &c));
}

#[test]
fn without_a_threshold_prices_do_not_trigger() {
    assert!(!triggered(&page("1"), &cfg(None, &[], &["product:"])));
}

#[test]
fn text_triggers_and_the_threshold_are_alternatives() {
    let c = cfg(Some(5.0), &["gadget"], &[]);
    assert!(triggered(&page("100"), &c), "text matched");
    assert!(triggered(
        "<html><body>x</body></html>",
        &cfg(Some(5.0), &["x"], &[])
    ));
    let c = cfg(Some(500.0), &["nomatch"], &[]);
    assert!(triggered(&page("100"), &c), "price matched");
    let c = cfg(Some(5.0), &["nomatch"], &[]);
    assert!(!triggered(&page("100"), &c));
}

#[test]
fn zero_is_a_valid_threshold_for_free_items() {
    let c = cfg(Some(0.0), &[], &[]);
    assert!(triggered(&page("0"), &c));
    assert!(!triggered(&page("0.01"), &c));
}
