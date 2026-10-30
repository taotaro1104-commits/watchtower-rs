//! 通知の送信待ちキュー (outbox)。送信済みの行は削除し、恒久的な失敗だけを残す。

use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{from_ts, to_ts, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotification {
    pub watch_id: Option<Uuid>,
    /// 通知先 URL (資格情報を含む)。
    pub target: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    Pending,
    Failed,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: i64,
    pub watch_id: Option<Uuid>,
    pub target: String,
    pub title: String,
    pub body: String,
    pub status: OutboxStatus,
    pub attempts: u32,
    pub next_attempt_at: OffsetDateTime,
    pub last_error: Option<String>,
    pub created_at: OffsetDateTime,
}

fn row_to_notification(r: &SqliteRow) -> Result<Notification, StoreError> {
    let corrupt = |e: &dyn std::fmt::Display| StoreError::Corrupt(e.to_string());
    Ok(Notification {
        id: r.get("id"),
        watch_id: r
            .get::<Option<String>, _>("watch_id")
            .map(|s| Uuid::parse_str(&s).map_err(|e| corrupt(&e)))
            .transpose()?,
        target: r.get("target"),
        title: r.get("title"),
        body: r.get("body"),
        status: match r.get::<String, _>("status").as_str() {
            "pending" => OutboxStatus::Pending,
            "failed" => OutboxStatus::Failed,
            other => return Err(StoreError::Corrupt(format!("outbox status {other}"))),
        },
        attempts: u32::try_from(r.get::<i64, _>("attempts")).unwrap_or(u32::MAX),
        next_attempt_at: from_ts(r.get("next_attempt_at"))?,
        last_error: r.get("last_error"),
        created_at: from_ts(r.get("created_at"))?,
    })
}

impl Store {
    /// 送信待ちに積む。すぐ送れる状態 (`next_attempt_at = now`) で入る。
    pub async fn enqueue_notification(
        &self,
        n: &NewNotification,
        now: OffsetDateTime,
    ) -> Result<i64, StoreError> {
        let res = sqlx::query(
            "INSERT INTO outbox (watch_id, target, title, body, next_attempt_at, created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(n.watch_id.map(|id| id.to_string()))
        .bind(&n.target)
        .bind(&n.title)
        .bind(&n.body)
        .bind(to_ts(now))
        .bind(to_ts(now))
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// 送信時刻が来ている保留中の通知を、積まれた順に最大 `limit` 件。
    pub async fn due_notifications(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<Notification>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, target, title, body, status, attempts, \
             next_attempt_at, last_error, created_at FROM outbox \
             WHERE status = 'pending' AND next_attempt_at <= ? ORDER BY id LIMIT ?",
        )
        .bind(to_ts(now))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_notification).collect()
    }

    /// 保留中・失敗済みをすべて (UI 表示用)。
    pub async fn list_notifications(&self) -> Result<Vec<Notification>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, target, title, body, status, attempts, \
             next_attempt_at, last_error, created_at FROM outbox ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_notification).collect()
    }

    pub async fn mark_notification_sent(&self, id: i64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM outbox WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 失敗を記録して `at` に再送する (試行回数を 1 増やす)。
    pub async fn retry_notification_at(
        &self,
        id: i64,
        error: &str,
        at: OffsetDateTime,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?, next_attempt_at = ? \
             WHERE id = ?",
        )
        .bind(error)
        .bind(to_ts(at))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 失敗済みの行を新しい `keep` 件だけ残して消し、消した件数を返す。保留中の行には触れない。
    pub async fn prune_failed_notifications(&self, keep: u32) -> Result<u64, StoreError> {
        let res = sqlx::query(
            "DELETE FROM outbox WHERE status = 'failed' AND id NOT IN              (SELECT id FROM outbox WHERE status = 'failed' ORDER BY id DESC LIMIT ?)",
        )
        .bind(i64::from(keep))
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 再送を打ち切る。行は表示のために残す。
    pub async fn fail_notification(&self, id: i64, error: &str) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE outbox SET status = 'failed', attempts = attempts + 1, last_error = ? \
             WHERE id = ?",
        )
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
