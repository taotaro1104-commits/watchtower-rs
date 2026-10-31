//! 編集画面 (ウォッチ・タグ) で、ブラウザ操作ステップをテキストで設定できること。

mod common;

use axum::http::StatusCode;
use common::*;
use url::Url;
use watchtower_core::{BrowserStep, Tag, Watch};

const BASE: &str = "url=https%3A%2F%2Fexample.com%2Fitem&fetch_backend=html_webdriver";

async fn post_edit(a: &App, cookie: &str, path: &str, extra: &str) -> StatusCode {
    let t = enc(&csrf(a, cookie).await);
    send(a, post(path, Some(cookie), &format!("csrf={t}&{extra}")))
        .await
        .status
}

async fn steps_of(a: &App, w: &Watch) -> Vec<BrowserStep> {
    a.store
        .get_watch(w.id)
        .await
        .unwrap()
        .unwrap()
        .config
        .browser_steps
}

fn watch() -> Watch {
    Watch::new(Url::parse("https://example.com/item").unwrap())
}

#[tokio::test]
async fn steps_typed_into_the_form_are_saved_and_shown_again() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);

    let text = "click #accept\ntype #q = hello world\nwait_for .result @3000\nwait 200";
    let status = post_edit(
        &a,
        &cookie,
        &path,
        &format!("{BASE}&browser_steps={}", enc(text)),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(steps_of(&a, &w).await.len(), 4);

    let page = send(&a, get(&path, Some(&cookie))).await;
    assert!(
        page.body.contains("name=\"browser_steps\""),
        "{}",
        page.body
    );
    assert!(page.body.contains("click #accept"), "{}", page.body);
    assert!(page.body.contains("type #q = hello world"), "{}", page.body);

    // 空欄にすると解除される
    let status = post_edit(&a, &cookie, &path, &format!("{BASE}&browser_steps=")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(steps_of(&a, &w).await.is_empty());
}

#[tokio::test]
async fn bad_steps_are_rejected_with_the_line_and_change_nothing() {
    let a = app().await;
    let mut w = watch();
    w.config.browser_steps = vec![BrowserStep::Wait { ms: 5 }];
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);

    let t = enc(&csrf(&a, &cookie).await);
    let bad = enc("click #ok\njump #nope");
    let r = send(
        &a,
        post(
            &path,
            Some(&cookie),
            &format!("csrf={t}&{BASE}&browser_steps={bad}"),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("line 2"), "{}", r.body);
    assert_eq!(steps_of(&a, &w).await, vec![BrowserStep::Wait { ms: 5 }]);

    let secret = enc("type #p = hunter2-SECRET-TEXT");
    let t = enc(&csrf(&a, &cookie).await);
    let r = send(
        &a,
        post(
            &path,
            Some(&cookie),
            &format!("csrf={t}&{BASE}&browser_steps={secret}&notify_urls=bad"),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!r.body.contains("SECRET"), "{}", r.body);
}

#[tokio::test]
async fn steps_without_the_browser_fetch_method_are_refused() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let status = post_edit(
        &a,
        &cookie,
        &path,
        &format!(
            "url=https%3A%2F%2Fexample.com%2Fitem&fetch_backend=html_requests&browser_steps={}",
            enc("click #a")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(steps_of(&a, &w).await.is_empty());
}

#[tokio::test]
async fn a_tag_can_carry_steps_for_all_its_watches() {
    let a = app().await;
    let tag = Tag::new("login-needed");
    a.store.add_tag(&tag).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/tags/{}/edit", tag.id);
    let status = post_edit(
        &a,
        &cookie,
        &path,
        &format!(
            "title=login-needed&fetch_backend=html_webdriver&browser_steps={}",
            enc("click #cookie-ok")
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        a.store
            .get_tag(tag.id)
            .await
            .unwrap()
            .unwrap()
            .config
            .browser_steps
            .len(),
        1
    );
    let page = send(&a, get(&path, Some(&cookie))).await;
    assert!(page.body.contains("click #cookie-ok"), "{}", page.body);
}
