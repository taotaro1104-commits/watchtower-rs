use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use watchtower_core::effective::MIN_INTERVAL_SECS;
use watchtower_core::{ConfigLayer, Watch};
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_metrics::Metrics;
use watchtower_scheduler::{
    CheckOutcome, CheckRunner, CheckerConfig, Scheduler, SchedulerConfig, StoreCheckRunner,
};
use watchtower_store::Store;

use crate::cli::{AddArgs, CheckArgs};
use crate::config::AppConfig;
use crate::notifications;
use crate::web_server;

const OUTCOME_QUEUE: usize = 256;
const MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024;

/// `backup` で作った zip から復元する。DB を開く前に呼ぶ。
pub async fn restore(cfg: &AppConfig, file: &std::path::Path, force: bool) -> Result<()> {
    let opts = watchtower_store::RestoreOptions {
        force,
        ..Default::default()
    };
    let r = watchtower_store::restore_backup(file, &cfg.db_path(), &cfg.snapshot_dir(), &opts)
        .await
        .context("restoring the backup")?;
    println!("restored {} files ({} bytes)", r.files, r.bytes);
    Ok(())
}

pub struct App {
    cfg: AppConfig,
    store: Store,
    /// チェッカー・スケジューラ・通知・API で共有する計測先。
    metrics: Arc<Metrics>,
}

impl App {
    pub async fn open(cfg: AppConfig) -> Result<Self> {
        tokio::fs::create_dir_all(&cfg.data_dir)
            .await
            .with_context(|| format!("creating data dir {}", cfg.data_dir.display()))?;
        let store = Store::open(&cfg.db_path(), cfg.snapshot_dir())
            .await
            .context("opening database")?;
        Ok(Self {
            cfg,
            store,
            metrics: Arc::new(Metrics::new()),
        })
    }

    fn runner(&self, outcomes: Option<mpsc::Sender<CheckOutcome>>) -> StoreCheckRunner {
        let fetcher = Arc::new(HttpFetcher::new(SsrfPolicy::with_allowed_ips(
            self.cfg.allowed_ips.clone(),
        )));
        let cfg = CheckerConfig {
            global: ConfigLayer {
                check_interval_secs: Some(self.cfg.default_interval_secs),
                ..Default::default()
            },
            snapshots_keep: self.cfg.snapshots_keep,
            metrics: Arc::clone(&self.metrics),
            ..Default::default()
        };
        StoreCheckRunner::new(self.store.clone(), fetcher, cfg, outcomes)
    }

    pub async fn add(&self, a: AddArgs) -> Result<()> {
        let url = Url::parse(&a.url).with_context(|| format!("invalid URL: {}", a.url))?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("only http/https URLs are supported: {url}");
        }
        if a.interval == Some(0) {
            bail!("interval must be at least {MIN_INTERVAL_SECS} second(s)");
        }
        notifications::validate_targets(&a.notify)?;
        let mut w = Watch::new(url);
        w.title = a.title;
        w.config.check_interval_secs = a.interval;
        w.config.include_filters = a.include;
        w.config.exclude_filters = a.exclude;
        w.config.ignore_text = a.ignore;
        w.config.trigger_text = a.trigger;
        w.config.notify_urls = a.notify;
        self.store.add_watch(&w).await?;
        println!("added {}", w.id);
        Ok(())
    }

    pub async fn list(&self) -> Result<()> {
        for w in self.store.list_watches().await? {
            let interval = w
                .config
                .check_interval_secs
                .unwrap_or(self.cfg.default_interval_secs);
            let checked = w
                .state
                .last_checked_at
                .and_then(|t| t.format(&Rfc3339).ok())
                .unwrap_or_else(|| "never".into());
            let status = match (&w.state.last_error, w.paused) {
                (_, true) => "paused".to_string(),
                (Some(e), _) => format!("error({}): {e}", w.state.consecutive_errors),
                _ => "ok".to_string(),
            };
            println!(
                "{}  {interval}s  {checked}  {status}  {}  {}",
                w.id,
                w.title.as_deref().unwrap_or("-"),
                w.url
            );
        }
        Ok(())
    }

    pub async fn remove(&self, id: Uuid) -> Result<()> {
        self.store
            .get_watch(id)
            .await?
            .ok_or_else(|| anyhow!("no such watch: {id}"))?;
        self.store.delete_watch(id).await?;
        println!("removed {id}");
        Ok(())
    }

    pub async fn check(&self, a: CheckArgs) -> Result<()> {
        let ids: Vec<Uuid> = match a.id {
            Some(id) => {
                self.store
                    .get_watch(id)
                    .await?
                    .ok_or_else(|| anyhow!("no such watch: {id}"))?;
                vec![id]
            }
            None => self
                .store
                .list_watches()
                .await?
                .into_iter()
                .filter(|w| !w.paused)
                .map(|w| w.id)
                .collect(),
        };

        let (tx, mut rx) = mpsc::channel(OUTCOME_QUEUE);
        let runner = self.runner(Some(tx));
        let mut failures = 0usize;
        for id in ids {
            runner.run(id, CancellationToken::new()).await;
            let Ok(outcome) = rx.try_recv() else { continue };
            failures += usize::from(outcome.error.is_some());
            print_outcome(&outcome);
        }
        if failures > 0 {
            bail!("{failures} check(s) failed");
        }
        Ok(())
    }

    pub async fn import(&self, file: &std::path::Path) -> Result<()> {
        let meta = tokio::fs::metadata(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        if meta.len() > MAX_IMPORT_BYTES {
            bail!("{} is too large to import", file.display());
        }
        let json = tokio::fs::read_to_string(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let r = self.store.import_changedetection(&json).await?;
        println!("imported {}, skipped {}", r.imported, r.skipped.len());
        for s in &r.skipped {
            println!("  skipped {s}");
        }
        for w in &r.warnings {
            println!("  warning: {w}");
        }
        Ok(())
    }

    pub async fn set_password(&self) -> Result<()> {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading password from stdin")?;
        let password = line.trim_end_matches(&['\r', '\n'][..]);
        web_server::set_password(&self.cfg, self.store.clone(), password).await?;
        println!("password updated");
        Ok(())
    }

    pub async fn api_key(&self) -> Result<()> {
        let key = web_server::rotate_api_key(self.store.clone()).await?;
        println!("API key: {key}");
        println!("Send it in the x-api-key header. Any previous key no longer works.");
        println!("This key is shown only once; only its hash is stored.");
        Ok(())
    }

    pub async fn backup(&self, file: &std::path::Path) -> Result<()> {
        let r = self
            .store
            .backup_to(file)
            .await
            .context("writing the backup")?;
        println!(
            "backup written to {} ({} files, {} bytes)",
            file.display(),
            r.files,
            r.bytes
        );
        println!("The archive contains notification URLs and password hashes; keep it private.");
        Ok(())
    }

    pub async fn rss_token(&self) -> Result<()> {
        let token = web_server::rotate_rss_token(self.store.clone()).await?;
        let base = self.cfg.base_url.trim_end_matches('/');
        println!("Feed URL: {base}/rss?token={token}");
        println!("Anyone with this URL can read the feed. Any previous token no longer works.");
        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
        let (out_tx, out_rx) = mpsc::channel::<CheckOutcome>(OUTCOME_QUEUE);
        let runner = Arc::new(self.runner(Some(out_tx)));
        let sched_cfg = SchedulerConfig {
            global_concurrency: self.cfg.global_concurrency,
            per_host_concurrency: self.cfg.per_host_concurrency,
            metrics: Arc::clone(&self.metrics),
            ..Default::default()
        };

        notifications::validate_targets(&self.cfg.notify_urls)?;
        let handler = notifications::spawn_outcome_handler(&self.cfg, self.store.clone(), out_rx);

        let stop = CancellationToken::new();
        let on_signal = stop.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!("shutdown requested");
            on_signal.cancel();
        });

        let worker = notifications::spawn_worker(
            &self.cfg,
            self.store.clone(),
            stop.clone(),
            Arc::clone(&self.metrics),
        );

        // 送信側を保持し続ける (閉じるとスケジューラが終了する)。Web UI からの追加・削除もここを通る。
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let web = web_server::spawn(
            &self.cfg,
            self.store.clone(),
            cmd_tx.clone(),
            stop.clone(),
            Arc::clone(&self.metrics),
        )
        .await?;
        tracing::info!(
            watches = self.store.list_watches().await?.len(),
            "scheduler starting"
        );
        Scheduler::new(runner, sched_cfg).run(cmd_rx, stop).await?;
        let _ = handler.await; // runner が drop されると送信側が閉じ、結果処理も終わる
        let _ = worker.await;
        let _ = web.await;
        drop(cmd_tx);
        Ok(())
    }
}

fn print_outcome(o: &CheckOutcome) {
    if let Some(e) = &o.error {
        println!("error  {}  {e}", o.url);
        return;
    }
    let label = if o.first_snapshot {
        "baseline"
    } else if o.changed {
        "changed"
    } else {
        "unchanged"
    };
    let trig = if o.triggered { " (triggered)" } else { "" };
    println!(
        "{label}  {}  {}ms{trig}",
        o.url,
        o.duration.as_millis().min(u128::from(u64::MAX))
    );
    if let Some(d) = &o.diff {
        for l in &d.removed {
            println!("- {l}");
        }
        for l in &d.added {
            println!("+ {l}");
        }
    }
}

/// Ctrl-C、unix では SIGTERM (systemd / docker stop) も待つ。
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!("cannot listen for SIGTERM: {e}");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
