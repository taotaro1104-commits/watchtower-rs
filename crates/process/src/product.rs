//! 商品の価格・在庫の検知。JSON-LD → microdata → OpenGraph の順に探し、足りない項目は後ろで補う。

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::OnceLock;

/// 1 つの JSON-LD スクリプトとして読む大きさの上限。
const MAX_SCRIPT_BYTES: usize = 4 * 1024 * 1024;
/// JSON-LD の木をたどる深さと、調べるノード数の上限。
const MAX_DEPTH: usize = 10;
const MAX_NODES: usize = 50_000;
/// これを超える価格は誤検出として捨てる。
const MAX_PRICE: f64 = 1e12;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProductInfo {
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub availability: Option<&'static str>,
}

impl ProductInfo {
    /// 差分に使う、正規化したテキスト行。
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = self.price {
            out.push(format!("price: {p}"));
        }
        if let Some(c) = &self.currency {
            out.push(format!("currency: {c}"));
        }
        if let Some(a) = self.availability {
            out.push(format!("availability: {a}"));
        }
        out
    }

    fn fill_from(&mut self, other: ProductInfo) {
        self.price = self.price.or(other.price);
        if self.currency.is_none() {
            self.currency = other.currency;
        }
        self.availability = self.availability.or(other.availability);
    }
}

pub fn extract(doc: &Html) -> ProductInfo {
    let mut info = from_json_ld(doc);
    info.fill_from(from_microdata(doc));
    info.fill_from(from_open_graph(doc));
    info
}

// ---- 値の正規化 -------------------------------------------------------------

fn exponent_notation() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[0-9][eE][+-]?[0-9]").expect("static regex"))
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\u{202f}')
}

/// `$1,299.00` / `1.299,00 €` / `19,99` / `1,980円` などを数値にする。
pub fn parse_price(raw: &str) -> Option<f64> {
    if exponent_notation().is_match(raw) {
        return None;
    }
    let start = raw.find(|c: char| c.is_ascii_digit())?;
    if raw[..start].trim_end().ends_with('-') {
        return None;
    }
    let run: String = raw[start..]
        .chars()
        .take_while(|c| is_number_char(*c))
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\u{202f}')
        .collect();
    let run = run.trim_end_matches([',', '.']);
    let (commas, dots) = (run.matches(',').count(), run.matches('.').count());
    let normalised = match (run.rfind(','), run.rfind('.')) {
        (Some(c), Some(d)) => {
            // 後ろにある方が小数点
            let (decimal, thousands) = if c > d { (',', '.') } else { ('.', ',') };
            run.replace(thousands, "").replace(decimal, ".")
        }
        (Some(c), None) => {
            let after = run.len() - c - 1;
            if commas > 1 || after == 3 {
                run.replace(',', "")
            } else {
                run.replace(',', ".")
            }
        }
        (None, Some(_)) if dots > 1 => run.replace('.', ""),
        _ => run.to_string(),
    };
    normalised
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && (0.0..=MAX_PRICE).contains(p))
}

/// schema.org の URL や自由な文言を、固定の語に寄せる。
pub fn normalise_availability(raw: &str) -> Option<&'static str> {
    let key: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let has = |words: &[&str]| words.iter().any(|w| key.contains(w));
    if has(&["discontinued"]) {
        Some("discontinued")
    } else if has(&["outofstock", "soldout", "unavailable"]) {
        Some("out_of_stock")
    } else if has(&["preorder", "presale"]) {
        Some("pre_order")
    } else if has(&["backorder"]) {
        Some("back_order")
    } else if has(&["limitedavailability"]) {
        Some("limited")
    } else if has(&["instock", "available"]) {
        Some("in_stock")
    } else {
        None
    }
}

// ---- JSON-LD ----------------------------------------------------------------

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn is_product(v: &Value) -> bool {
    match v.get("@type") {
        Some(Value::String(t)) => t.eq_ignore_ascii_case("product"),
        Some(Value::Array(a)) => a.iter().any(|t| {
            t.as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case("product"))
        }),
        _ => false,
    }
}

fn collect_products<'a>(v: &'a Value, depth: usize, budget: &mut usize, out: &mut Vec<&'a Value>) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    *budget -= 1;
    match v {
        Value::Object(map) => {
            if is_product(v) {
                out.push(v);
            }
            for child in map.values() {
                collect_products(child, depth + 1, budget, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_products(child, depth + 1, budget, out);
            }
        }
        _ => {}
    }
}

fn offer_info(offer: &Value) -> ProductInfo {
    let price_of = |o: &Value| {
        ["price", "lowPrice"]
            .iter()
            .find_map(|k| o.get(*k).and_then(text_of))
            .and_then(|s| parse_price(&s))
    };
    let mut info = ProductInfo {
        price: price_of(offer),
        currency: offer.get("priceCurrency").and_then(text_of),
        availability: offer
            .get("availability")
            .and_then(text_of)
            .and_then(|a| normalise_availability(&a)),
    };
    // priceSpecification (オブジェクトまたは配列) に入っている場合
    let specs = match offer.get("priceSpecification") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    };
    for spec in specs {
        info.price = info.price.or_else(|| price_of(spec));
        if info.currency.is_none() {
            info.currency = spec.get("priceCurrency").and_then(text_of);
        }
    }
    info
}

fn from_json_ld(doc: &Html) -> ProductInfo {
    let Ok(sel) = Selector::parse("script[type]") else {
        return ProductInfo::default();
    };
    let mut info = ProductInfo::default();
    for script in doc.select(&sel) {
        let is_ld = script
            .value()
            .attr("type")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"));
        if !is_ld {
            continue;
        }
        let text: String = script.text().collect();
        if text.len() > MAX_SCRIPT_BYTES {
            continue;
        }
        // 壊れた JSON や深すぎる入れ子 (serde_json の再帰上限) は読み飛ばす
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let (mut products, mut budget) = (Vec::new(), MAX_NODES);
        collect_products(&value, 0, &mut budget, &mut products);
        for product in products {
            let offers = match product.get("offers") {
                Some(Value::Array(a)) => a.iter().collect(),
                Some(v @ Value::Object(_)) => vec![v],
                _ => Vec::new(),
            };
            for offer in offers {
                info.fill_from(offer_info(offer));
            }
        }
    }
    info
}

// ---- microdata / OpenGraph ----------------------------------------------------

fn first_value(doc: &Html, css: &str, attrs: &[&str], use_text: bool) -> Option<String> {
    let sel = Selector::parse(css).ok()?;
    doc.select(&sel).find_map(|e| {
        attrs
            .iter()
            .find_map(|a| e.value().attr(a).map(str::to_string))
            .or_else(|| use_text.then(|| e.text().collect::<String>().trim().to_string()))
            .filter(|s| !s.is_empty())
    })
}

fn from_microdata(doc: &Html) -> ProductInfo {
    ProductInfo {
        price: first_value(doc, "[itemprop=price]", &["content"], true)
            .and_then(|s| parse_price(&s)),
        currency: first_value(doc, "[itemprop=priceCurrency]", &["content"], true),
        availability: first_value(doc, "[itemprop=availability]", &["href", "content"], true)
            .and_then(|a| normalise_availability(&a)),
    }
}

fn meta(doc: &Html, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| first_value(doc, &format!("meta[property=\"{n}\"]"), &["content"], false))
}

fn from_open_graph(doc: &Html) -> ProductInfo {
    ProductInfo {
        price: meta(doc, &["product:price:amount", "og:price:amount"])
            .and_then(|s| parse_price(&s)),
        currency: meta(doc, &["product:price:currency", "og:price:currency"]),
        availability: meta(doc, &["product:availability", "og:availability"])
            .and_then(|a| normalise_availability(&a)),
    }
}
