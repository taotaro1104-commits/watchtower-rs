//! タグ・設定・イベントの永続化。

use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;
use watchtower_core::{ConfigLayer, Tag};

use crate::{from_ts, to_ts, Store, StoreError};

const GLOBAL_NOTIFY_URLS: &str = "global_notify_urls";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    pub watch_id: Option<Uuid>,
    pub kind: String,
    pub at: OffsetDateTime,
    pub detail: Option<String>,
}

fn corrupt(e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

fn row_to_tag(r: &SqliteRow) -> Result<Tag, StoreError> {
    Ok(Tag {
        id: Uuid::parse_str(r.get("id")).map_err(corrupt)?,
        title: r.get("title"),
        config: serde_json::from_str(r.get("config_json"))?,
    })
}

fn row_to_event(r: &SqliteRow) -> Result<Event, StoreError> {
    Ok(Event {
        id: r.get("id"),
        watch_id: r
            .get::<Option<String>, _>("watch_id")
            .map(|s| Uuid::parse_str(&s).map_err(corrupt))
            .transpose()?,
        kind: r.get("kind"),
        at: from_ts(r.get("at"))?,
        detail: r.get("detail"),
    })
}

impl Store {
    pub async fn add_tag(&self, t: &Tag) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO tags (id, title, config_json) VALUES (?,?,?)")
            .bind(t.id.to_string())
            .bind(&t.title)
            .bind(serde_json::to_string(&t.config)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_tag(&self, id: Uuid) -> Result<Option<Tag>, StoreError> {
        let row = sqlx::query("SELECT id, title, config_json FROM tags WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_tag).transpose()
    }

    pub async fn list_tags(&self) -> Result<Vec<Tag>, StoreError> {
        let rows = sqlx::query("SELECT id, title, config_json FROM tags ORDER BY title, id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_tag).collect()
    }

    /// ウォッチが持つタグの設定層を、タグの並び順 (= 優先順) で返す。存在しないタグは無視する。
    pub async fn tag_layers(&self, ids: &[Uuid]) -> Result<Vec<ConfigLayer>, StoreError> {
        let mut layers = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(tag) = self.get_tag(*id).await? {
                layers.push(tag.config);
            }
        }
        Ok(layers)
    }

    pub async fn update_tag(&self, t: &Tag) -> Result<bool, StoreError> {
        let res = sqlx::query("UPDATE tags SET title = ?, config_json = ? WHERE id = ?")
            .bind(&t.title)
            .bind(serde_json::to_string(&t.config)?)
            .bind(t.id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// タグを消し、それを持つウォッチからも外す。タグが無ければ false。
    pub async fn delete_tag(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await?;
        let res = sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() == 0 {
            return Ok(false);
        }
        let rows = sqlx::query("SELECT id, tags_json FROM watches")
            .fetch_all(&mut *tx)
            .await?;
        for r in &rows {
            let mut tags: Vec<Uuid> = serde_json::from_str(r.get("tags_json"))?;
            let before = tags.len();
            tags.retain(|t| *t != id);
            if tags.len() != before {
                sqlx::query("UPDATE watches SET tags_json = ? WHERE id = ?")
                    .bind(serde_json::to_string(&tags)?)
                    .bind(r.get::<String, _>("id"))
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn get_setting(&self, key: &str) -> Result<Option<Value>, StoreError> {
        let row = sqlx::query("SELECT value_json FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| serde_json::from_str(r.get("value_json")).map_err(Into::into))
            .transpose()
    }

    pub async fn set_setting(&self, key: &str, value: &Value) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO settings (key, value_json) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 全ウォッチ共通の通知先 (設定画面で編集する)。未設定なら空。
    pub async fn global_notify_urls(&self) -> Result<Vec<String>, StoreError> {
        match self.get_setting(GLOBAL_NOTIFY_URLS).await? {
            Some(v) => Ok(serde_json::from_value(v)?),
            None => Ok(Vec::new()),
        }
    }

    pub async fn set_global_notify_urls(&self, urls: &[String]) -> Result<(), StoreError> {
        self.set_setting(GLOBAL_NOTIFY_URLS, &serde_json::to_value(urls)?)
            .await
    }

    pub async fn record_event(
        &self,
        watch_id: Option<Uuid>,
        kind: &str,
        at: OffsetDateTime,
        detail: Option<&str>,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO events (watch_id, kind, at, detail) VALUES (?,?,?,?)")
            .bind(watch_id.map(|id| id.to_string()))
            .bind(kind)
            .bind(to_ts(at))
            .bind(detail)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 記録済みイベントの最大 ID (まだ無ければ 0)。新着の検知に使う。
    pub async fn latest_event_id(&self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COALESCE(MAX(id), 0) AS id FROM events")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("id"))
    }

    /// 新しい順。`watch_id` を渡すとそのウォッチのものだけ。
    pub async fn list_events(
        &self,
        watch_id: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, kind, at, detail FROM events \
             WHERE (?1 IS NULL OR watch_id = ?1) ORDER BY at DESC, id DESC LIMIT ?2",
        )
        .bind(watch_id.map(|id| id.to_string()))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_event).collect()
    }

    /// 新しい `keep` 件を残して古いものを消し、消した件数を返す。
    pub async fn prune_events(&self, keep: u32) -> Result<u64, StoreError> {
        let res = sqlx::query(
            "DELETE FROM events WHERE id NOT IN \
             (SELECT id FROM events ORDER BY at DESC, id DESC LIMIT ?)",
        )
        .bind(i64::from(keep))
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
}
