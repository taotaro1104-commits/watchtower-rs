//! ウォッチとタグの編集画面で、取得方法 (継承 / ブラウザ / 通常の HTTP) を選べること。

mod common;

use axum::http::StatusCode;
use common::*;
use url::Url;
use watchtower_core::{Tag, Watch};

async fn post_edit(a: &App, cookie: &str, path: &str, extra: &str) -> StatusCode {
    let t = enc(&csrf(a, cookie).await);
    send(a, post(path, Some(cookie), &format!("csrf={t}&{extra}")))
        .await
        .status
}

fn watch() -> Watch {
    Watch::new(Url::parse("https://example.com/item").unwrap())
}

const BASE: &str = "url=https%3A%2F%2Fexample.com%2Fitem";

#[tokio::test]
async fn a_watch_can_choose_browser_http_or_inherit() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let current = || async {
        a.store
            .get_watch(w.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .use_browser
    };

    for (value, want) in [
        ("html_webdriver", Some(true)),
        ("html_requests", Some(false)),
        ("system", None),
        ("html_webdriver", Some(true)),
        ("", None),
    ] {
        let status = post_edit(&a, &cookie, &path, &format!("{BASE}&fetch_backend={value}")).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{value}");
        assert_eq!(current().await, want, "{value}");
    }
}

#[tokio::test]
async fn the_edit_page_shows_the_current_choice() {
    let a = app().await;
    let mut w = watch();
    w.config.use_browser = Some(true);
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let page = send(&a, get(&format!("/watches/{}/edit", w.id), Some(&cookie))).await;
    assert!(
        page.body.contains("name=\"fetch_backend\""),
        "{}",
        page.body
    );
    assert!(
        page.body.contains("value=\"html_webdriver\" selected"),
        "the browser option must be selected: {}",
        page.body
    );
}

#[tokio::test]
async fn unknown_choices_are_rejected_without_changes() {
    let a = app().await;
    let mut w = watch();
    w.config.use_browser = Some(true);
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    for bad in ["chrome", "javascript:1", "HTML_WEBDRIVER"] {
        let status = post_edit(
            &a,
            &cookie,
            &path,
            &format!("{BASE}&fetch_backend={}", enc(bad)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(
        a.store
            .get_watch(w.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .use_browser,
        Some(true)
    );
}

#[tokio::test]
async fn a_tag_can_choose_the_browser_for_all_its_watches() {
    let a = app().await;
    let tag = Tag::new("js-heavy");
    a.store.add_tag(&tag).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/tags/{}/edit", tag.id);
    assert_eq!(
        post_edit(
            &a,
            &cookie,
            &path,
            "title=js-heavy&fetch_backend=html_webdriver"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        a.store
            .get_tag(tag.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .use_browser,
        Some(true)
    );
    let page = send(&a, get(&path, Some(&cookie))).await;
    assert!(
        page.body.contains("value=\"html_webdriver\" selected"),
        "{}",
        page.body
    );
}
