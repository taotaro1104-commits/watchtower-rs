//! 価格・在庫の検知 (`product:` フィルタ)。JSON-LD / microdata / OpenGraph から取り出す。

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::{process, ProcessError};

fn cfg(f: impl FnOnce(&mut ConfigLayer)) -> EffectiveConfig {
    let mut l = ConfigLayer::default();
    f(&mut l);
    EffectiveConfig::resolve(&l, &[], &ConfigLayer::default())
}

fn product(html: &str) -> Result<String, ProcessError> {
    process(
        html,
        Some("text/html"),
        &cfg(|l| l.include_filters = vec!["product:".into()]),
    )
    .map(|p| p.text)
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn ld(json: &str) -> String {
    format!(
        r#"<html><head><script type="application/ld+json">{json}</script></head><body>Buy now</body></html>"#
    )
}

#[test]
fn json_ld_product_with_an_offer() {
    let html = ld(
        r#"{"@context":"https://schema.org","@type":"Product","name":"Widget",
        "offers":{"@type":"Offer","price":"19.99","priceCurrency":"USD",
        "availability":"https://schema.org/InStock"}}"#,
    );
    assert_eq!(
        lines(&product(&html).unwrap()),
        ["price: 19.99", "currency: USD", "availability: in_stock"]
    );
}

#[test]
fn json_ld_graph_arrays_and_numeric_prices() {
    let html = ld(
        r#"{"@graph":[{"@type":"WebSite"},{"@type":["Product","Thing"],
        "offers":[{"@type":"Offer","price":1299,"priceCurrency":"JPY",
        "availability":"http://schema.org/OutOfStock"}]}]}"#,
    );
    assert_eq!(
        lines(&product(&html).unwrap()),
        ["price: 1299", "currency: JPY", "availability: out_of_stock"]
    );
}

#[test]
fn aggregate_offers_use_the_low_price() {
    let html = ld(r#"{"@type":"Product","offers":{"@type":"AggregateOffer",
        "lowPrice":"5.50","highPrice":"9.00","priceCurrency":"EUR"}}"#);
    assert!(product(&html).unwrap().contains("price: 5.5"));
}

#[test]
fn price_specification_is_understood() {
    let html = ld(r#"{"@type":"Product","offers":{"@type":"Offer",
        "priceSpecification":{"price":"42.00","priceCurrency":"GBP"}}}"#);
    let t = product(&html).unwrap();
    assert!(
        t.contains("price: 42") && t.contains("currency: GBP"),
        "{t}"
    );
}

#[test]
fn microdata_is_read() {
    let html = r#"<html><body itemscope itemtype="https://schema.org/Product">
      <span itemprop="price" content="249.00">$249</span>
      <meta itemprop="priceCurrency" content="USD">
      <link itemprop="availability" href="https://schema.org/PreOrder">
    </body></html>"#;
    assert_eq!(
        lines(&product(html).unwrap()),
        ["price: 249", "currency: USD", "availability: pre_order"]
    );
}

#[test]
fn open_graph_is_the_fallback() {
    let html = r#"<html><head>
      <meta property="product:price:amount" content="1,980">
      <meta property="product:price:currency" content="JPY">
      <meta property="og:availability" content="out of stock">
    </head><body></body></html>"#;
    assert_eq!(
        lines(&product(html).unwrap()),
        ["price: 1980", "currency: JPY", "availability: out_of_stock"]
    );
}

#[test]
fn json_ld_wins_and_other_sources_fill_the_gaps() {
    let html = r#"<html><head>
      <script type="application/ld+json">{"@type":"Product","offers":{"price":"10.00"}}</script>
      <meta property="product:price:amount" content="99.00">
      <meta property="product:price:currency" content="USD">
    </head></html>"#;
    assert_eq!(
        lines(&product(html).unwrap()),
        ["price: 10", "currency: USD"]
    );
}

#[test]
fn price_strings_in_many_formats_are_normalised() {
    let cases = [
        ("$1,299.00", "1299"),
        ("1.299,00 €", "1299"),
        ("19,99", "19.99"),
        ("1,980円", "1980"),
        ("USD 5", "5"),
        ("  7.5  ", "7.5"),
        ("1 299,50", "1299.5"),
    ];
    for (raw, want) in cases {
        let html = format!(
            r#"<html><head><meta property="product:price:amount" content="{raw}"></head></html>"#
        );
        assert_eq!(
            product(&html).unwrap().trim(),
            format!("price: {want}"),
            "{raw}"
        );
    }
}

#[test]
fn unparseable_or_absurd_prices_are_ignored() {
    for raw in ["free", "", "-5", "1e99", "NaN", "999999999999999"] {
        let html = format!(
            r#"<html><head><meta property="product:price:amount" content="{raw}"></head></html>"#
        );
        assert!(!product(&html).unwrap().contains("price:"), "{raw}");
    }
}

#[test]
fn availability_texts_are_normalised() {
    let cases = [
        ("https://schema.org/InStock", "in_stock"),
        ("In Stock", "in_stock"),
        ("https://schema.org/OutOfStock", "out_of_stock"),
        ("Sold out", "out_of_stock"),
        ("Currently unavailable", "out_of_stock"),
        ("https://schema.org/BackOrder", "back_order"),
        ("https://schema.org/LimitedAvailability", "limited"),
        ("https://schema.org/Discontinued", "discontinued"),
    ];
    for (raw, want) in cases {
        let html = format!(
            r#"<html><head><meta property="product:availability" content="{raw}"></head></html>"#
        );
        assert_eq!(
            product(&html).unwrap().trim(),
            format!("availability: {want}"),
            "{raw}"
        );
    }
}

#[test]
fn pages_without_product_data_give_empty_text() {
    assert_eq!(
        product("<html><body>Hello</body></html>").unwrap().trim(),
        ""
    );
}

#[test]
fn broken_json_ld_is_skipped_not_fatal() {
    let html = format!(
        "{}{}",
        ld("{not json"),
        r#"<meta property="product:price:amount" content="3.00">"#
    );
    assert_eq!(product(&html).unwrap().trim(), "price: 3");
}

#[test]
fn a_price_change_changes_the_hash_and_text() {
    let a = process(
        &ld(r#"{"@type":"Product","offers":{"price":"10","priceCurrency":"USD"}}"#),
        Some("text/html"),
        &cfg(|l| l.include_filters = vec!["product:".into()]),
    )
    .unwrap();
    let b = process(
        &ld(r#"{"@type":"Product","offers":{"price":"9","priceCurrency":"USD"}}"#),
        Some("text/html"),
        &cfg(|l| l.include_filters = vec!["product:".into()]),
    )
    .unwrap();
    assert_ne!(a.hash, b.hash);
}

#[test]
fn product_can_be_combined_with_other_filters() {
    let html = r#"<html><head><meta property="product:price:amount" content="3.00"></head>
        <body><h1 class="t">Title</h1></body></html>"#;
    let t = process(
        html,
        Some("text/html"),
        &cfg(|l| l.include_filters = vec![".t".into(), "product:".into()]),
    )
    .unwrap()
    .text;
    assert!(t.contains("Title") && t.contains("price: 3"), "{t}");
}

#[test]
fn deeply_nested_or_huge_json_ld_does_not_blow_up() {
    let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
    let html = ld(&deep);
    assert!(product(&html).is_ok());
    let big = format!(
        r#"{{"@type":"Product","description":"{}","offers":{{"price":"2"}}}}"#,
        "x".repeat(2_000_000)
    );
    assert!(product(&ld(&big)).unwrap().contains("price: 2"));
}
