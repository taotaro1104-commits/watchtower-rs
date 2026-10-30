use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use watchtower_core::WatchId;

use crate::SchedulerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledWatch {
    pub id: WatchId,
    /// ホスト別セマフォのキー。
    pub host: String,
    /// 現在時刻からの初回実行までの待ち時間。
    pub initial_delay: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// `None` なら再スケジュールしない (削除・一時停止)。
    pub next_delay: Option<Duration>,
}

impl RunReport {
    pub fn next_in(d: Duration) -> Self {
        Self {
            next_delay: Some(d),
        }
    }
    pub fn unschedule() -> Self {
        Self { next_delay: None }
    }
}

/// スケジューラが呼び出すチェック処理。I/O はすべてこの向こう側に置く。
#[async_trait]
pub trait CheckRunner: Send + Sync {
    /// 起動時に監視対象をすべて返す。
    async fn load(&self) -> Result<Vec<ScheduledWatch>, SchedulerError>;
    /// 単一ウォッチの現在の状態。`None` は対象外 (削除済み・一時停止)。
    async fn describe(&self, id: WatchId) -> Result<Option<ScheduledWatch>, SchedulerError>;
    /// 1 回分のチェックを実行し、次回までの間隔を返す。
    async fn run(&self, id: WatchId, cancel: CancellationToken) -> RunReport;
}
