//! 取得にブラウザを使うか (`use_browser`) は Watch > Tag > Global の順で最初の指定を使う。既定は使わない。

use watchtower_core::{ConfigLayer, EffectiveConfig};

fn layer(v: Option<bool>) -> ConfigLayer {
    ConfigLayer {
        use_browser: v,
        ..Default::default()
    }
}

#[test]
fn the_default_is_plain_http() {
    assert!(!EffectiveConfig::resolve(&layer(None), &[layer(None)], &layer(None)).use_browser);
}

#[test]
fn the_most_specific_explicit_choice_wins_even_when_it_is_false() {
    let e = EffectiveConfig::resolve(
        &layer(Some(false)),
        &[layer(Some(true))],
        &layer(Some(true)),
    );
    assert!(!e.use_browser, "the watch opts out");
    let e = EffectiveConfig::resolve(&layer(None), &[layer(Some(true))], &layer(Some(false)));
    assert!(e.use_browser, "the tag beats the global setting");
    let e = EffectiveConfig::resolve(&layer(None), &[layer(None)], &layer(Some(true)));
    assert!(e.use_browser);
}

#[test]
fn old_stored_settings_without_the_field_still_load() {
    let old: ConfigLayer = serde_json::from_str(r#"{"check_interval_secs": 60}"#).unwrap();
    assert_eq!(old.use_browser, None);
    let json = serde_json::to_string(&layer(Some(true))).unwrap();
    assert_eq!(
        serde_json::from_str::<ConfigLayer>(&json)
            .unwrap()
            .use_browser,
        Some(true)
    );
}
