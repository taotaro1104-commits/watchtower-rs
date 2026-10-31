//! ブラウザ操作ステップ (click / type / wait_for / wait) の検証・テキスト表現・設定への反映。

use watchtower_core::steps::{
    parse_steps_text, steps_to_text, validate_steps, BrowserStep, MAX_STEPS,
};
use watchtower_core::{ConfigLayer, EffectiveConfig};

fn click(s: &str) -> BrowserStep {
    BrowserStep::Click { selector: s.into() }
}

#[test]
fn the_text_form_round_trips() {
    let text = "click #accept\ntype input[name=q] = hello world\nwait_for .result\nwait_for .slow @3000\nwait 500";
    let steps = parse_steps_text(text).unwrap();
    assert_eq!(
        steps,
        vec![
            click("#accept"),
            BrowserStep::Type {
                selector: "input[name=q]".into(),
                text: "hello world".into()
            },
            BrowserStep::WaitFor {
                selector: ".result".into(),
                timeout_ms: None
            },
            BrowserStep::WaitFor {
                selector: ".slow".into(),
                timeout_ms: Some(3000)
            },
            BrowserStep::Wait { ms: 500 },
        ]
    );
    assert_eq!(steps_to_text(&steps), text);
}

#[test]
fn blank_lines_are_ignored_and_empty_text_means_no_steps() {
    assert!(parse_steps_text("").unwrap().is_empty());
    assert!(parse_steps_text("  \n\n").unwrap().is_empty());
    assert_eq!(parse_steps_text("\nclick a\n\n").unwrap().len(), 1);
}

#[test]
fn malformed_lines_are_rejected_with_the_line_number() {
    for (bad, line) in [
        ("jump #a", 1),
        ("click", 1),
        ("click #a\ntype #q", 2),
        ("type = text", 1),
        ("wait abc", 1),
        ("wait -1", 1),
        ("wait 999999", 1),
        ("wait_for .a @x", 1),
        ("click #a\n\nwait_for", 3),
    ] {
        let err = parse_steps_text(bad).unwrap_err();
        assert!(err.contains(&format!("line {line}")), "{bad:?} -> {err}");
    }
}

#[test]
fn errors_never_echo_typed_text() {
    let err = parse_steps_text("type = hunter2-SECRET").unwrap_err();
    assert!(!err.contains("SECRET"), "{err}");
    let long = format!("type #p = {}", "x".repeat(5000));
    let err = parse_steps_text(&long).unwrap_err();
    assert!(!err.contains("xxxx"), "{err}");
}

#[test]
fn limits_are_enforced() {
    let many = vec![click("a"); MAX_STEPS + 1];
    assert!(validate_steps(&many).is_err());
    assert!(validate_steps(&many[..MAX_STEPS]).is_ok());
    assert!(validate_steps(&[click(&"a".repeat(501))]).is_err());
    assert!(validate_steps(&[click("")]).is_err());
    assert!(validate_steps(&[click("a\nb")]).is_err());
    assert!(validate_steps(&[BrowserStep::Wait { ms: 30_001 }]).is_err());
    assert!(validate_steps(&[BrowserStep::WaitFor {
        selector: "a".into(),
        timeout_ms: Some(0)
    }])
    .is_err());
    assert!(validate_steps(&[BrowserStep::Type {
        selector: "a".into(),
        text: "あ".repeat(1001)
    }])
    .is_err());
}

#[test]
fn debug_output_hides_the_typed_text() {
    let step = BrowserStep::Type {
        selector: "#password".into(),
        text: "hunter2-SECRET".into(),
    };
    assert!(!format!("{step:?}").contains("SECRET"));
    assert!(!format!("{:?}", vec![step]).contains("SECRET"));
}

#[test]
fn json_form_is_tagged_and_rejects_unknown_fields() {
    let s: Vec<BrowserStep> =
        serde_json::from_str(r##"[{"op":"click","selector":"#a"},{"op":"wait","ms":10}]"##)
            .unwrap();
    assert_eq!(s, vec![click("#a"), BrowserStep::Wait { ms: 10 }]);
    assert!(serde_json::from_str::<Vec<BrowserStep>>(r##"[{"op":"eval","code":"1"}]"##).is_err());
    assert!(
        serde_json::from_str::<Vec<BrowserStep>>(r##"[{"op":"click","selector":"a","x":1}]"##)
            .is_err()
    );
}

#[test]
fn the_closest_layer_with_steps_wins_and_they_are_not_merged() {
    let w = ConfigLayer {
        browser_steps: vec![click("#w")],
        ..Default::default()
    };
    let t = ConfigLayer {
        browser_steps: vec![click("#t1"), click("#t2")],
        ..Default::default()
    };
    let g = ConfigLayer {
        browser_steps: vec![click("#g")],
        ..Default::default()
    };
    let none = ConfigLayer::default();
    assert_eq!(
        EffectiveConfig::resolve(&w, std::slice::from_ref(&t), &g).browser_steps,
        vec![click("#w")]
    );
    assert_eq!(
        EffectiveConfig::resolve(&none, &[t], &g).browser_steps,
        vec![click("#t1"), click("#t2")]
    );
    assert_eq!(
        EffectiveConfig::resolve(&none, &[], &g).browser_steps,
        vec![click("#g")]
    );
    assert!(EffectiveConfig::resolve(&none, &[], &none)
        .browser_steps
        .is_empty());
}

#[test]
fn old_stored_configs_without_steps_still_load() {
    let layer: ConfigLayer = serde_json::from_str(r#"{"use_browser":true}"#).unwrap();
    assert!(layer.browser_steps.is_empty());
}
