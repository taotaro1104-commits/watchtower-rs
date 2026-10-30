//! 運用メトリクス。依存なし・アトミックだけで、Prometheus のテキスト形式に書き出す。

use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

/// 取得時間のバケット上限 (秒)。
const FETCH_BOUNDS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];
/// 実行待ちのバケット上限 (秒)。
const QUEUE_BOUNDS: &[f64] = &[0.001, 0.01, 0.1, 0.5, 1.0, 5.0, 30.0, 120.0];

/// 累積しないバケット数を持つヒストグラム。出力時に累積へ直す。
struct Histogram {
    bounds: &'static [f64],
    /// `bounds.len() + 1` 個。最後は +Inf (どの上限にも収まらない分)。
    buckets: Vec<AtomicU64>,
    sum_micros: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    fn new(bounds: &'static [f64]) -> Self {
        Self {
            bounds,
            buckets: (0..=bounds.len()).map(|_| AtomicU64::new(0)).collect(),
            sum_micros: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe(&self, d: Duration) {
        let secs = d.as_secs_f64();
        let i = self
            .bounds
            .iter()
            .position(|b| secs <= *b)
            .unwrap_or(self.bounds.len());
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
        let micros = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, out: &mut String, name: &str, help: &str) {
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} histogram");
        let mut cumulative = 0;
        for (i, bound) in self.bounds.iter().enumerate() {
            cumulative += self.buckets[i].load(Ordering::Relaxed);
            let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {cumulative}");
        }
        cumulative += self.buckets[self.bounds.len()].load(Ordering::Relaxed);
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {cumulative}");
        let sum = self.sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let _ = writeln!(out, "{name}_sum {sum}");
        let _ = writeln!(out, "{name}_count {}", self.count.load(Ordering::Relaxed));
    }
}

/// 出力時点で数え直す値 (DB から取る)。
#[derive(Debug, Clone, Copy, Default)]
pub struct Gauges {
    pub watches: u64,
    pub paused_watches: u64,
    pub queue_length: u64,
    pub outbox_pending: u64,
    pub outbox_failed: u64,
    pub uptime_seconds: u64,
}

pub struct Metrics {
    checks: AtomicU64,
    check_failures: AtomicU64,
    changes: AtomicU64,
    notifications_sent: AtomicU64,
    notifications_failed: AtomicU64,
    in_flight: AtomicI64,
    fetch: Histogram,
    queue_wait: Histogram,
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metrics { .. }")
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            checks: AtomicU64::new(0),
            check_failures: AtomicU64::new(0),
            changes: AtomicU64::new(0),
            notifications_sent: AtomicU64::new(0),
            notifications_failed: AtomicU64::new(0),
            in_flight: AtomicI64::new(0),
            fetch: Histogram::new(FETCH_BOUNDS),
            queue_wait: Histogram::new(QUEUE_BOUNDS),
        }
    }

    /// 1 回のチェックが終わった。
    pub fn check_finished(&self, failed: bool, changed: bool) {
        self.checks.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.check_failures.fetch_add(1, Ordering::Relaxed);
        }
        if changed {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn observe_fetch(&self, d: Duration) {
        self.fetch.observe(d);
    }

    /// 実行枠 (ホスト別・全体) を待った時間。
    pub fn observe_queue_wait(&self, d: Duration) {
        self.queue_wait.observe(d);
    }

    pub fn job_started(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn job_finished(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn notification_sent(&self) {
        self.notifications_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn notification_failed(&self) {
        self.notifications_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Prometheus のテキスト形式 (version 0.0.4)。
    pub fn render(&self, g: &Gauges) -> String {
        let mut out = String::with_capacity(4096);
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        counter(
            &mut out,
            "watchtower_checks_total",
            "Checks finished",
            load(&self.checks),
        );
        counter(
            &mut out,
            "watchtower_check_failures_total",
            "Checks that ended in an error",
            load(&self.check_failures),
        );
        counter(
            &mut out,
            "watchtower_changes_total",
            "Checks that found a change",
            load(&self.changes),
        );
        counter(
            &mut out,
            "watchtower_notifications_sent_total",
            "Notifications delivered",
            load(&self.notifications_sent),
        );
        counter(
            &mut out,
            "watchtower_notifications_failed_total",
            "Notifications given up on after retries",
            load(&self.notifications_failed),
        );
        gauge(
            &mut out,
            "watchtower_checks_in_flight",
            "Checks running right now",
            self.in_flight.load(Ordering::Relaxed).max(0) as u64,
        );
        gauge(&mut out, "watchtower_watches", "Watches", g.watches);
        gauge(
            &mut out,
            "watchtower_watches_paused",
            "Paused watches",
            g.paused_watches,
        );
        gauge(
            &mut out,
            "watchtower_queue_length",
            "Watches due or never checked",
            g.queue_length,
        );
        gauge(
            &mut out,
            "watchtower_outbox_pending",
            "Notifications waiting to be sent",
            g.outbox_pending,
        );
        gauge(
            &mut out,
            "watchtower_outbox_failed",
            "Notifications that failed permanently",
            g.outbox_failed,
        );
        gauge(
            &mut out,
            "watchtower_uptime_seconds",
            "Seconds since start",
            g.uptime_seconds,
        );
        self.fetch.render(
            &mut out,
            "watchtower_fetch_seconds",
            "Time spent fetching a page",
        );
        self.queue_wait.render(
            &mut out,
            "watchtower_queue_wait_seconds",
            "Time a due check waited for a free slot",
        );
        let _ = writeln!(out, "# HELP watchtower_build_info Build information");
        let _ = writeln!(out, "# TYPE watchtower_build_info gauge");
        let _ = writeln!(
            out,
            "watchtower_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );
        out
    }
}

fn counter(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} counter");
    let _ = writeln!(out, "{name} {value}");
}

fn gauge(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    let _ = writeln!(out, "{name} {value}");
}
