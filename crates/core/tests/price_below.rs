//! 価格の閾値 (`price_below`) は Watch > Tag > Global の順で最初に指定されたものを使う。

use watchtower_core::{ConfigLayer, EffectiveConfig};

fn layer(price_below: Option<f64>) -> ConfigLayer {
    ConfigLayer {
        price_below,
        ..Default::default()
    }
}

#[test]
fn unset_everywhere_means_no_threshold() {
    let e = EffectiveConfig::resolve(&layer(None), &[layer(None)], &layer(None));
    assert_eq!(e.price_below, None);
}

#[test]
fn the_most_specific_layer_wins() {
    let e = EffectiveConfig::resolve(&layer(Some(10.0)), &[layer(Some(20.0))], &layer(Some(30.0)));
    assert_eq!(e.price_below, Some(10.0));
    let e = EffectiveConfig::resolve(&layer(None), &[layer(Some(20.0))], &layer(Some(30.0)));
    assert_eq!(e.price_below, Some(20.0));
    let e = EffectiveConfig::resolve(&layer(None), &[layer(None)], &layer(Some(30.0)));
    assert_eq!(e.price_below, Some(30.0));
}

#[test]
fn it_survives_a_json_roundtrip_and_old_data_without_it_still_loads() {
    let json = serde_json::to_string(&layer(Some(12.5))).unwrap();
    let back: ConfigLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.price_below, Some(12.5));
    let old: ConfigLayer = serde_json::from_str(r#"{"check_interval_secs": 60}"#).unwrap();
    assert_eq!(old.price_below, None);
}

#[test]
fn nonsensical_thresholds_are_dropped() {
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        let e = EffectiveConfig::resolve(&layer(Some(bad)), &[], &layer(None));
        assert_eq!(e.price_below, None, "{bad}");
    }
    let e = EffectiveConfig::resolve(&layer(Some(0.0)), &[], &layer(None));
    assert_eq!(e.price_below, Some(0.0), "zero is a legitimate threshold");
}
