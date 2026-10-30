use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tokio::task::{Id as TaskId, JoinSet};
use tokio::time::{sleep_until, timeout, Instant};
use tokio_util::sync::CancellationToken;
use watchtower_core::WatchId;

use crate::queue::DueQueue;
use crate::runner::{CheckRunner, RunReport, ScheduledWatch};
use crate::SchedulerError;

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub global_concurrency: usize,
    pub per_host_concurrency: usize,
    /// 終了時に実行中ジョブの完了を待つ最大時間。
    pub shutdown_grace: Duration,
    /// ジョブがパニックした場合の再試行までの待ち時間。
    pub panic_retry: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_concurrency: 10,
            per_host_concurrency: 2,
            shutdown_grace: Duration::from_secs(30),
            panic_retry: Duration::from_secs(60),
        }
    }
}

/// UI / API からスケジューラへの通知。
#[derive(Debug, Clone, Copy)]
pub enum Command {
    /// 今すぐチェックする (実行中なら無視)。
    CheckNow(WatchId),
    /// 設定の追加・変更。再読込して再スケジュールする。
    Upsert(WatchId),
    Remove(WatchId),
}

pub struct Scheduler {
    runner: Arc<dyn CheckRunner>,
    cfg: SchedulerConfig,
}

struct State {
    queue: DueQueue<Instant>,
    hosts: HashMap<WatchId, String>,
    in_flight: HashSet<WatchId>,
    /// 実行中に設定変更を受けたウォッチ。完了後すぐ再実行する。
    dirty: HashSet<WatchId>,
    jobs: JoinSet<(WatchId, RunReport)>,
    task_ids: HashMap<TaskId, WatchId>,
    global: Arc<Semaphore>,
    host_sems: HashMap<String, Arc<Semaphore>>,
}

impl Scheduler {
    pub fn new(runner: Arc<dyn CheckRunner>, cfg: SchedulerConfig) -> Self {
        Self { runner, cfg }
    }

    pub async fn run(
        self,
        mut commands: mpsc::Receiver<Command>,
        stop: CancellationToken,
    ) -> Result<(), SchedulerError> {
        let mut st = State {
            queue: DueQueue::new(),
            hosts: HashMap::new(),
            in_flight: HashSet::new(),
            dirty: HashSet::new(),
            jobs: JoinSet::new(),
            task_ids: HashMap::new(),
            global: Arc::new(Semaphore::new(self.cfg.global_concurrency.max(1))),
            host_sems: HashMap::new(),
        };
        let job_cancel = CancellationToken::new();

        for w in self.runner.load().await? {
            self.track(&mut st, w);
        }

        loop {
            let next = st.queue.next_due();
            let wake = async {
                match next {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                cmd = commands.recv() => match cmd {
                    Some(cmd) => self.handle_command(&mut st, cmd).await,
                    None => break,
                },
                Some(joined) = st.jobs.join_next_with_id(), if !st.jobs.is_empty() => {
                    self.handle_completion(&mut st, joined);
                }
                _ = wake => {
                    while let Some(id) = st.queue.pop_due(Instant::now()) {
                        self.spawn_job(&mut st, id, &stop, &job_cancel);
                    }
                }
            }
        }

        self.drain(&mut st, &job_cancel).await;
        Ok(())
    }

    /// 新しいジョブの起動を止め、実行中ジョブの完了を猶予時間だけ待つ。
    async fn drain(&self, st: &mut State, job_cancel: &CancellationToken) {
        let wait_all = async { while st.jobs.join_next().await.is_some() {} };
        if timeout(self.cfg.shutdown_grace, wait_all).await.is_err() {
            tracing::warn!("shutdown grace expired; aborting {} job(s)", st.jobs.len());
            job_cancel.cancel();
            st.jobs.abort_all();
            while st.jobs.join_next().await.is_some() {}
        }
    }

    fn track(&self, st: &mut State, w: ScheduledWatch) {
        st.hosts.insert(w.id, w.host);
        st.queue.schedule(w.id, Instant::now() + w.initial_delay);
    }

    async fn handle_command(&self, st: &mut State, cmd: Command) {
        match cmd {
            Command::CheckNow(id) => {
                if st.in_flight.contains(&id) {
                    return;
                }
                if let Entry::Vacant(e) = st.hosts.entry(id) {
                    let Some(w) = self.describe(id).await else {
                        return;
                    };
                    e.insert(w.host);
                }
                st.queue.schedule(id, Instant::now());
            }
            Command::Upsert(id) => match self.describe(id).await {
                Some(w) if st.in_flight.contains(&id) => {
                    st.hosts.insert(id, w.host);
                    st.dirty.insert(id);
                }
                Some(w) => self.track(st, w),
                None => self.forget(st, id),
            },
            Command::Remove(id) => self.forget(st, id),
        }
    }

    async fn describe(&self, id: WatchId) -> Option<ScheduledWatch> {
        match self.runner.describe(id).await {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(%id, "describe failed: {e}");
                None
            }
        }
    }

    fn forget(&self, st: &mut State, id: WatchId) {
        st.queue.remove(id);
        st.hosts.remove(&id);
        st.dirty.remove(&id);
    }

    fn spawn_job(
        &self,
        st: &mut State,
        id: WatchId,
        stop: &CancellationToken,
        job_cancel: &CancellationToken,
    ) {
        let Some(host) = st.hosts.get(&id).cloned() else {
            return;
        };
        if !st.in_flight.insert(id) {
            return;
        }
        let per_host = self.cfg.per_host_concurrency.max(1);
        let host_sem = st
            .host_sems
            .entry(host)
            .or_insert_with(|| Arc::new(Semaphore::new(per_host)))
            .clone();
        let global = Arc::clone(&st.global);
        let runner = Arc::clone(&self.runner);
        let (stop, token) = (stop.clone(), job_cancel.clone());

        let handle = st.jobs.spawn(async move {
            // ホスト → グローバルの順に取り、片方だけ握ったまま待たない。
            let _host = host_sem.acquire_owned().await;
            let _global = global.acquire_owned().await;
            if stop.is_cancelled() {
                // 終了処理中に順番が回ってきたジョブは開始しない。
                return (id, RunReport::unschedule());
            }
            (id, runner.run(id, token).await)
        });
        st.task_ids.insert(handle.id(), id);
    }

    fn handle_completion(
        &self,
        st: &mut State,
        joined: Result<(TaskId, (WatchId, RunReport)), tokio::task::JoinError>,
    ) {
        let (id, delay) = match joined {
            Ok((task, (id, report))) => {
                st.task_ids.remove(&task);
                (id, report.next_delay)
            }
            Err(e) => {
                let Some(id) = st.task_ids.remove(&e.id()) else {
                    return;
                };
                tracing::error!(%id, "check job failed: {e}");
                (id, Some(self.cfg.panic_retry))
            }
        };
        st.in_flight.remove(&id);
        if !st.hosts.contains_key(&id) {
            return; // 実行中に削除された
        }
        match delay {
            _ if st.dirty.remove(&id) => st.queue.schedule(id, Instant::now()),
            Some(d) => st.queue.schedule(id, Instant::now() + d),
            None => self.forget(st, id),
        }
    }
}
