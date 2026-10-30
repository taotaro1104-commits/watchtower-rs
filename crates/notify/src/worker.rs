//! outbox から取り出して送るワーカー。失敗は指数バックオフで再送し、上限で打ち切る。

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use watchtower_store::{Notification, Store};

use crate::{Message, Notifier, NotifyError, NotifyTarget};

/// 1 回の走査で処理する最大件数。
const BATCH: u32 = 20;
/// 倍々にする回数の上限 (オーバーフロー防止)。
const MAX_DOUBLINGS: u32 = 16;
/// 表示用に残す失敗済み通知の件数。
const KEEP_FAILED: u32 = 500;
/// 履歴として残すイベントの件数。
const KEEP_EVENTS: u32 = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// 最初の失敗のあとに行う再送の回数。
    pub max_retries: u32,
    /// 最初の再送までの待ち。以降は 2 倍ずつ。
    pub base: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// `failures` 回目の失敗のあとの待ち時間 (1 始まり)。
    fn delay_after(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(MAX_DOUBLINGS);
        self.base.saturating_mul(2u32.saturating_pow(doublings))
    }
}

pub struct OutboxWorker {
    store: Store,
    notifier: Arc<dyn Notifier>,
    policy: RetryPolicy,
}

impl OutboxWorker {
    pub fn new(store: Store, notifier: Arc<dyn Notifier>, policy: RetryPolicy) -> Self {
        Self {
            store,
            notifier,
            policy,
        }
    }

    /// 送信時刻が来ている通知を 1 回ぶん処理し、試行した件数を返す。
    pub async fn run_once(
        &self,
        now: OffsetDateTime,
        cancel: CancellationToken,
    ) -> Result<usize, NotifyError> {
        self.store.prune_failed_notifications(KEEP_FAILED).await?;
        self.store.prune_events(KEEP_EVENTS).await?;
        let due = self.store.due_notifications(now, BATCH).await?;
        let mut attempted = 0;
        for item in &due {
            if cancel.is_cancelled() {
                break;
            }
            attempted += 1;
            self.deliver(item, now, cancel.clone()).await?;
        }
        Ok(attempted)
    }

    /// 停止されるまで `poll` 間隔で `run_once` を繰り返す。
    pub async fn run(&self, poll: Duration, cancel: CancellationToken) {
        loop {
            if let Err(e) = self
                .run_once(OffsetDateTime::now_utc(), cancel.clone())
                .await
            {
                tracing::error!("outbox worker: {e}");
            }
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(poll) => {}
            }
        }
    }

    async fn deliver(
        &self,
        item: &Notification,
        now: OffsetDateTime,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError> {
        let sent = match NotifyTarget::parse(&item.target) {
            // 保存後に解釈できなくなった宛先は再送しても直らない。
            Err(e) => return self.give_up(item, now, &e.to_string()).await,
            Ok(target) => {
                let msg = Message {
                    title: item.title.clone(),
                    body: item.body.clone(),
                };
                self.notifier.send(&target, &msg, cancel).await
            }
        };
        match sent {
            Ok(()) => {
                tracing::info!(id = item.id, "notification sent");
                // 先に送信済みにする。記録だけ先に済ませると、マークに失敗して再送したとき履歴が二重になる
                self.store.mark_notification_sent(item.id).await?;
                self.record(item, "notification_sent", now, None).await;
                Ok(())
            }
            // 停止による中断は失敗として数えず、次回に回す。
            Err(NotifyError::Cancelled) => Ok(()),
            Err(e) => self.after_failure(item, now, &e.to_string()).await,
        }
    }

    async fn after_failure(
        &self,
        item: &Notification,
        now: OffsetDateTime,
        error: &str,
    ) -> Result<(), NotifyError> {
        let failures = item.attempts.saturating_add(1);
        if failures > self.policy.max_retries {
            return self.give_up(item, now, error).await;
        }
        let wait = self.policy.delay_after(failures);
        let at = time::Duration::try_from(wait)
            .ok()
            .and_then(|d| now.checked_add(d))
            .unwrap_or(now);
        tracing::warn!(
            id = item.id,
            failures,
            error,
            "notification failed, will retry"
        );
        Ok(self.store.retry_notification_at(item.id, error, at).await?)
    }

    /// 履歴への記録は補助情報なので、失敗しても配送の処理は止めない。
    async fn record(
        &self,
        item: &Notification,
        kind: &str,
        now: OffsetDateTime,
        detail: Option<&str>,
    ) {
        if let Err(e) = self
            .store
            .record_event(item.watch_id, kind, now, detail)
            .await
        {
            tracing::warn!("recording event failed: {e}");
        }
    }

    async fn give_up(
        &self,
        item: &Notification,
        now: OffsetDateTime,
        error: &str,
    ) -> Result<(), NotifyError> {
        tracing::error!(id = item.id, error, "notification failed permanently");
        self.record(item, "notification_failed", now, Some(error))
            .await;
        Ok(self.store.fail_notification(item.id, error).await?)
    }
}
