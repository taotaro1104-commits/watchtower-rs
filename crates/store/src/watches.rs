use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;
use watchtower_core::{Watch, WatchState};

use crate::{from_ts, to_ts, Store, StoreError};

macro_rules! select_sql {
    ($tail:literal) => {
        concat!(
            "SELECT w.id, w.url, w.title, w.paused, w.tags_json, w.config_json, \
    s.next_check_at, s.last_checked_at, s.last_changed_at, s.last_error, s.error_count \
    FROM watches w LEFT JOIN watch_state s ON s.watch_id = w.id",
            $tail
        )
    };
}

fn opt_ts(v: Option<i64>) -> Result<Option<OffsetDateTime>, StoreError> {
    v.map(from_ts).transpose()
}

fn corrupt(e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

fn row_to_watch(r: &SqliteRow) -> Result<Watch, StoreError> {
    Ok(Watch {
        id: Uuid::parse_str(r.get("id")).map_err(corrupt)?,
        url: r.get::<String, _>("url").parse().map_err(corrupt)?,
        title: r.get("title"),
        paused: r.get::<i64, _>("paused") != 0,
        tags: serde_json::from_str(r.get("tags_json"))?,
        config: serde_json::from_str(r.get("config_json"))?,
        state: WatchState {
            next_check_at: opt_ts(r.get("next_check_at"))?,
            last_checked_at: opt_ts(r.get("last_checked_at"))?,
            last_changed_at: opt_ts(r.get("last_changed_at"))?,
            last_error: r.get("last_error"),
            consecutive_errors: r.get::<Option<i64>, _>("error_count").unwrap_or(0) as u32,
        },
    })
}

impl Store {
    pub async fn add_watch(&self, w: &Watch) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO watches (id, url, title, paused, tags_json, config_json) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(w.id.to_string())
        .bind(w.url.as_str())
        .bind(&w.title)
        .bind(w.paused as i64)
        .bind(serde_json::to_string(&w.tags)?)
        .bind(serde_json::to_string(&w.config)?)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO watch_state (watch_id) VALUES (?)")
            .bind(w.id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.update_state(w.id, &w.state).await
    }

    pub async fn get_watch(&self, id: Uuid) -> Result<Option<Watch>, StoreError> {
        let row = sqlx::query(select_sql!(" WHERE w.id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_watch).transpose()
    }

    pub async fn list_watches(&self) -> Result<Vec<Watch>, StoreError> {
        let rows = sqlx::query(select_sql!(" ORDER BY w.url"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_watch).collect()
    }

    pub async fn delete_watch(&self, id: Uuid) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM watches WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        match tokio::fs::remove_dir_all(self.snapshot_root.join(id.to_string())).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// 設定項目 (URL・タイトル・一時停止・タグ・設定) を更新する。状態には触れない。
    /// 該当ウォッチが無ければ false。
    pub async fn update_watch(&self, w: &Watch) -> Result<bool, StoreError> {
        // 検証後にタグが削除されても、存在しないタグ ID を書き戻さないよう、同じトランザクションで絞り込む
        let mut tx = self.pool.begin().await?;
        let known: Vec<String> = sqlx::query_scalar("SELECT id FROM tags")
            .fetch_all(&mut *tx)
            .await?;
        let tags: Vec<_> = w
            .tags
            .iter()
            .filter(|t| known.contains(&t.to_string()))
            .collect();
        let res = sqlx::query(
            "UPDATE watches SET url=?, title=?, paused=?, tags_json=?, config_json=? WHERE id=?",
        )
        .bind(w.url.as_str())
        .bind(&w.title)
        .bind(w.paused as i64)
        .bind(serde_json::to_string(&tags)?)
        .bind(serde_json::to_string(&w.config)?)
        .bind(w.id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn update_state(&self, id: Uuid, st: &WatchState) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE watch_state SET next_check_at=?, last_checked_at=?, last_changed_at=?, \
             last_error=?, error_count=? WHERE watch_id=?",
        )
        .bind(st.next_check_at.map(to_ts))
        .bind(st.last_checked_at.map(to_ts))
        .bind(st.last_changed_at.map(to_ts))
        .bind(&st.last_error)
        .bind(st.consecutive_errors as i64)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
