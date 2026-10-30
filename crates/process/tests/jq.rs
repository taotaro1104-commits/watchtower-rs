//! jq による JSON の抽出 (`jq:` 接頭辞)。

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::{process, ProcessError};

fn run(json: &str, filters: &[&str]) -> Result<String, ProcessError> {
    let l = ConfigLayer {
        include_filters: filters.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    let cfg = EffectiveConfig::resolve(&l, &[], &ConfigLayer::default());
    process(json, Some("application/json"), &cfg).map(|p| p.text)
}

const DATA: &str = r#"{
  "store": {"name": "Shop", "open": true},
  "items": [
    {"sku": "a1", "price": 10.5, "tags": ["new"]},
    {"sku": "b2", "price": 25, "tags": []},
    {"sku": "c3", "price": 7, "tags": ["sale", "hot"]}
  ]
}"#;

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[test]
fn simple_paths_and_iteration() {
    assert_eq!(lines(&run(DATA, &["jq:.store.name"]).unwrap()), ["Shop"]);
    assert_eq!(
        lines(&run(DATA, &["jq:.items[].sku"]).unwrap()),
        ["a1", "b2", "c3"]
    );
}

#[test]
fn select_map_and_arithmetic() {
    let t = run(DATA, &["jq:.items[] | select(.price > 8) | .sku"]).unwrap();
    assert_eq!(lines(&t), ["a1", "b2"]);
    let t = run(DATA, &["jq:[.items[].price] | add"]).unwrap();
    assert_eq!(t.trim(), "42.5");
    let t = run(DATA, &["jq:.items | map(.sku) | join(\",\")"]).unwrap();
    assert_eq!(t.trim(), "a1,b2,c3");
}

#[test]
fn non_string_results_are_rendered_as_compact_json() {
    let t = run(DATA, &["jq:.items[0]"]).unwrap();
    assert_eq!(t.trim(), r#"{"sku":"a1","price":10.5,"tags":["new"]}"#);
    assert_eq!(run(DATA, &["jq:.store.open"]).unwrap().trim(), "true");
    assert_eq!(run(DATA, &["jq:.missing"]).unwrap().trim(), "null");
}

#[test]
fn the_filters_can_be_mixed_with_json_paths() {
    let t = run(DATA, &["json:$.store.name", "jq:.items[1].sku"]).unwrap();
    assert_eq!(lines(&t), ["Shop", "b2"]);
}

#[test]
fn invalid_programs_are_reported_with_the_right_error() {
    for bad in ["jq:.items[", "jq:|||", "jq:", "jq:undefined_function(1)"] {
        assert!(
            matches!(run(DATA, &[bad]), Err(ProcessError::InvalidJq(_))),
            "{bad}"
        );
    }
}

#[test]
fn runtime_errors_are_reported() {
    // 数値に対する .foo はエラー
    let err = run(DATA, &["jq:.items[0].price.foo"]).unwrap_err();
    assert!(matches!(err, ProcessError::InvalidJq(_)), "{err:?}");
}

#[test]
fn programs_that_could_run_forever_or_leak_the_environment_are_refused() {
    for bad in [
        "jq:until(false; .)",
        "jq:while(true; .)",
        "jq:repeat(.)",
        "jq:env",
        "jq:$ENV",
        "jq:input",
        "jq:inputs",
        "jq:halt",
        "jq:halt_error",
    ] {
        let r = run(DATA, &[bad]);
        assert!(
            matches!(r, Err(ProcessError::InvalidJq(ref m)) if m.contains("not allowed")),
            "{bad}: {r:?}"
        );
    }
    // 似た名前の別物 (キー名・文字列) は止めない
    assert!(run(r#"{"envelope": "until"}"#, &["jq:.envelope"]).is_ok());
}

#[test]
fn infinite_generators_are_cut_off_by_the_output_cap() {
    let t = run(DATA, &["jq:range(0; 1000000000)"]).unwrap();
    assert!(t.lines().count() <= 10_000, "{}", t.lines().count());
}

#[test]
fn deep_recursion_over_the_data_is_fine() {
    let t = run(DATA, &["jq:[..|numbers] | length"]).unwrap();
    assert_eq!(t.trim(), "3");
}

#[test]
fn invalid_json_input_is_still_an_invalid_json_error() {
    let err = run("{not json", &["jq:."]).unwrap_err();
    assert!(matches!(err, ProcessError::InvalidJson(_)), "{err:?}");
}
