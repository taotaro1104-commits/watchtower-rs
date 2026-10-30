use std::time::Duration;

/// ジッター込みでも 0 秒間隔で空回りしないための下限。
const MIN_DELAY: Duration = Duration::from_secs(1);

/// `factor` は `-ratio..=ratio` の範囲 (例: ±10% なら -0.1..=0.1)。
pub fn apply_jitter(base: Duration, factor: f64) -> Duration {
    base.mul_f64((1.0 + factor).max(0.0)).max(MIN_DELAY)
}

/// `ratio` の範囲でランダムなジッター係数を返す。
pub fn random_jitter(ratio: f64) -> f64 {
    if ratio <= 0.0 {
        return 0.0;
    }
    rand::random_range(-ratio..=ratio)
}

pub fn success_delay(interval: Duration, jitter_factor: f64) -> Duration {
    apply_jitter(interval, jitter_factor)
}

/// 指数バックオフ。上限は通常間隔。`consecutive_errors` は今回を含む連続失敗回数。
pub fn error_delay(interval: Duration, base: Duration, consecutive_errors: u32) -> Duration {
    let exp = consecutive_errors.saturating_sub(1).min(20);
    let scaled = base.saturating_mul(1u32 << exp);
    scaled.min(interval).max(MIN_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn jitter_scales_within_ratio() {
        assert_eq!(
            apply_jitter(Duration::from_secs(100), 0.1),
            Duration::from_secs(110)
        );
        assert_eq!(
            apply_jitter(Duration::from_secs(100), -0.1),
            Duration::from_secs(90)
        );
        assert_eq!(
            apply_jitter(Duration::from_secs(100), 0.0),
            Duration::from_secs(100)
        );
    }

    #[test]
    fn random_jitter_stays_in_bounds() {
        for _ in 0..1000 {
            let f = random_jitter(0.1);
            assert!((-0.1..=0.1).contains(&f));
        }
        assert_eq!(random_jitter(0.0), 0.0);
    }

    #[test]
    fn delay_never_below_floor() {
        assert_eq!(success_delay(Duration::ZERO, 0.0), Duration::from_secs(1));
    }

    #[test]
    fn backoff_doubles_and_is_capped_by_interval() {
        let base = Duration::from_secs(30);
        assert_eq!(error_delay(MIN * 60, base, 1), Duration::from_secs(30));
        assert_eq!(error_delay(MIN * 60, base, 2), Duration::from_secs(60));
        assert_eq!(error_delay(MIN * 60, base, 3), Duration::from_secs(120));
        assert_eq!(error_delay(MIN * 5, base, 10), MIN * 5);
        assert_eq!(error_delay(MIN * 5, base, u32::MAX), MIN * 5);
    }

    #[test]
    fn error_delay_has_an_absolute_floor() {
        assert_eq!(
            error_delay(Duration::ZERO, Duration::from_secs(30), 1),
            Duration::from_secs(1)
        );
        assert_eq!(
            error_delay(Duration::from_secs(60), Duration::ZERO, 5),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn backoff_never_exceeds_a_short_interval() {
        assert_eq!(
            error_delay(Duration::from_secs(10), Duration::from_secs(30), 1),
            Duration::from_secs(10)
        );
    }
}
