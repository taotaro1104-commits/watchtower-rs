//! Prometheus テキスト形式の出力。

use std::sync::Arc;
use std::time::Duration;

use watchtower_metrics::{Gauges, Metrics};

fn gauges() -> Gauges {
    Gauges {
        watches: 3,
        paused_watches: 1,
        queue_length: 2,
        outbox_pending: 4,
        outbox_failed: 5,
        uptime_seconds: 60,
    }
}

fn value_of(text: &str, name: &str) -> f64 {
    text.lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap_or_else(|| panic!("{name} not found in:\n{text}"))
        .parse()
        .unwrap()
}

#[test]
fn a_fresh_registry_renders_zero_counters_and_the_given_gauges() {
    let text = Metrics::new().render(&gauges());
    assert_eq!(value_of(&text, "watchtower_checks_total"), 0.0);
    assert_eq!(value_of(&text, "watchtower_check_failures_total"), 0.0);
    assert_eq!(value_of(&text, "watchtower_watches"), 3.0);
    assert_eq!(value_of(&text, "watchtower_watches_paused"), 1.0);
    assert_eq!(value_of(&text, "watchtower_queue_length"), 2.0);
    assert_eq!(value_of(&text, "watchtower_outbox_pending"), 4.0);
    assert_eq!(value_of(&text, "watchtower_outbox_failed"), 5.0);
    assert_eq!(value_of(&text, "watchtower_uptime_seconds"), 60.0);
}

#[test]
fn counters_accumulate() {
    let m = Metrics::new();
    m.check_finished(false, true);
    m.check_finished(true, false);
    m.check_finished(false, false);
    m.notification_sent();
    m.notification_failed();
    m.notification_failed();
    let text = m.render(&gauges());
    assert_eq!(value_of(&text, "watchtower_checks_total"), 3.0);
    assert_eq!(value_of(&text, "watchtower_check_failures_total"), 1.0);
    assert_eq!(value_of(&text, "watchtower_changes_total"), 1.0);
    assert_eq!(value_of(&text, "watchtower_notifications_sent_total"), 1.0);
    assert_eq!(
        value_of(&text, "watchtower_notifications_failed_total"),
        2.0
    );
}

#[test]
fn histograms_are_cumulative_with_sum_and_count() {
    let m = Metrics::new();
    m.observe_fetch(Duration::from_millis(30));
    m.observe_fetch(Duration::from_millis(700));
    m.observe_fetch(Duration::from_secs(100)); // どの上限にも収まらない
    let text = m.render(&gauges());

    let bucket = |le: &str| {
        text.lines()
            .find(|l| l.starts_with(&format!("watchtower_fetch_seconds_bucket{{le=\"{le}\"}}")))
            .unwrap_or_else(|| panic!("bucket {le} missing:\n{text}"))
            .rsplit(' ')
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    assert_eq!(bucket("0.05"), 1);
    assert_eq!(bucket("1"), 2);
    assert_eq!(bucket("+Inf"), 3);
    assert_eq!(value_of(&text, "watchtower_fetch_seconds_count"), 3.0);
    let sum = value_of(&text, "watchtower_fetch_seconds_sum");
    assert!((sum - 100.73).abs() < 0.01, "{sum}");

    // バケットは単調非減少
    let counts: Vec<u64> = text
        .lines()
        .filter(|l| l.starts_with("watchtower_fetch_seconds_bucket"))
        .map(|l| l.rsplit(' ').next().unwrap().parse().unwrap())
        .collect();
    assert!(counts.windows(2).all(|w| w[0] <= w[1]), "{counts:?}");
}

#[test]
fn queue_wait_has_its_own_histogram() {
    let m = Metrics::new();
    m.observe_queue_wait(Duration::from_millis(2));
    let text = m.render(&gauges());
    assert_eq!(value_of(&text, "watchtower_queue_wait_seconds_count"), 1.0);
    assert!(text.contains("watchtower_queue_wait_seconds_bucket{le=\"+Inf\"} 1"));
}

#[test]
fn in_flight_goes_up_and_down() {
    let m = Metrics::new();
    m.job_started();
    m.job_started();
    m.job_finished();
    assert_eq!(
        value_of(&m.render(&gauges()), "watchtower_checks_in_flight"),
        1.0
    );
}

#[test]
fn every_metric_has_help_and_type_and_lines_are_well_formed() {
    let text = Metrics::new().render(&gauges());
    assert!(text.ends_with('\n'));
    let mut typed = std::collections::HashSet::new();
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("# TYPE ") {
            let mut it = rest.split(' ');
            let name = it.next().unwrap();
            assert!(
                matches!(it.next(), Some("counter" | "gauge" | "histogram")),
                "{l}"
            );
            typed.insert(name.to_string());
        } else if !l.starts_with('#') {
            let name = l.split([' ', '{']).next().unwrap();
            let base = name
                .trim_end_matches("_bucket")
                .trim_end_matches("_sum")
                .trim_end_matches("_count");
            assert!(
                typed.contains(name) || typed.contains(base),
                "no TYPE before {l}"
            );
            let value = l.rsplit(' ').next().unwrap();
            assert!(value.parse::<f64>().is_ok_and(f64::is_finite), "{l}");
        }
    }
    assert!(text.contains("# HELP watchtower_checks_total"));
    assert!(text.contains("watchtower_build_info{version=\""));
}

#[test]
fn concurrent_updates_are_not_lost() {
    let m = Arc::new(Metrics::new());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let m = Arc::clone(&m);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    m.check_finished(false, false);
                    m.observe_fetch(Duration::from_millis(1));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let text = m.render(&gauges());
    assert_eq!(value_of(&text, "watchtower_checks_total"), 8000.0);
    assert_eq!(value_of(&text, "watchtower_fetch_seconds_count"), 8000.0);
}
