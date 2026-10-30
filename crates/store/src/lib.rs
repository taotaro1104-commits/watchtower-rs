//! 永続化: メタデータは SQLite、スナップショット本文は zstd 圧縮ファイル。

mod admin;
mod backup;
mod error;
mod import;
mod outbox;
mod snapshots;
mod watches;

pub use admin::Event;
pub use backup::{restore_backup, BackupReport, RestoreOptions};
pub use error::StoreError;
pub use import::ImportReport;
pub use outbox::{NewNotification, Notification, OutboxStatus};
pub use snapshots::SnapshotRecord;

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
    pub(crate) snapshot_root: PathBuf,
}

impl Store {
    /// 接続を閉じる。`drop` だけでは接続が残ることがある (Windows ではファイルが開いたままになる)。
    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn open(db_path: &Path, snapshot_root: PathBuf) -> Result<Self, StoreError> {
        tokio::fs::create_dir_all(&snapshot_root).await?;
        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self {
            pool,
            snapshot_root,
        })
    }
}

pub(crate) fn to_ts(t: time::OffsetDateTime) -> i64 {
    t.unix_timestamp()
}

pub(crate) fn from_ts(secs: i64) -> Result<time::OffsetDateTime, StoreError> {
    time::OffsetDateTime::from_unix_timestamp(secs).map_err(|e| StoreError::Corrupt(e.to_string()))
}
