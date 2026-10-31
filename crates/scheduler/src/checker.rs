use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::{ConfigLayer, EffectiveConfig, Watch, WatchId, WatchState};
use watchtower_fetch::{FetchError, FetchRequest, Fetcher};
use watchtower_metrics::Metrics;
use watchtower_process::{diff, process, process_pdf, DiffResult};
use watchtower_store::Store;

use crate::backoff::{error_delay, random_jitter, success_delay};
use crate::runner::{CheckRunner, RunReport, ScheduledWatch};
use crate::SchedulerError;

/// ストア障害時の再試行間隔。
const STORE_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct CheckerConfig {
    /// グローバル設定 (Watch / Tag より低優先)。
    pub global: ConfigLayer,
    pub backoff_base: Duration,
    /// 例: 0.1 なら間隔の ±10%。
    pub jitter_ratio: f64,
    /// ウォッチごとに保持するスナップショット数。
    pub snapshots_keep: usize,
    /// チェック数・失敗数・取得時間の計測先。
    pub metrics: Arc<Metrics>,
}

impl Default for CheckerConfig {
    fn default() -> Self {
        Self {
            global: ConfigLayer::default(),
            backoff_base: Duration::from_secs(30),
            jitter_ratio: 0.1,
            snapshots_keep: 100,
            metrics: Arc::new(Metrics::new()),
        }
    }
}

/// 1 回のチェックの結果。通知や記録は後段が行う。
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub watch_id: WatchId,
    pub url: Url,
    pub changed: bool,
    /// 初回の基準スナップショットを保存した (変化としては扱わない)。
    pub first_snapshot: bool,
    pub triggered: bool,
    pub diff: Option<DiffResult>,
    pub error: Option<String>,
    pub duration: Duration,
}

struct Checked {
    changed: bool,
    first_snapshot: bool,
    triggered: bool,
    diff: Option<DiffResult>,
}

pub struct StoreCheckRunner {
    store: Store,
    fetcher: Arc<dyn Fetcher>,
    /// `use_browser` のウォッチ用 (ブラウザの設定があるときだけ)。
    browser: Option<Arc<dyn Fetcher>>,
    cfg: CheckerConfig,
    outcomes: Option<mpsc::Sender<CheckOutcome>>,
}

impl StoreCheckRunner {
    pub fn new(
        store: Store,
        fetcher: Arc<dyn Fetcher>,
        cfg: CheckerConfig,
        outcomes: Option<mpsc::Sender<CheckOutcome>>,
    ) -> Self {
        Self {
            store,
            fetcher,
            browser: None,
            cfg,
            outcomes,
        }
    }

    /// JavaScript の描画が要るウォッチ (`use_browser`) の取得に使う取得器を設定する。
    pub fn with_browser(mut self, browser: Arc<dyn Fetcher>) -> Self {
        self.browser = Some(browser);
        self
    }

    async fn effective(&self, w: &Watch) -> EffectiveConfig {
        let tags = self.store.tag_layers(&w.tags).await.unwrap_or_else(|e| {
            // タグが読めなくても監視は止めない (Watch と Global だけで続ける)。
            tracing::warn!(watch = %w.id, "failed to load tag settings: {e}");
            Vec::new()
        });
        EffectiveConfig::resolve(&w.config, &tags, &self.cfg.global)
    }

    async fn check(
        &self,
        w: &Watch,
        cfg: &EffectiveConfig,
        cancel: CancellationToken,
    ) -> Result<Checked, String> {
        let mut req = FetchRequest::get(w.url.clone());
        req.timeout = Duration::from_secs(cfg.timeout_secs);
        req.max_body_bytes = cfg.max_body_bytes;
        req.method = cfg
            .method
            .parse()
            .map_err(|_| "the configured HTTP method is not valid".to_string())?;
        req.headers = cfg.headers.clone();
        req.body = cfg.body.as_ref().map(|b| b.clone().into_bytes());
        req.proxy = cfg.proxy.clone();
        if !cfg.browser_steps.is_empty() && !cfg.use_browser {
            return Err("browser steps need the browser fetch method (html_webdriver)".to_string());
        }
        req.browser_steps = cfg.browser_steps.clone();
        let fetch_started = Instant::now();
        let fetcher = if cfg.use_browser {
            self.browser.as_ref().ok_or_else(|| {
                "this watch needs a browser, but no browser is configured (set browser_ws_url)"
                    .to_string()
            })?
        } else {
            &self.fetcher
        };
        let fetched = fetcher.fetch(&req, cancel).await;
        if !matches!(fetched, Err(FetchError::Cancelled)) {
            self.cfg.metrics.observe_fetch(fetch_started.elapsed());
        }
        let resp = fetched.map_err(|e| e.to_string())?;

        // PDF などのバイナリは生のバイト列から、それ以外は文字列から
        let processed = match &resp.raw {
            Some(raw) => process_pdf(raw, cfg),
            None => process(&resp.body, resp.content_type.as_deref(), cfg),
        }
        .map_err(|e| e.to_string())?;
        let previous = self.previous_text(w.id).await?;
        let saved = self
            .store
            .save_snapshot(
                w.id,
                OffsetDateTime::now_utc(),
                &processed.text,
                processed.hash,
            )
            .await
            .map_err(|e| e.to_string())?;

        let (changed, first_snapshot, d) = match (saved, previous) {
            (Some(_), None) => (false, true, None),
            (Some(_), Some(prev)) => (true, false, Some(diff(&prev, &processed.text))),
            (None, _) => (false, false, None),
        };
        if changed || first_snapshot {
            if let Err(e) = self.store.gc_snapshots(w.id, self.cfg.snapshots_keep).await {
                tracing::warn!(watch = %w.id, "snapshot gc failed: {e}");
            }
        }
        Ok(Checked {
            changed,
            first_snapshot,
            triggered: processed.triggered,
            diff: d,
        })
    }

    /// 直近スナップショットの本文。ファイルが壊れている / 無い場合は、その記録を捨てて
    /// 「基準なし」として扱う (放置すると以降のチェックが毎回失敗して止まってしまう)。
    async fn previous_text(&self, id: WatchId) -> Result<Option<String>, String> {
        let Some(rec) = self
            .store
            .latest_snapshot(id)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        match self.store.read_snapshot(&rec).await {
            Ok(text) => Ok(Some(text)),
            Err(e) => {
                tracing::warn!(
                    watch = %id, path = %rec.path.display(),
                    "unreadable snapshot, re-baselining: {e}"
                );
                self.store
                    .drop_snapshot(&rec)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(None)
            }
        }
    }

    fn to_describe(&self, w: &Watch) -> ScheduledWatch {
        let initial_delay = w
            .state
            .next_check_at
            .map(|t| t - OffsetDateTime::now_utc())
            .and_then(|d| Duration::try_from(d).ok())
            .unwrap_or(Duration::ZERO);
        ScheduledWatch {
            id: w.id,
            host: w.url.host_str().unwrap_or_default().to_string(),
            initial_delay,
        }
    }
}

#[async_trait]
impl CheckRunner for StoreCheckRunner {
    async fn load(&self) -> Result<Vec<ScheduledWatch>, SchedulerError> {
        let all = self.store.list_watches().await?;
        Ok(all
            .iter()
            .filter(|w| !w.paused)
            .map(|w| self.to_describe(w))
            .collect())
    }

    async fn describe(&self, id: WatchId) -> Result<Option<ScheduledWatch>, SchedulerError> {
        let w = self.store.get_watch(id).await?;
        Ok(w.filter(|w| !w.paused).map(|w| self.to_describe(&w)))
    }

    async fn run(&self, id: WatchId, cancel: CancellationToken) -> RunReport {
        let watch = match self.store.get_watch(id).await {
            Ok(Some(w)) if !w.paused => w,
            Ok(_) => return RunReport::unschedule(),
            Err(e) => {
                tracing::error!(%id, "failed to load watch: {e}");
                return RunReport::next_in(STORE_RETRY);
            }
        };
        let cfg = self.effective(&watch).await;
        let interval = Duration::from_secs(cfg.check_interval_secs);

        let started = Instant::now();
        let result = self.check(&watch, &cfg, cancel.clone()).await;
        let duration = started.elapsed();

        if cancel.is_cancelled() {
            return RunReport::next_in(interval); // 中断は失敗として数えない
        }

        let now = OffsetDateTime::now_utc();
        let mut state: WatchState = watch.state.clone();
        state.last_checked_at = Some(now);
        let (delay, outcome) = match result {
            Ok(c) => {
                self.cfg.metrics.check_finished(false, c.changed);
                state.consecutive_errors = 0;
                state.last_error = None;
                if c.changed {
                    state.last_changed_at = Some(now);
                }
                let delay = success_delay(interval, random_jitter(self.cfg.jitter_ratio));
                let outcome = CheckOutcome {
                    watch_id: id,
                    url: watch.url.clone(),
                    changed: c.changed,
                    first_snapshot: c.first_snapshot,
                    triggered: c.triggered,
                    diff: c.diff,
                    error: None,
                    duration,
                };
                (delay, outcome)
            }
            Err(msg) => {
                self.cfg.metrics.check_finished(true, false);
                state.consecutive_errors = state.consecutive_errors.saturating_add(1);
                state.last_error = Some(msg.clone());
                let delay = error_delay(interval, self.cfg.backoff_base, state.consecutive_errors);
                let outcome = CheckOutcome {
                    watch_id: id,
                    url: watch.url.clone(),
                    changed: false,
                    first_snapshot: false,
                    triggered: false,
                    diff: None,
                    error: Some(msg),
                    duration,
                };
                (delay, outcome)
            }
        };
        state.next_check_at = Some(
            i64::try_from(delay.as_secs())
                .ok()
                .and_then(|secs| now.checked_add(time::Duration::seconds(secs)))
                .unwrap_or(now),
        );
        if let Err(e) = self.store.update_state(id, &state).await {
            tracing::error!(%id, "failed to persist watch state: {e}");
        }
        if let Some(tx) = &self.outcomes {
            let _ = tx.send(outcome).await; // 受信側が閉じていても監視は続ける
        }
        RunReport::next_in(delay)
    }
}
