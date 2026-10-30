//! 全ウォッチ共通の通知先 (settings に保存)。

use watchtower_store::Store;

#[tokio::test]
async fn global_notify_urls_default_to_empty_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    assert!(s.global_notify_urls().await.unwrap().is_empty());
    let urls = vec!["json://a/x".to_string(), "discord://1/t".to_string()];
    s.set_global_notify_urls(&urls).await.unwrap();
    assert_eq!(s.global_notify_urls().await.unwrap(), urls);
    s.set_global_notify_urls(&[]).await.unwrap();
    assert!(s.global_notify_urls().await.unwrap().is_empty());
}
