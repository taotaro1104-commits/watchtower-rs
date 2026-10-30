//! バックアップ (DB のオンラインコピー + スナップショットを 1 つの zip に) と復元。
//!
//! zip の中身は `watchtower.db` と `snapshots/<ウォッチ ID>/<ファイル>` だけ。
//! 復元は、それ以外の名前・`..`・絶対パスなどを含む zip をすべて拒否する (zip-slip 対策)。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sqlx::Row;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{Store, StoreError};

const DB_ENTRY: &str = "watchtower.db";
const SNAPSHOT_PREFIX: &str = "snapshots/";
const MAX_COMPONENT_LEN: usize = 255;
/// 復元で展開してよい合計サイズの既定値。
const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupReport {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct RestoreOptions {
    /// 既存のデータを消して置き換える。
    pub force: bool,
    /// 展開する合計サイズの上限 (zip 爆弾対策)。
    pub max_bytes: u64,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            force: false,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

fn backup_err(msg: impl Into<String>) -> StoreError {
    StoreError::Backup(msg.into())
}

fn zip_err(e: zip::result::ZipError) -> StoreError {
    backup_err(format!("zip: {e}"))
}

/// パスの 1 要素として安全か (英数字・`.`・`_`・`-` だけ、`.`/`..` は不可)。
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_COMPONENT_LEN
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

enum Entry<'a> {
    Db,
    Snapshot { dir: &'a str, file: &'a str },
}

fn classify(name: &str) -> Result<Entry<'_>, StoreError> {
    if name == DB_ENTRY {
        return Ok(Entry::Db);
    }
    let rest = name
        .strip_prefix(SNAPSHOT_PREFIX)
        .ok_or_else(|| backup_err("unexpected entry in the archive"))?;
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        [dir, file] if safe_component(dir) && safe_component(file) => {
            Ok(Entry::Snapshot { dir, file })
        }
        _ => Err(backup_err("unsafe entry name in the archive")),
    }
}

impl Store {
    /// DB と全スナップショットを `dest` (zip) に書き出す。`dest` が既にあれば何もせず失敗する。
    /// 稼働中でも使える (DB は SQLite のスナップショットを取る)。
    pub async fn backup_to(&self, dest: &Path) -> Result<BackupReport, StoreError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            // 通知先のトークンなどを含むので、本人だけが読める
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(dest)
            .map_err(|e| backup_err(format!("cannot create {}: {e}", dest.display())))?;

        let result = self.write_backup(file).await;
        if result.is_err() {
            let _ = fs::remove_file(dest);
        }
        result
    }

    async fn write_backup(&self, file: File) -> Result<BackupReport, StoreError> {
        let tmp = tempfile::tempdir()?;
        let copy = tmp.path().join("copy.db");
        sqlx::query("VACUUM INTO ?")
            .bind(copy.to_string_lossy().as_ref())
            .execute(&self.pool)
            .await?;
        let root = self.snapshot_root.clone();
        tokio::task::spawn_blocking(move || {
            let report = write_zip(file, &copy, &root);
            drop(tmp); // コピーはここまで残す
            report
        })
        .await
        .map_err(|e| backup_err(e.to_string()))?
    }

    /// 復元後、スナップショットの保存先を現在の場所に付け替える
    /// (DB には元の環境の絶対パスが入っているため)。
    pub(crate) async fn rebase_snapshot_paths(&self) -> Result<(), StoreError> {
        let rows = sqlx::query("SELECT id, watch_id, path FROM snapshots")
            .fetch_all(&self.pool)
            .await?;
        let mut tx = self.pool.begin().await?;
        for r in rows {
            let id: i64 = r.get("id");
            let watch_id: String = r.get("watch_id");
            let old: String = r.get("path");
            let name = old.rsplit(['/', '\\']).next().unwrap_or_default();
            if Uuid::parse_str(&watch_id).is_err() || !safe_component(name) {
                continue;
            }
            let new = self.snapshot_root.join(&watch_id).join(name);
            sqlx::query("UPDATE snapshots SET path = ? WHERE id = ?")
                .bind(new.to_string_lossy().as_ref())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn add_file<W: io::Write + io::Seek>(
    z: &mut ZipWriter<W>,
    name: &str,
    path: &Path,
    report: &mut BackupReport,
) -> Result<(), StoreError> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        // 取り込み中に GC などで消えたスナップショットは読み飛ばす
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    z.start_file(name, opts).map_err(zip_err)?;
    report.bytes += io::copy(&mut f, z)?;
    report.files += 1;
    Ok(())
}

fn write_zip(file: File, db_copy: &Path, root: &Path) -> Result<BackupReport, StoreError> {
    let mut z = ZipWriter::new(file);
    let mut report = BackupReport::default();
    add_file(&mut z, DB_ENTRY, db_copy, &mut report)?;
    if let Ok(dirs) = fs::read_dir(root) {
        for dir in dirs.flatten() {
            if !dir.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(dir_name) = dir.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(files) = fs::read_dir(dir.path()) else {
                continue;
            };
            for f in files.flatten() {
                let Some(file_name) = f.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                // 通常のファイルだけ (シンボリックリンクはたどらない)
                if !f.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                let entry = format!("{SNAPSHOT_PREFIX}{dir_name}/{file_name}");
                add_file(&mut z, &entry, &f.path(), &mut report)?;
            }
        }
    }
    z.finish().map_err(zip_err)?;
    Ok(report)
}

/// `zip_path` を `db_path` / `snapshot_root` に復元する。中身を検証してから書き込む。
pub async fn restore_backup(
    zip_path: &Path,
    db_path: &Path,
    snapshot_root: &Path,
    opts: &RestoreOptions,
) -> Result<BackupReport, StoreError> {
    let (zip_path, db, root, opts) = (
        zip_path.to_path_buf(),
        db_path.to_path_buf(),
        snapshot_root.to_path_buf(),
        *opts,
    );
    let (db2, root2) = (db.clone(), root.clone());
    let report = tokio::task::spawn_blocking(move || extract(&zip_path, &db2, &root2, &opts))
        .await
        .map_err(|e| backup_err(e.to_string()))??;
    let store = Store::open(&db, root).await?;
    store.rebase_snapshot_paths().await?;
    Ok(report)
}

fn validate(archive: &mut ZipArchive<File>, max_bytes: u64) -> Result<(), StoreError> {
    let (mut dbs, mut total) = (0usize, 0u64);
    let mut seen = std::collections::HashSet::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_err)?;
        let name = entry.name().map_err(zip_err)?.into_owned();
        if !entry.is_file() {
            return Err(backup_err("unexpected directory entry in the archive"));
        }
        match classify(&name)? {
            Entry::Db => dbs += 1,
            Entry::Snapshot { .. } => {}
        }
        if !seen.insert(name) {
            return Err(backup_err("duplicate entry in the archive"));
        }
        total = total.saturating_add(entry.size());
        if total > max_bytes {
            return Err(backup_err("the archive is larger than the allowed size"));
        }
    }
    if dbs != 1 {
        return Err(backup_err("the archive must contain exactly one database"));
    }
    Ok(())
}

/// 既に無いのは構わない。それ以外 (使用中など) は、サーバが動いていないか確かめてもらう。
fn ignore_not_found(r: io::Result<()>) -> Result<(), StoreError> {
    match r {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(backup_err(format!(
            "cannot remove the existing data ({e}); is the server still running?"
        ))),
        _ => Ok(()),
    }
}

fn create_new(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn extract(
    zip_path: &Path,
    db_path: &Path,
    snapshot_root: &Path,
    opts: &RestoreOptions,
) -> Result<BackupReport, StoreError> {
    let file = File::open(zip_path)
        .map_err(|e| backup_err(format!("cannot open {}: {e}", zip_path.display())))?;
    let mut archive = ZipArchive::new(file).map_err(zip_err)?;
    validate(&mut archive, opts.max_bytes)?;

    if db_path.exists() && !opts.force {
        return Err(backup_err(format!(
            "{} already exists; restoring would replace it (use --force)",
            db_path.display()
        )));
    }
    if opts.force {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db_path.as_os_str().to_owned();
            p.push(suffix);
            ignore_not_found(fs::remove_file(PathBuf::from(p)))?;
        }
        ignore_not_found(fs::remove_dir_all(snapshot_root))?;
    }
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(snapshot_root)?;

    let staged_db = db_path.with_extension("restoring");
    let _ = fs::remove_file(&staged_db);
    let mut written: Vec<PathBuf> = Vec::new();
    let result = (|| {
        let (mut report, mut budget) = (BackupReport::default(), opts.max_bytes);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(zip_err)?;
            let name = entry.name().map_err(zip_err)?.into_owned();
            let dest = match classify(&name)? {
                Entry::Db => staged_db.clone(),
                Entry::Snapshot { dir, file } => {
                    let dir = snapshot_root.join(dir);
                    fs::create_dir_all(&dir)?;
                    dir.join(file)
                }
            };
            let mut out = create_new(&dest)?;
            written.push(dest);
            // 宣言されたサイズを信用せず、残りの許容量で打ち切る
            let copied = io::copy(&mut entry.by_ref().take(budget.saturating_add(1)), &mut out)?;
            if copied > budget {
                return Err(backup_err("the archive expands beyond the allowed size"));
            }
            budget -= copied;
            report.bytes += copied;
            report.files += 1;
        }
        fs::rename(&staged_db, db_path)?;
        Ok(report)
    })();
    if result.is_err() {
        for p in &written {
            let _ = fs::remove_file(p);
        }
    }
    result
}
