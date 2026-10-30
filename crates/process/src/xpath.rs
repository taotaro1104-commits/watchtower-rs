//! XPath 1.0 による抽出。
//!
//! html5ever で解釈した木を整形式の XML に直し、sxd-xpath で評価する。
//! 一致した要素は内部の識別属性で元の木のノードに戻し、HTML としてテキスト化できるようにする。

use ego_tree::NodeId;
use scraper::{Html, Node};
use std::fmt::Write as _;
use sxd_document::parser;
use sxd_xpath::nodeset::Node as XNode;
use sxd_xpath::{evaluate_xpath, Value};

use crate::ProcessError;

/// 元の木のノードを指すために XML 側へ付ける属性。ページ側の同名属性は取り除く。
const MARKER: &str = "wt-node-id";
/// 1 つの式から取り出す結果の上限。
pub const MAX_HITS: usize = 10_000;

/// 式の評価結果。
#[derive(Debug)]
pub enum Hit {
    /// 元の木の要素。
    Element(NodeId),
    /// 属性値・テキスト・数値・真偽値・文字列。
    Text(String),
}

fn valid_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn escape(out: &mut String, s: &str) {
    for c in s.chars().filter(|c| valid_char(*c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// 木を XML 文字列にする。`ids[n]` は識別属性 `n` に対応する元のノード。
fn to_xml(doc: &Html) -> (String, Vec<NodeId>) {
    let mut xml = String::new();
    let mut ids = Vec::new();
    for edge in doc.tree.root().traverse() {
        match edge {
            ego_tree::iter::Edge::Open(n) => match n.value() {
                Node::Element(el) => {
                    let name = if valid_name(el.name()) {
                        el.name()
                    } else {
                        "x-invalid"
                    };
                    let _ = write!(xml, "<{name} {MARKER}=\"{}\"", ids.len());
                    ids.push(n.id());
                    for (k, v) in el.attrs() {
                        // 名前空間宣言・接頭辞つき・不正な名前・識別属性の偽装は捨てる
                        if !valid_name(k) || k == MARKER || k.starts_with("xmlns") {
                            continue;
                        }
                        let _ = write!(xml, " {k}=\"");
                        escape(&mut xml, v);
                        xml.push('"');
                    }
                    xml.push('>');
                }
                Node::Text(t) => escape(&mut xml, &t.text),
                _ => {}
            },
            ego_tree::iter::Edge::Close(n) => {
                if let Node::Element(el) = n.value() {
                    let name = if valid_name(el.name()) {
                        el.name()
                    } else {
                        "x-invalid"
                    };
                    let _ = write!(xml, "</{name}>");
                }
            }
        }
    }
    (xml, ids)
}

fn number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// `expr` を評価する。式の誤り・未知の関数は `InvalidXPath`。
pub fn select(doc: &Html, expr: &str) -> Result<Vec<Hit>, ProcessError> {
    let invalid = |e: String| ProcessError::InvalidXPath(format!("{expr}: {e}"));
    if expr.trim().is_empty() {
        return Err(invalid("empty expression".into()));
    }
    let (xml, ids) = to_xml(doc);
    if xml.is_empty() {
        return Ok(Vec::new());
    }
    let package = parser::parse(&xml).map_err(|e| ProcessError::Text(format!("xml: {e:?}")))?;
    let document = package.as_document();
    let value = evaluate_xpath(&document, expr).map_err(|e| invalid(e.to_string()))?;

    Ok(match value {
        Value::Boolean(b) => vec![Hit::Text(b.to_string())],
        Value::Number(n) => vec![Hit::Text(number(n))],
        Value::String(s) => vec![Hit::Text(s)],
        Value::Nodeset(set) => set
            .document_order()
            .into_iter()
            .filter_map(|n| match n {
                XNode::Element(e) => e
                    .attribute_value(MARKER)
                    .and_then(|v| v.parse::<usize>().ok())
                    .and_then(|i| ids.get(i).copied())
                    .map(Hit::Element),
                XNode::Attribute(a) => Some(Hit::Text(a.value().to_string())),
                XNode::Text(t) => Some(Hit::Text(t.text().to_string())),
                _ => None,
            })
            .take(MAX_HITS)
            .collect(),
    })
}
