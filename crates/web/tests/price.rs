//! 価格の閾値 (`price_below`) をウォッチとタグの編集画面から設定できること。

mod common;

use axum::http::StatusCode;
use common::*;
use url::Url;
use watchtower_core::{Tag, Watch};

fn watch() -> Watch {
    Watch::new(Url::parse("https://example.com/item").unwrap())
}

async fn post_edit(a: &App, cookie: &str, path: &str, extra: &str) -> StatusCode {
    let t = enc(&csrf(a, cookie).await);
    let body = format!("csrf={t}&{extra}");
    send(a, post(path, Some(cookie), &body)).await.status
}

#[tokio::test]
async fn a_watch_can_be_given_and_relieved_of_a_threshold() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let base = "url=https%3A%2F%2Fexample.com%2Fitem";

    assert_eq!(
        post_edit(&a, &cookie, &path, &format!("{base}&price_below=19.99")).await,
        StatusCode::SEE_OTHER
    );
    let got = a.store.get_watch(w.id).await.unwrap().unwrap();
    assert_eq!(got.config.price_below, Some(19.99));

    // 編集画面に現在値が出る
    let page = send(&a, get(&path, Some(&cookie))).await;
    assert!(page.body.contains("name=\"price_below\""), "{}", page.body);
    assert!(page.body.contains("value=\"19.99\""), "{}", page.body);

    // 空欄で解除
    assert_eq!(
        post_edit(&a, &cookie, &path, &format!("{base}&price_below=")).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        a.store
            .get_watch(w.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .price_below,
        None
    );
}

#[tokio::test]
async fn zero_is_accepted_and_nonsense_is_rejected_without_changes() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let base = "url=https%3A%2F%2Fexample.com%2Fitem";

    assert_eq!(
        post_edit(&a, &cookie, &path, &format!("{base}&price_below=0")).await,
        StatusCode::SEE_OTHER
    );
    for bad in ["abc", "-1", "NaN", "inf", "1e99", "1,5", "12abc"] {
        assert_eq!(
            post_edit(
                &a,
                &cookie,
                &path,
                &format!("{base}&price_below={}", enc(bad))
            )
            .await,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(
        a.store
            .get_watch(w.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .price_below,
        Some(0.0),
        "the last valid value stays"
    );
}

#[tokio::test]
async fn a_tag_can_carry_a_threshold() {
    let a = app().await;
    let tag = Tag::new("deals");
    a.store.add_tag(&tag).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/tags/{}/edit", tag.id);
    assert_eq!(
        post_edit(&a, &cookie, &path, "title=deals&price_below=15.5").await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        a.store
            .get_tag(tag.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .price_below,
        Some(15.5)
    );
    let page = send(&a, get(&path, Some(&cookie))).await;
    assert!(page.body.contains("value=\"15.5\""), "{}", page.body);
    assert_eq!(
        post_edit(&a, &cookie, &path, "title=deals&price_below=bad").await,
        StatusCode::BAD_REQUEST
    );
}
