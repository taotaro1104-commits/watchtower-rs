use time::OffsetDateTime;
use url::Url;
use watchtower_core::{Watch, WatchState};
use watchtower_store::Store;

async fn open() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("wt.db"), dir.path().join("snapshots"))
        .await
        .unwrap();
    (store, dir)
}

fn watch(u: &str) -> Watch {
    Watch::new(Url::parse(u).unwrap())
}

fn ts(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000 + n).unwrap()
}

fn hash(s: &str) -> [u8; 32] {
    [s.as_bytes()[0]; 32]
}

#[tokio::test]
async fn watch_crud_roundtrip() {
    let (s, _d) = open().await;
    let mut w = watch("https://example.com/a");
    w.title = Some("A".into());
    w.config.ignore_text = vec!["x".into()];
    s.add_watch(&w).await.unwrap();
    assert_eq!(s.get_watch(w.id).await.unwrap(), Some(w.clone()));
    assert_eq!(s.list_watches().await.unwrap().len(), 1);
    s.delete_watch(w.id).await.unwrap();
    assert_eq!(s.get_watch(w.id).await.unwrap(), None);
}

#[tokio::test]
async fn state_update_is_persisted() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    let st = WatchState {
        next_check_at: Some(ts(60)),
        last_checked_at: Some(ts(0)),
        last_changed_at: None,
        last_error: Some("boom".into()),
        consecutive_errors: 2,
    };
    s.update_state(w.id, &st).await.unwrap();
    assert_eq!(s.get_watch(w.id).await.unwrap().unwrap().state, st);
}

#[tokio::test]
async fn identical_hash_is_not_saved_twice() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    assert!(s
        .save_snapshot(w.id, ts(0), "one", hash("one"))
        .await
        .unwrap()
        .is_some());
    assert!(s
        .save_snapshot(w.id, ts(1), "one", hash("one"))
        .await
        .unwrap()
        .is_none());
    assert!(s
        .save_snapshot(w.id, ts(2), "two", hash("two"))
        .await
        .unwrap()
        .is_some());
    assert_eq!(s.list_snapshots(w.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn snapshot_text_roundtrips_through_zstd() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    let text = "日本語\nline2";
    s.save_snapshot(w.id, ts(0), text, hash("a")).await.unwrap();
    assert_eq!(
        s.latest_snapshot_text(w.id).await.unwrap().as_deref(),
        Some(text)
    );
    let snaps = s.list_snapshots(w.id).await.unwrap();
    assert_eq!(s.read_snapshot(&snaps[0]).await.unwrap(), text);
}

#[tokio::test]
async fn gc_keeps_newest_and_removes_files() {
    let (s, d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    for (i, c) in ["a", "b", "c", "d"].iter().enumerate() {
        s.save_snapshot(w.id, ts(i as i64), c, hash(c))
            .await
            .unwrap();
    }
    assert_eq!(s.gc_snapshots(w.id, 2).await.unwrap(), 2);
    let snaps = s.list_snapshots(w.id).await.unwrap();
    assert_eq!(snaps.len(), 2);
    assert_eq!(
        s.latest_snapshot_text(w.id).await.unwrap().as_deref(),
        Some("d")
    );
    let files = std::fs::read_dir(d.path().join("snapshots").join(w.id.to_string()))
        .unwrap()
        .count();
    assert_eq!(files, 2);
}

#[tokio::test]
async fn same_second_snapshots_get_distinct_files_and_gc_keeps_survivors_readable() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    // A, B, A を同じ秒に保存しても、それぞれ別ファイルになる
    for c in ["A", "B", "A"] {
        s.save_snapshot(w.id, ts(0), c, hash(c))
            .await
            .unwrap()
            .unwrap();
    }
    let snaps = s.list_snapshots(w.id).await.unwrap();
    let paths: std::collections::HashSet<_> = snaps.iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths.len(), 3);
    s.gc_snapshots(w.id, 1).await.unwrap();
    assert_eq!(
        s.latest_snapshot_text(w.id).await.unwrap().as_deref(),
        Some("A")
    );
}

#[tokio::test]
async fn latest_is_by_insertion_order_even_if_the_clock_steps_back() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    s.save_snapshot(w.id, ts(100), "first", hash("first"))
        .await
        .unwrap();
    s.save_snapshot(w.id, ts(0), "second", hash("second"))
        .await
        .unwrap();
    assert_eq!(
        s.latest_snapshot(w.id).await.unwrap().unwrap().hash,
        hash("second")
    );
    assert_eq!(
        s.latest_snapshot_text(w.id).await.unwrap().as_deref(),
        Some("second")
    );
    // 時刻が戻っても、直近と同じ内容は重複保存しない
    assert!(s
        .save_snapshot(w.id, ts(1), "second", hash("second"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn failed_insert_leaves_no_orphan_file() {
    let (s, d) = open().await;
    let ghost = watch("https://example.com/never-added");
    assert!(s
        .save_snapshot(ghost.id, ts(0), "x", hash("x"))
        .await
        .is_err());
    let dir = d.path().join("snapshots").join(ghost.id.to_string());
    let leftovers = std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0);
    assert_eq!(leftovers, 0);
}

#[tokio::test]
async fn drop_snapshot_removes_row_and_file() {
    let (s, _d) = open().await;
    let w = watch("https://example.com/");
    s.add_watch(&w).await.unwrap();
    let rec = s
        .save_snapshot(w.id, ts(0), "x", hash("x"))
        .await
        .unwrap()
        .unwrap();
    s.drop_snapshot(&rec).await.unwrap();
    assert!(s.list_snapshots(w.id).await.unwrap().is_empty());
    assert!(!rec.path.exists());
    s.drop_snapshot(&rec).await.unwrap(); // 二重削除してもエラーにならない
}
