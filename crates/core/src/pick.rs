//! ビジュアルセレクタ: ブラウザで描画したページのスクリーンショットと、クリックできる要素の一覧。
//!
//! 要素の情報はページの JavaScript が作るので信用しない。`PickElement::sanitize` を通ったものだけ使う。

use serde::Deserialize;

/// 一覧に載せる要素の上限と、各項目の大きさの上限。
pub const MAX_ELEMENTS: usize = 400;
pub const MAX_SELECTOR_CHARS: usize = 500;
pub const MAX_TEXT_CHARS: usize = 60;
/// 描画するページの最大の大きさ (px)。
pub const MAX_PAGE_WIDTH: u32 = 1920;
pub const MAX_PAGE_HEIGHT: u32 = 6000;

#[derive(Debug, Clone, PartialEq)]
pub struct PickElement {
    pub selector: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
}

#[derive(Clone)]
pub struct Inspection {
    pub width: u32,
    pub height: u32,
    /// PNG。
    pub screenshot: Vec<u8>,
    pub elements: Vec<PickElement>,
}

impl std::fmt::Debug for Inspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Inspection({}x{}, {} elements)",
            self.width,
            self.height,
            self.elements.len()
        )
    }
}

#[derive(Deserialize)]
struct RawPage {
    w: f64,
    h: f64,
    #[serde(default)]
    els: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawElement {
    s: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    #[serde(default)]
    t: String,
}

fn px(v: f64, max: u32) -> Option<u32> {
    (v.is_finite() && v >= 0.0).then(|| (v.round() as u64).min(u64::from(max)) as u32)
}

/// ページが返した JSON を検証して、ページの大きさと使える要素だけにする。
/// 形式が違えば `None`。要素は、不正なものを捨て、上限までで打ち切る。
pub fn parse_page_report(json: &str) -> Option<(u32, u32, Vec<PickElement>)> {
    let page: RawPage = serde_json::from_str(json).ok()?;
    let width = px(page.w, MAX_PAGE_WIDTH).filter(|w| *w > 0)?;
    let height = px(page.h, MAX_PAGE_HEIGHT).filter(|h| *h > 0)?;
    let elements = page
        .els
        .into_iter()
        // 1 つの要素の形が崩れていても、全体を捨てずにその要素だけ捨てる
        .filter_map(|v| serde_json::from_value::<RawElement>(v).ok())
        .filter_map(|e| sanitize(e, width, height))
        .take(MAX_ELEMENTS)
        .collect();
    Some((width, height, elements))
}

fn sanitize(e: RawElement, page_w: u32, page_h: u32) -> Option<PickElement> {
    if e.s.trim().is_empty()
        || e.s.chars().count() > MAX_SELECTOR_CHARS
        || e.s.chars().any(char::is_control)
    {
        return None;
    }
    let x = px(e.x, page_w)?;
    let y = px(e.y, page_h)?;
    // ページの外にはみ出す分は切り詰め、見えない大きさのものは捨てる
    let width = px(e.w, page_w)?.min(page_w - x);
    let height = px(e.h, page_h)?.min(page_h - y);
    if width < 2 || height < 2 {
        return None;
    }
    let text: String =
        e.t.chars()
            .filter(|c| !c.is_control())
            .take(MAX_TEXT_CHARS)
            .collect();
    Some(PickElement {
        selector: e.s,
        x,
        y,
        width,
        height,
        text,
    })
}
