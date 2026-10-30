//! 通知まわりの組み立て: チェック結果の受信 → outbox → 送信ワーカー。

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use watchtower_core::ConfigLayer;
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_notify::{
    ChangeEvent, CompositeNotifier, Dispatcher, HttpNotifier, NotifyTarget, OutboxWorker,
    RetryPolicy, SmtpNotifier, TemplateSet,
};
use watchtower_scheduler::CheckOutcome;
use watchtower_store::Store;

use crate::config::AppConfig;

/// outbox の確認間隔。
const OUTBOX_POLL: Duration = Duration::from_secs(2);

/// 起動時にグローバル設定の通知先を検証する (不正なら起動しない)。
pub fn validate_targets(urls: &[String]) -> anyhow::Result<()> {
    for u in urls {
        NotifyTarget::parse(u).map_err(|e| anyhow::anyhow!("invalid notification target: {e}"))?;
    }
    Ok(())
}

/// 送信ワーカーを起動する。`stop` でキャンセルされるまで動く。
pub fn spawn_worker(cfg: &AppConfig, store: Store, stop: CancellationToken) -> JoinHandle<()> {
    // HTTP 系も SMTP も同じ SSRF ポリシー (許可 IP) に従う
    let policy = SsrfPolicy::with_allowed_ips(cfg.allowed_ips.clone());
    let http = Arc::new(HttpNotifier::new(Arc::new(HttpFetcher::new(
        policy.clone(),
    ))));
    let smtp = Arc::new(SmtpNotifier::new(Arc::new(policy)));
    let worker = OutboxWorker::new(
        store,
        Arc::new(CompositeNotifier::new(http, smtp)),
        RetryPolicy::default(),
    );
    tokio::spawn(async move { worker.run(OUTBOX_POLL, stop).await })
}

/// チェック結果を受け取り、ログに出し、内容が変わっていれば通知を積む。
/// 送信側 (スケジューラ) が閉じると終了する。
pub fn spawn_outcome_handler(
    cfg: &AppConfig,
    store: Store,
    mut rx: mpsc::Receiver<CheckOutcome>,
) -> JoinHandle<()> {
    let global = ConfigLayer {
        notify_urls: cfg.notify_urls.clone(),
        ..Default::default()
    };
    let dispatcher = Dispatcher::new(
        store.clone(),
        global,
        TemplateSet::default(),
        cfg.base_url.clone(),
    );
    tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            log_outcome(&o);
            record_event(&store, &o).await;
            let (true, Some(diff), None) = (o.changed, o.diff, &o.error) else {
                continue;
            };
            let ev = ChangeEvent {
                watch_id: o.watch_id,
                diff,
                triggered: o.triggered,
            };
            if let Err(e) = dispatcher.on_change(&ev, OffsetDateTime::now_utc()).await {
                tracing::error!(watch = %o.watch_id, "queueing notifications failed: {e}");
            }
        }
    })
}

/// チェック結果のうち履歴に残すもの (変化なしは残さない)。
fn event_for(o: &CheckOutcome) -> Option<(&'static str, Option<String>)> {
    if let Some(e) = &o.error {
        return Some(("check_failed", Some(e.clone())));
    }
    if o.first_snapshot {
        return Some(("baseline", None));
    }
    if o.changed {
        let (added, removed) = o
            .diff
            .as_ref()
            .map_or((0, 0), |d| (d.added.len(), d.removed.len()));
        return Some(("changed", Some(format!("+{added} -{removed}"))));
    }
    None
}

async fn record_event(store: &Store, o: &CheckOutcome) {
    let Some((kind, detail)) = event_for(o) else {
        return;
    };
    let at = OffsetDateTime::now_utc();
    if let Err(e) = store
        .record_event(Some(o.watch_id), kind, at, detail.as_deref())
        .await
    {
        tracing::warn!(watch = %o.watch_id, "recording event failed: {e}");
    }
}

fn log_outcome(o: &CheckOutcome) {
    match &o.error {
        Some(e) => tracing::warn!(watch = %o.watch_id, url = %o.url, error = %e, "check failed"),
        None => tracing::info!(
            watch = %o.watch_id, url = %o.url, changed = o.changed,
            first = o.first_snapshot, triggered = o.triggered,
            ms = u64::try_from(o.duration.as_millis()).unwrap_or(u64::MAX),
            "checked"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use url::Url;
    use watchtower_process::DiffResult;

    use super::*;

    fn outcome() -> CheckOutcome {
        CheckOutcome {
            watch_id: uuid::Uuid::new_v4(),
            url: Url::parse("https://example.com/").unwrap(),
            changed: false,
            first_snapshot: false,
            triggered: false,
            diff: None,
            error: None,
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn unchanged_checks_produce_no_event() {
        assert_eq!(event_for(&outcome()), None);
    }

    #[test]
    fn baseline_change_and_error_are_recorded() {
        let mut o = outcome();
        o.first_snapshot = true;
        assert_eq!(event_for(&o), Some(("baseline", None)));

        let mut o = outcome();
        o.changed = true;
        o.diff = Some(DiffResult {
            added: vec!["a".into(), "b".into()],
            removed: vec!["c".into()],
        });
        assert_eq!(event_for(&o), Some(("changed", Some("+2 -1".to_string()))));

        let mut o = outcome();
        o.error = Some("HTTP 500".into());
        assert_eq!(
            event_for(&o),
            Some(("check_failed", Some("HTTP 500".to_string())))
        );
    }
}
