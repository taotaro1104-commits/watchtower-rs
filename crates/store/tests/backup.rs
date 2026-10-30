//! バックアップ (DB のオンラインコピー + スナップショットを zip に) と復元。

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use url::Url;
use watchtower_core::{Tag, Watch};
use watchtower_store::{restore_backup, RestoreOptions, Store};

struct Env {
    store: Store,
    dir: tempfile::TempDir,
}

impl Env {
    fn zip(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("watchtower.db"),
        dir.path().join("snapshots"),
    )
    .await
    .unwrap();
    Env { store, dir }
}

fn ts(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap()
}

async fn seed(e: &Env) -> Watch {
    let tag = Tag::new("Shops");
    e.store.add_tag(&tag).await.unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/item").unwrap());
    w.title = Some("Item".into());
    w.tags = vec![tag.id];
    e.store.add_watch(&w).await.unwrap();
    e.store
        .save_snapshot(w.id, ts(0), "price: 10", [1; 32])
        .await
        .unwrap();
    e.store
        .save_snapshot(w.id, ts(60), "price: 9", [2; 32])
        .await
        .unwrap();
    e.store
        .set_setting("answer", &serde_json::json!({"x": 42}))
        .await
        .unwrap();
    w
}

fn zip_names(path: &Path) -> BTreeSet<String> {
    let f = std::fs::File::open(path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
        .collect()
}

/// 任意の名前のエントリだけを持つ zip を作る (zip-slip などの検証用)。
fn crafted_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

fn target(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    (
        dir.path().join("restored.db"),
        dir.path().join("restored-snaps"),
    )
}

#[tokio::test]
async fn a_backup_contains_the_database_and_every_snapshot() {
    let e = env().await;
    let w = seed(&e).await;
    let out = e.zip("backup.zip");
    let report = e.store.backup_to(&out).await.unwrap();
    let names = zip_names(&out);
    assert!(names.contains("watchtower.db"), "{names:?}");
    let snaps: Vec<_> = names
        .iter()
        .filter(|n| n.starts_with(&format!("snapshots/{}/", w.id)))
        .collect();
    assert_eq!(snaps.len(), 2, "{names:?}");
    assert_eq!(report.files, 3);
    assert!(report.bytes > 0);
    assert_eq!(names.len(), 3, "nothing else is included: {names:?}");
}

#[tokio::test]
async fn backup_and_restore_round_trip_into_a_new_location() {
    let e = env().await;
    let w = seed(&e).await;
    let out = e.zip("backup.zip");
    e.store.backup_to(&out).await.unwrap();

    let fresh = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&fresh);
    let r = restore_backup(&out, &db, &snaps, &RestoreOptions::default())
        .await
        .unwrap();
    assert_eq!(r.files, 3);

    let s = Store::open(&db, snaps.clone()).await.unwrap();
    let got = s.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.title.as_deref(), Some("Item"));
    assert_eq!(s.list_tags().await.unwrap()[0].title, "Shops");
    assert_eq!(s.get_setting("answer").await.unwrap().unwrap()["x"], 42);
    assert_eq!(
        s.latest_snapshot_text(w.id).await.unwrap().as_deref(),
        Some("price: 9")
    );
    let records = s.list_snapshots(w.id).await.unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records.iter().all(|r| r.path.starts_with(&snaps)),
        "snapshot paths must point into the new location: {records:?}"
    );
}

#[tokio::test]
async fn backup_refuses_to_overwrite_and_leaves_nothing_behind_on_failure() {
    let e = env().await;
    seed(&e).await;
    let out = e.zip("backup.zip");
    std::fs::write(&out, b"precious").unwrap();
    assert!(e.store.backup_to(&out).await.is_err());
    assert_eq!(std::fs::read(&out).unwrap(), b"precious");

    let missing_dir = e.dir.path().join("no-such-dir").join("b.zip");
    assert!(e.store.backup_to(&missing_dir).await.is_err());
}

#[tokio::test]
async fn vanished_snapshot_files_are_skipped() {
    let e = env().await;
    let w = seed(&e).await;
    let first = e.store.list_snapshots(w.id).await.unwrap().remove(0);
    std::fs::remove_file(&first.path).unwrap();
    let out = e.zip("backup.zip");
    e.store.backup_to(&out).await.unwrap();
    assert_eq!(zip_names(&out).len(), 2);
}

#[tokio::test]
async fn backing_up_while_writes_continue_gives_a_consistent_database() {
    let e = env().await;
    seed(&e).await;
    let writer = {
        let store = e.store.clone();
        tokio::spawn(async move {
            for i in 0..50 {
                let w = Watch::new(Url::parse(&format!("https://example.com/{i}")).unwrap());
                store.add_watch(&w).await.unwrap();
            }
        })
    };
    let out = e.zip("live.zip");
    e.store.backup_to(&out).await.unwrap();
    writer.await.unwrap();

    let fresh = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&fresh);
    restore_backup(&out, &db, &snaps, &RestoreOptions::default())
        .await
        .unwrap();
    let s = Store::open(&db, snaps).await.unwrap();
    let n = s.list_watches().await.unwrap().len();
    assert!((1..=51).contains(&n), "{n}");
}

#[tokio::test]
async fn restore_will_not_replace_existing_data_unless_forced() {
    let e = env().await;
    seed(&e).await;
    let out = e.zip("backup.zip");
    e.store.backup_to(&out).await.unwrap();

    let fresh = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&fresh);
    let s = Store::open(&db, snaps.clone()).await.unwrap();
    s.add_watch(&Watch::new(Url::parse("https://other.example/").unwrap()))
        .await
        .unwrap();
    drop(s);

    assert!(
        restore_backup(&out, &db, &snaps, &RestoreOptions::default())
            .await
            .is_err()
    );
    let s = Store::open(&db, snaps.clone()).await.unwrap();
    assert_eq!(
        s.list_watches().await.unwrap()[0].url.as_str(),
        "https://other.example/"
    );
    s.close().await; // 接続が残っていると、Windows ではファイルを置き換えられない

    let forced = RestoreOptions {
        force: true,
        ..Default::default()
    };
    restore_backup(&out, &db, &snaps, &forced).await.unwrap();
    let s = Store::open(&db, snaps).await.unwrap();
    let urls: Vec<_> = s
        .list_watches()
        .await
        .unwrap()
        .iter()
        .map(|w| w.url.to_string())
        .collect();
    assert_eq!(urls, ["https://example.com/item"]);
}

#[tokio::test]
async fn hostile_entry_names_are_rejected_and_nothing_is_written() {
    for bad in [
        "../evil.txt",
        "snapshots/../../evil.txt",
        "snapshots/../evil",
        "/etc/evil",
        "snapshots\\..\\evil",
        "C:/evil",
        "snapshots//x",
        "snapshots/a/./b",
        "snapshots/",
        "evil.txt",
        "watchtower.db/extra",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("bad.zip");
        crafted_zip(&z, &[("watchtower.db", b"x"), (bad, b"payload")]);
        let (db, snaps) = target(&dir);
        let err = restore_backup(&z, &db, &snaps, &RestoreOptions::default()).await;
        assert!(err.is_err(), "{bad:?} must be rejected");
        assert!(!db.exists(), "{bad:?}: no database may be created");
        assert!(
            !snaps.exists() || std::fs::read_dir(&snaps).unwrap().next().is_none(),
            "{bad:?}"
        );
        let parent = dir.path().parent().unwrap();
        assert!(
            !parent.join("evil.txt").exists() && !parent.join("evil").exists(),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn archives_without_exactly_one_database_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&dir);
    let none = dir.path().join("none.zip");
    crafted_zip(&none, &[("snapshots/a/b.zst", b"x")]);
    assert!(
        restore_backup(&none, &db, &snaps, &RestoreOptions::default())
            .await
            .is_err()
    );
    // 同名エントリを 2 つ持つ zip は zip ライブラリでは作れない (書き込み側が拒否する)。
    // 他のツールで作られたものに備えた重複検査は、復元側の `validate` が行う。
    assert!(!db.exists());
}

#[tokio::test]
async fn archives_larger_than_the_limit_are_rejected_before_extraction() {
    let e = env().await;
    seed(&e).await;
    let out = e.zip("backup.zip");
    e.store.backup_to(&out).await.unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&fresh);
    let tiny = RestoreOptions {
        force: false,
        max_bytes: 10,
    };
    assert!(restore_backup(&out, &db, &snaps, &tiny).await.is_err());
    assert!(!db.exists());
}

#[tokio::test]
async fn a_corrupt_archive_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let z = dir.path().join("junk.zip");
    std::fs::write(&z, b"this is not a zip").unwrap();
    let (db, snaps) = target(&dir);
    assert!(restore_backup(&z, &db, &snaps, &RestoreOptions::default())
        .await
        .is_err());
    assert!(restore_backup(
        &dir.path().join("missing.zip"),
        &db,
        &snaps,
        &RestoreOptions::default()
    )
    .await
    .is_err());
}

/// Windows では、開いたままの DB ファイルは消せない。生の OS エラーではなく、理由が分かる文面にする。
#[cfg(windows)]
#[tokio::test]
async fn forcing_a_restore_over_an_open_database_explains_what_to_do() {
    let e = env().await;
    seed(&e).await;
    let out = e.zip("backup.zip");
    e.store.backup_to(&out).await.unwrap();

    let fresh = tempfile::tempdir().unwrap();
    let (db, snaps) = target(&fresh);
    let open = Store::open(&db, snaps.clone()).await.unwrap(); // 開いたまま
    let forced = RestoreOptions {
        force: true,
        ..Default::default()
    };
    let err = restore_backup(&out, &db, &snaps, &forced)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("still running"), "{err}");
    open.close().await;
}
