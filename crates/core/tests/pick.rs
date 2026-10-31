//! ページが返す要素の一覧は信用せず、検証してから使う。

use watchtower_core::pick::{parse_page_report, MAX_ELEMENTS, MAX_PAGE_WIDTH, MAX_TEXT_CHARS};

fn report(els: &str) -> String {
    format!(r#"{{"w":1280,"h":800,"els":{els}}}"#)
}

#[test]
fn a_well_formed_report_is_accepted() {
    let (w, h, els) = parse_page_report(&report(
        r##"[{"s":"#price","x":10,"y":20,"w":100,"h":30,"t":"¥1,980"}]"##,
    ))
    .unwrap();
    assert_eq!((w, h), (1280, 800));
    assert_eq!(els.len(), 1);
    assert_eq!(els[0].selector, "#price");
    assert_eq!(
        (els[0].x, els[0].y, els[0].width, els[0].height),
        (10, 20, 100, 30)
    );
    assert_eq!(els[0].text, "¥1,980");
}

#[test]
fn malformed_reports_are_refused() {
    for bad in [
        "",
        "null",
        "[]",
        r#"{"w":"x","h":1,"els":[]}"#,
        r#"{"w":0,"h":800,"els":[]}"#,
        r#"{"w":1280,"h":-5,"els":[]}"#,
        r#"{"h":800,"els":[]}"#,
    ] {
        assert!(parse_page_report(bad).is_none(), "{bad}");
    }
}

#[test]
fn the_page_size_is_clamped() {
    let (w, h, _) = parse_page_report(r#"{"w":99999,"h":99999999,"els":[]}"#).unwrap();
    assert_eq!(w, MAX_PAGE_WIDTH);
    assert!(h <= 6000);
}

#[test]
fn bad_elements_are_dropped_but_good_ones_stay() {
    let long = "a".repeat(501);
    let els = format!(
        r##"[
          {{"s":"","x":0,"y":0,"w":50,"h":50}},
          {{"s":"{long}","x":0,"y":0,"w":50,"h":50}},
          {{"s":"a\nb","x":0,"y":0,"w":50,"h":50}},
          {{"s":"#tiny","x":0,"y":0,"w":1,"h":1}},
          {{"s":"#neg","x":-5,"y":0,"w":50,"h":50}},
          {{"s":"#nan","x":0,"y":0,"w":"x","h":50}},
          {{"s":"#ok","x":0,"y":0,"w":50,"h":50}}
        ]"##
    );
    let (_, _, out) = parse_page_report(&report(&els)).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].selector, "#ok");
}

#[test]
fn boxes_are_cut_to_the_page() {
    let (_, _, out) = parse_page_report(&report(
        r##"[{"s":"#wide","x":1200,"y":790,"w":5000,"h":5000}]"##,
    ))
    .unwrap();
    assert_eq!(
        (out[0].x + out[0].width, out[0].y + out[0].height),
        (1280, 800)
    );
}

#[test]
fn the_number_of_elements_and_the_text_length_are_capped() {
    let many: Vec<String> = (0..MAX_ELEMENTS + 50)
        .map(|i| {
            format!(
                r##"{{"s":"#e{i}","x":0,"y":0,"w":10,"h":10,"t":"{}"}}"##,
                "あ".repeat(500)
            )
        })
        .collect();
    let (_, _, out) = parse_page_report(&report(&format!("[{}]", many.join(",")))).unwrap();
    assert_eq!(out.len(), MAX_ELEMENTS);
    assert!(out.iter().all(|e| e.text.chars().count() <= MAX_TEXT_CHARS));
}

#[test]
fn control_characters_are_removed_from_text() {
    let (_, _, out) = parse_page_report(&report(
        r##"[{"s":"#a","x":0,"y":0,"w":10,"h":10,"t":"a\u0000b\nc"}]"##,
    ))
    .unwrap();
    assert_eq!(out[0].text, "abc");
}
