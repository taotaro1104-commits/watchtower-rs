//! インポートの上限と、取り込んだ ID の報告 (スケジューラへ伝えるために必要)。

use watchtower_store::Store;

async fn open() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    (store, dir)
}

fn entry(i: usize, extra: &str) -> String {
    format!(
        r#""{:08x}-0000-4000-8000-000000000000": {{"url": "https://example.com/{i}"{extra}}}"#,
        i + 1
    )
}

fn file(entries: &[String]) -> String {
    format!(r#"{{"watching": {{{}}}}}"#, entries.join(","))
}

#[tokio::test]
async fn imported_ids_are_reported() {
    let (s, _d) = open().await;
    let r = s
        .import_changedetection(&file(&[entry(1, ""), entry(2, "")]))
        .await
        .unwrap();
    let mut stored: Vec<_> = s
        .list_watches()
        .await
        .unwrap()
        .into_iter()
        .map(|w| w.id)
        .collect();
    let mut reported = r.ids.clone();
    stored.sort();
    reported.sort();
    assert_eq!(r.imported, 2);
    assert_eq!(reported, stored);
}

#[tokio::test]
async fn skipped_entries_are_not_reported_as_ids() {
    let (s, _d) = open().await;
    let r = s
        .import_changedetection(&file(&[
            entry(1, ""),
            r#""not-a-uuid": {"url": "ftp://x"}"#.to_string(),
        ]))
        .await
        .unwrap();
    assert_eq!(r.ids.len(), 1);
    // 2 回目は既存 ID なので、何も報告されない
    let again = s
        .import_changedetection(&file(&[entry(1, "")]))
        .await
        .unwrap();
    assert!(again.ids.is_empty());
}

#[tokio::test]
async fn entries_violating_the_form_limits_are_skipped_with_a_reason() {
    let (s, _d) = open().await;
    let long_url = format!(
        r#""00000001-0000-4000-8000-000000000001": {{"url": "https://e.com/{}"}}"#,
        "a".repeat(3000)
    );
    let long_title = format!(
        r#""00000002-0000-4000-8000-000000000002": {{"url": "https://e.com/t", "title": "{}"}}"#,
        "t".repeat(300)
    );
    let many_lines = format!(
        r#""00000003-0000-4000-8000-000000000003": {{"url": "https://e.com/l", "ignore_text": [{}]}}"#,
        (0..60)
            .map(|i| format!("\"x{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let long_line = format!(
        r#""00000004-0000-4000-8000-000000000004": {{"url": "https://e.com/c", "include_filters": ["{}"]}}"#,
        "s".repeat(600)
    );
    let r = s
        .import_changedetection(&file(&[
            long_url,
            long_title,
            many_lines,
            long_line,
            entry(9, ""),
        ]))
        .await
        .unwrap();
    assert_eq!(r.imported, 1, "{r:?}");
    assert_eq!(r.skipped.len(), 4, "{r:?}");
    assert!(
        r.skipped
            .iter()
            .all(|s| s.contains("too long") || s.contains("too many")),
        "{:?}",
        r.skipped
    );
}

#[tokio::test]
async fn files_with_too_many_watches_are_refused_up_front() {
    let (s, _d) = open().await;
    let entries: Vec<String> = (0..5001).map(|i| entry(i, "")).collect();
    let err = s.import_changedetection(&file(&entries)).await.unwrap_err();
    assert!(err.to_string().contains("5000"), "{err}");
    assert!(
        s.list_watches().await.unwrap().is_empty(),
        "nothing is imported"
    );
}
