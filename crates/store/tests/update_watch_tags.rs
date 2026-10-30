//! タグ削除と編集が競合しても、存在しないタグ ID が残らないこと。

use url::Url;
use uuid::Uuid;
use watchtower_core::{Tag, Watch};
use watchtower_store::Store;

#[tokio::test]
async fn update_watch_drops_tag_ids_that_no_longer_exist() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let alive = Tag::new("alive");
    s.add_tag(&alive).await.unwrap();
    let mut w = Watch::new(Url::parse("https://example.com/").unwrap());
    s.add_watch(&w).await.unwrap();

    // 編集フォームが検証したあとにタグが消えた、という状況を模す
    w.tags = vec![alive.id, Uuid::new_v4()];
    assert!(s.update_watch(&w).await.unwrap());
    assert_eq!(
        s.get_watch(w.id).await.unwrap().unwrap().tags,
        vec![alive.id]
    );
}
