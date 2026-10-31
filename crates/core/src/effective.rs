use crate::config::ConfigLayer;
use crate::steps::BrowserStep;

pub const DEFAULT_INTERVAL_SECS: u64 = 300;
/// これより短い間隔は対象サイトと DB への負荷が大きいので許さない。
pub const MIN_INTERVAL_SECS: u64 = 1;
/// 1 年。時刻計算のオーバーフローも避ける。
pub const MAX_INTERVAL_SECS: u64 = 366 * 24 * 3600;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// 層をマージした最終設定。マージ規則はここに集約する。
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveConfig {
    pub check_interval_secs: u64,
    pub timeout_secs: u64,
    pub max_body_bytes: usize,
    pub include_filters: Vec<String>,
    pub exclude_filters: Vec<String>,
    pub ignore_text: Vec<String>,
    pub trigger_text: Vec<String>,
    pub notify_urls: Vec<String>,
    pub notify_muted: bool,
    pub price_below: Option<f64>,
    pub use_browser: bool,
    pub browser_steps: Vec<BrowserStep>,
}

impl EffectiveConfig {
    /// 優先順位は watch > tags(渡された順) > global。
    /// スカラは最初の `Some`、リストは優先順に連結して重複除去する。
    pub fn resolve(watch: &ConfigLayer, tags: &[ConfigLayer], global: &ConfigLayer) -> Self {
        let layers: Vec<&ConfigLayer> = std::iter::once(watch)
            .chain(tags.iter())
            .chain(std::iter::once(global))
            .collect();

        let first = |f: fn(&ConfigLayer) -> Option<u64>| layers.iter().find_map(|l| f(l));
        let merge = |f: fn(&ConfigLayer) -> &Vec<String>| {
            let mut out: Vec<String> = Vec::new();
            for item in layers.iter().flat_map(|l| f(l).iter()) {
                if !out.contains(item) {
                    out.push(item.clone());
                }
            }
            out
        };

        Self {
            check_interval_secs: first(|l| l.check_interval_secs)
                .unwrap_or(DEFAULT_INTERVAL_SECS)
                .clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS),
            timeout_secs: first(|l| l.timeout_secs).unwrap_or(DEFAULT_TIMEOUT_SECS),
            max_body_bytes: layers
                .iter()
                .find_map(|l| l.max_body_bytes)
                .unwrap_or(DEFAULT_MAX_BODY_BYTES),
            include_filters: merge(|l| &l.include_filters),
            exclude_filters: merge(|l| &l.exclude_filters),
            ignore_text: merge(|l| &l.ignore_text),
            trigger_text: merge(|l| &l.trigger_text),
            notify_urls: merge(|l| &l.notify_urls),
            notify_muted: layers.iter().find_map(|l| l.notify_muted).unwrap_or(false),
            use_browser: layers.iter().find_map(|l| l.use_browser).unwrap_or(false),
            browser_steps: layers
                .iter()
                .map(|l| &l.browser_steps)
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_default(),
            // 不正な値 (NaN・無限大・負) は無視して、下の層の指定に任せる
            price_below: layers
                .iter()
                .find_map(|l| l.price_below.filter(|p| p.is_finite() && *p >= 0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(interval: Option<u64>, ignore: &[&str]) -> ConfigLayer {
        ConfigLayer {
            check_interval_secs: interval,
            ignore_text: ignore.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn defaults_when_all_layers_empty() {
        let e = EffectiveConfig::resolve(&Default::default(), &[], &Default::default());
        assert_eq!(e.check_interval_secs, DEFAULT_INTERVAL_SECS);
        assert_eq!(e.max_body_bytes, DEFAULT_MAX_BODY_BYTES);
        assert!(!e.notify_muted);
    }

    #[test]
    fn watch_beats_tag_beats_global() {
        let g = layer(Some(900), &[]);
        let t = layer(Some(600), &[]);
        let w = layer(Some(60), &[]);
        assert_eq!(
            EffectiveConfig::resolve(&w, std::slice::from_ref(&t), &g).check_interval_secs,
            60
        );
        assert_eq!(
            EffectiveConfig::resolve(&Default::default(), &[t], &g).check_interval_secs,
            600
        );
        assert_eq!(
            EffectiveConfig::resolve(&Default::default(), &[], &g).check_interval_secs,
            900
        );
    }

    #[test]
    fn lists_are_merged_in_priority_order_without_duplicates() {
        let w = layer(None, &["a", "b"]);
        let t = layer(None, &["b", "c"]);
        let g = layer(None, &["d", "a"]);
        let e = EffectiveConfig::resolve(&w, &[t], &g);
        assert_eq!(e.ignore_text, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn interval_is_clamped_to_a_sane_range() {
        let zero = layer(Some(0), &[]);
        let huge = layer(Some(u64::MAX), &[]);
        let none = ConfigLayer::default();
        assert_eq!(
            EffectiveConfig::resolve(&zero, &[], &none).check_interval_secs,
            MIN_INTERVAL_SECS
        );
        assert_eq!(
            EffectiveConfig::resolve(&huge, &[], &none).check_interval_secs,
            MAX_INTERVAL_SECS
        );
        // グローバル既定が不正でも範囲に収める
        assert_eq!(
            EffectiveConfig::resolve(&none, &[], &zero).check_interval_secs,
            MIN_INTERVAL_SECS
        );
    }

    #[test]
    fn muted_flag_follows_first_specified_layer() {
        let w = ConfigLayer {
            notify_muted: Some(false),
            ..Default::default()
        };
        let g = ConfigLayer {
            notify_muted: Some(true),
            ..Default::default()
        };
        assert!(!EffectiveConfig::resolve(&w, &[], &g).notify_muted);
        assert!(EffectiveConfig::resolve(&Default::default(), &[], &g).notify_muted);
    }
}
