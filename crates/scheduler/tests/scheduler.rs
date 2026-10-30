//! スケジューラのオーケストレーション。時間は tokio の pause で制御し、I/O は持たない。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::{sleep, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use watchtower_scheduler::{
    CheckRunner, Command, RunReport, ScheduledWatch, Scheduler, SchedulerConfig, SchedulerError,
    WatchId,
};

struct FakeRunner {
    watches: Vec<ScheduledWatch>,
    interval: Duration,
    work: Duration,
    runs: Mutex<HashMap<WatchId, Vec<Instant>>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    honour_cancel: bool,
}

impl FakeRunner {
    fn new(watches: Vec<ScheduledWatch>, interval: Duration, work: Duration) -> Arc<Self> {
        Arc::new(Self {
            watches,
            interval,
            work,
            runs: Mutex::default(),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            honour_cancel: true,
        })
    }
    fn count(&self, id: WatchId) -> usize {
        self.runs.lock().unwrap().get(&id).map_or(0, Vec::len)
    }
    fn total(&self) -> usize {
        self.runs.lock().unwrap().values().map(Vec::len).sum()
    }
}

#[async_trait]
impl CheckRunner for FakeRunner {
    async fn load(&self) -> Result<Vec<ScheduledWatch>, SchedulerError> {
        Ok(self.watches.clone())
    }
    async fn describe(&self, id: WatchId) -> Result<Option<ScheduledWatch>, SchedulerError> {
        Ok(self.watches.iter().find(|w| w.id == id).cloned())
    }
    async fn run(&self, id: WatchId, cancel: CancellationToken) -> RunReport {
        self.runs
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push(Instant::now());
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        if self.honour_cancel {
            tokio::select! { _ = sleep(self.work) => {}, _ = cancel.cancelled() => {} }
        } else {
            sleep(self.work).await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        RunReport::next_in(self.interval)
    }
}

fn sw(host: &str) -> ScheduledWatch {
    ScheduledWatch {
        id: Uuid::new_v4(),
        host: host.into(),
        initial_delay: Duration::ZERO,
    }
}

struct Harness {
    tx: mpsc::Sender<Command>,
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<Result<(), SchedulerError>>,
}

fn start(runner: Arc<FakeRunner>, cfg: SchedulerConfig) -> Harness {
    let (tx, rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    let sched = Scheduler::new(runner, cfg);
    let c = cancel.clone();
    let join = tokio::spawn(async move { sched.run(rx, c).await });
    Harness { tx, cancel, join }
}

fn cfg() -> SchedulerConfig {
    SchedulerConfig {
        global_concurrency: 10,
        per_host_concurrency: 10,
        ..Default::default()
    }
}

#[tokio::test(start_paused = true)]
async fn runs_each_watch_at_its_interval() {
    let w = sw("a.test");
    let r = FakeRunner::new(
        vec![w.clone()],
        Duration::from_secs(60),
        Duration::from_millis(1),
    );
    let h = start(r.clone(), cfg());
    sleep(Duration::from_secs(150)).await;
    assert_eq!(r.count(w.id), 3); // t=0, 60, 120
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn honours_initial_delay() {
    let mut w = sw("a.test");
    w.initial_delay = Duration::from_secs(30);
    let r = FakeRunner::new(
        vec![w.clone()],
        Duration::from_secs(600),
        Duration::from_millis(1),
    );
    let h = start(r.clone(), cfg());
    sleep(Duration::from_secs(29)).await;
    assert_eq!(r.count(w.id), 0);
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.count(w.id), 1);
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn global_concurrency_is_limited() {
    let ws: Vec<_> = (0..5).map(|i| sw(&format!("h{i}.test"))).collect();
    let r = FakeRunner::new(ws, Duration::from_secs(3600), Duration::from_secs(10));
    let c = SchedulerConfig {
        global_concurrency: 2,
        per_host_concurrency: 10,
        ..Default::default()
    };
    let h = start(r.clone(), c);
    sleep(Duration::from_secs(60)).await;
    assert_eq!(r.total(), 5);
    assert_eq!(r.max_active.load(Ordering::SeqCst), 2);
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn per_host_concurrency_is_limited() {
    let ws: Vec<_> = (0..4).map(|_| sw("same.test")).collect();
    let r = FakeRunner::new(ws, Duration::from_secs(3600), Duration::from_secs(10));
    let c = SchedulerConfig {
        global_concurrency: 10,
        per_host_concurrency: 1,
        ..Default::default()
    };
    let h = start(r.clone(), c);
    sleep(Duration::from_secs(60)).await;
    assert_eq!(r.total(), 4);
    assert_eq!(r.max_active.load(Ordering::SeqCst), 1);
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn check_now_runs_immediately_and_is_not_duplicated_while_running() {
    let mut w = sw("a.test");
    w.initial_delay = Duration::from_secs(3600);
    let r = FakeRunner::new(
        vec![w.clone()],
        Duration::from_secs(3600),
        Duration::from_secs(10),
    );
    let h = start(r.clone(), cfg());
    sleep(Duration::from_secs(1)).await;
    h.tx.send(Command::CheckNow(w.id)).await.unwrap();
    sleep(Duration::from_secs(1)).await;
    h.tx.send(Command::CheckNow(w.id)).await.unwrap(); // 実行中: 無視される
    sleep(Duration::from_secs(30)).await;
    assert_eq!(r.count(w.id), 1);
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn remove_stops_future_runs() {
    let w = sw("a.test");
    let r = FakeRunner::new(
        vec![w.clone()],
        Duration::from_secs(60),
        Duration::from_millis(1),
    );
    let h = start(r.clone(), cfg());
    sleep(Duration::from_secs(10)).await;
    h.tx.send(Command::Remove(w.id)).await.unwrap();
    sleep(Duration::from_secs(300)).await;
    assert_eq!(r.count(w.id), 1);
    h.cancel.cancel();
    h.join.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_running_job_within_grace() {
    let w = sw("a.test");
    let mut r = FakeRunner::new(
        vec![w.clone()],
        Duration::from_secs(3600),
        Duration::from_secs(10),
    );
    Arc::get_mut(&mut r).unwrap().honour_cancel = false;
    let c = SchedulerConfig {
        shutdown_grace: Duration::from_secs(30),
        ..cfg()
    };
    let h = start(r.clone(), c);
    sleep(Duration::from_secs(1)).await;
    h.cancel.cancel();
    let started = Instant::now();
    h.join.await.unwrap().unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(9),
        "should wait for the job"
    );
    assert_eq!(r.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn shutdown_gives_up_after_grace() {
    let w = sw("a.test");
    let mut r = FakeRunner::new(
        vec![w],
        Duration::from_secs(3600),
        Duration::from_secs(1000),
    );
    Arc::get_mut(&mut r).unwrap().honour_cancel = false;
    let c = SchedulerConfig {
        shutdown_grace: Duration::from_secs(30),
        ..cfg()
    };
    let h = start(r.clone(), c);
    sleep(Duration::from_secs(1)).await;
    h.cancel.cancel();
    let started = Instant::now();
    h.join.await.unwrap().unwrap();
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(29) && waited < Duration::from_secs(40),
        "{waited:?}"
    );
}
