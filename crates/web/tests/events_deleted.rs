//! 削除済みウォッチの履歴は、空のリンクではなく目に見える表示になること。

mod common;

use common::*;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn events_of_deleted_watches_show_a_placeholder_instead_of_an_empty_link() {
    let a = app().await;
    let gone = Uuid::new_v4();
    let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .record_event(Some(gone), "changed", t, None)
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get("/events", Some(&cookie))).await;
    assert!(r.body.contains("deleted watch"), "{}", r.body);
    assert!(
        !r.body.contains(&format!("/watches/{gone}")),
        "no dead link"
    );
}
