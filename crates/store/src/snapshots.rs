use std::path::PathBuf;

use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{from_ts, to_ts, Store, StoreError};

const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRecord {
    pub watch_id: Uuid,
    pub taken_at: OffsetDateTime,
    pub hash: [u8; 32],
    pub path: PathBuf,
    pub size: u64,
}

fn row_to_record(r: &SqliteRow) -> Result<SnapshotRecord, StoreError> {
    let hash: Vec<u8> = r.get("hash");
    Ok(SnapshotRecord {
        watch_id: Uuid::parse_str(r.get("watch_id"))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        taken_at: from_ts(r.get("taken_at"))?,
        hash: hash
            .try_into()
            .map_err(|_| StoreError::Corrupt("hash length".into()))?,
        path: PathBuf::from(r.get::<String, _>("path")),
        size: r.get::<i64, _>("size") as u64,
    })
}

impl Store {
    /// 直近と同じハッシュなら保存せず `None` を返す。
    ///
    /// ファイルは一時ファイルに書いてから rename し、DB への INSERT に失敗したら消す。
    /// ファイル名は UUID で一意にする (同一秒・同一内容でも衝突しない)。
    pub async fn save_snapshot(
        &self,
        watch_id: Uuid,
        taken_at: OffsetDateTime,
        text: &str,
        hash: [u8; 32],
    ) -> Result<Option<SnapshotRecord>, StoreError> {
        let latest =
            sqlx::query("SELECT hash FROM snapshots WHERE watch_id=? ORDER BY id DESC LIMIT 1")
                .bind(watch_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        if let Some(r) = latest {
            if r.get::<Vec<u8>, _>("hash") == hash {
                return Ok(None);
            }
        }

        let dir = self.snapshot_root.join(watch_id.to_string());
        tokio::fs::create_dir_all(&dir).await?;
        let name = format!("{}-{}.txt.zst", to_ts(taken_at), Uuid::new_v4().simple());
        let path = dir.join(&name);
        let tmp = dir.join(format!("{name}.tmp"));
        let compressed = zstd::encode_all(text.as_bytes(), ZSTD_LEVEL)?;
        tokio::fs::write(&tmp, &compressed).await?;
        tokio::fs::rename(&tmp, &path).await?;

        let inserted = sqlx::query(
            "INSERT INTO snapshots (watch_id, taken_at, hash, path, size) VALUES (?,?,?,?,?)",
        )
        .bind(watch_id.to_string())
        .bind(to_ts(taken_at))
        .bind(hash.as_slice())
        .bind(path.to_string_lossy().as_ref())
        .bind(text.len() as i64)
        .execute(&self.pool)
        .await;
        if let Err(e) = inserted {
            remove_file_if_exists(&path).await?;
            return Err(e.into());
        }
        Ok(Some(SnapshotRecord {
            watch_id,
            taken_at,
            hash,
            path,
            size: text.len() as u64,
        }))
    }

    /// 直近 (挿入順で最後) のスナップショット。
    pub async fn latest_snapshot(
        &self,
        watch_id: Uuid,
    ) -> Result<Option<SnapshotRecord>, StoreError> {
        let row = sqlx::query("SELECT * FROM snapshots WHERE watch_id=? ORDER BY id DESC LIMIT 1")
            .bind(watch_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_record).transpose()
    }

    /// 行とファイルを削除する。ファイルが既に無くてもエラーにしない。
    pub async fn drop_snapshot(&self, rec: &SnapshotRecord) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM snapshots WHERE watch_id=? AND path=?")
            .bind(rec.watch_id.to_string())
            .bind(rec.path.to_string_lossy().as_ref())
            .execute(&self.pool)
            .await?;
        remove_file_if_exists(&rec.path).await
    }

    /// 古い順。
    pub async fn list_snapshots(&self, watch_id: Uuid) -> Result<Vec<SnapshotRecord>, StoreError> {
        let rows = sqlx::query("SELECT * FROM snapshots WHERE watch_id=? ORDER BY id")
            .bind(watch_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_record).collect()
    }

    pub async fn read_snapshot(&self, rec: &SnapshotRecord) -> Result<String, StoreError> {
        let bytes = zstd::decode_all(tokio::fs::read(&rec.path).await?.as_slice())?;
        String::from_utf8(bytes).map_err(|e| StoreError::Corrupt(e.to_string()))
    }

    pub async fn latest_snapshot_text(&self, watch_id: Uuid) -> Result<Option<String>, StoreError> {
        match self.latest_snapshot(watch_id).await? {
            Some(rec) => Ok(Some(self.read_snapshot(&rec).await?)),
            None => Ok(None),
        }
    }

    /// 新しい `keep` 件を残して古いものを削除し、削除件数を返す。
    pub async fn gc_snapshots(&self, watch_id: Uuid, keep: usize) -> Result<usize, StoreError> {
        let all = self.list_snapshots(watch_id).await?;
        let stale = all.len().saturating_sub(keep);
        for rec in &all[..stale] {
            self.drop_snapshot(rec).await?;
        }
        Ok(stale)
    }
}

async fn remove_file_if_exists(path: &std::path::Path) -> Result<(), StoreError> {
    match tokio::fs::remove_file(path).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
