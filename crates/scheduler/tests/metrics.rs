//! スケジューラが実行待ち時間と実行中の数を計測すること。時間は tokio の pause で制御する。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use watchtower_metrics::{Gauges, Metrics};
use watchtower_scheduler::{
    CheckRunner, Command, RunReport, ScheduledWatch, Scheduler, SchedulerConfig, SchedulerError,
    WatchId,
};

struct SlowRunner {
    watches: Vec<ScheduledWatch>,
    work: Duration,
}

#[async_trait]
impl CheckRunner for SlowRunner {
    async fn load(&self) -> Result<Vec<ScheduledWatch>, SchedulerError> {
        Ok(self.watches.clone())
    }
    async fn describe(&self, _id: WatchId) -> Result<Option<ScheduledWatch>, SchedulerError> {
        Ok(None)
    }
    async fn run(&self, _id: WatchId, _cancel: CancellationToken) -> RunReport {
        sleep(self.work).await;
        RunReport::next_in(Duration::from_secs(3600))
    }
}

fn value_of(text: &str, name: &str) -> f64 {
    text.lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap_or_else(|| panic!("{name} not found"))
        .parse()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn waiting_for_a_host_slot_is_measured_and_in_flight_returns_to_zero() {
    // 同じホストの 3 件を 1 つの枠で順に処理する: 待ちは 0 秒・1 秒・2 秒
    let watches: Vec<_> = (0..3)
        .map(|_| ScheduledWatch {
            id: Uuid::new_v4(),
            host: "same.example".into(),
            initial_delay: Duration::ZERO,
        })
        .collect();
    let runner = Arc::new(SlowRunner {
        watches,
        work: Duration::from_secs(1),
    });
    let metrics = Arc::new(Metrics::new());
    let cfg = SchedulerConfig {
        per_host_concurrency: 1,
        metrics: Arc::clone(&metrics),
        ..Default::default()
    };
    let (_tx, rx) = mpsc::channel::<Command>(4);
    let cancel = CancellationToken::new();
    let sched = Scheduler::new(runner, cfg);
    let c = cancel.clone();
    let join = tokio::spawn(async move { sched.run(rx, c).await });

    sleep(Duration::from_secs(10)).await;
    let text = metrics.render(&Gauges::default());
    assert_eq!(value_of(&text, "watchtower_queue_wait_seconds_count"), 3.0);
    let sum = value_of(&text, "watchtower_queue_wait_seconds_sum");
    assert!(
        (2.9..=3.1).contains(&sum),
        "waits should add up to about 3s, got {sum}"
    );
    assert_eq!(value_of(&text, "watchtower_checks_in_flight"), 0.0);

    cancel.cancel();
    join.await.unwrap().unwrap();
}
