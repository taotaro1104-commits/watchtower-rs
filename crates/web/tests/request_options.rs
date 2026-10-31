//! 編集画面 (ウォッチ・タグ) で、ヘッダ・メソッド・ボディ・プロキシを設定できること。

mod common;

use axum::http::StatusCode;
use common::*;
use url::Url;
use watchtower_core::{ConfigLayer, Tag, Watch};

const BASE: &str = "url=https%3A%2F%2Fexample.com%2Fitem";

async fn post_edit(a: &App, cookie: &str, path: &str, extra: &str) -> (StatusCode, String) {
    let t = enc(&csrf(a, cookie).await);
    let r = send(a, post(path, Some(cookie), &format!("csrf={t}&{extra}"))).await;
    (r.status, r.body)
}

async fn config_of(a: &App, w: &Watch) -> ConfigLayer {
    a.store.get_watch(w.id).await.unwrap().unwrap().config
}

fn watch() -> Watch {
    Watch::new(Url::parse("https://example.com/item").unwrap())
}

#[tokio::test]
async fn request_options_are_saved_and_shown_again() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);

    let extra = format!(
        "{BASE}&method=post&headers={}&body={}&proxy={}",
        enc("X-Token: abc\nAccept-Language: ja"),
        enc("a=1&b=2"),
        enc("http://proxy.example:8080")
    );
    let (status, body) = post_edit(&a, &cookie, &path, &extra).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");

    let c = config_of(&a, &w).await;
    assert_eq!(c.method.as_deref(), Some("POST"));
    assert_eq!(
        c.headers,
        vec![
            ("X-Token".to_string(), "abc".to_string()),
            ("Accept-Language".to_string(), "ja".to_string())
        ]
    );
    assert_eq!(c.body.as_deref(), Some("a=1&b=2"));
    assert_eq!(c.proxy.as_deref(), Some("http://proxy.example:8080"));

    let page = send(&a, get(&path, Some(&cookie))).await.body;
    assert!(page.contains("X-Token: abc"), "{page}");
    assert!(page.contains("http://proxy.example:8080"), "{page}");
    assert!(page.contains("value=\"POST\" selected"), "{page}");
}

#[tokio::test]
async fn blank_fields_clear_the_options() {
    let a = app().await;
    let mut w = watch();
    w.config.method = Some("POST".into());
    w.config.headers = vec![("X-A".into(), "1".into())];
    w.config.body = Some("b".into());
    w.config.proxy = Some("http://p:1".into());
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);
    let (status, _) = post_edit(&a, &cookie, &path, BASE).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let c = config_of(&a, &w).await;
    assert!(c.method.is_none() && c.headers.is_empty() && c.body.is_none() && c.proxy.is_none());
}

#[tokio::test]
async fn invalid_options_are_rejected_without_echoing_secrets() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);

    let cases = [
        format!("{BASE}&method=CONNECT"),
        format!("{BASE}&headers={}", enc("no colon here")),
        format!("{BASE}&headers={}", enc("Host: evil.example")),
        format!("{BASE}&proxy={}", enc("ftp://user:hunter2@proxy.example")),
    ];
    for extra in cases {
        let (status, body) = post_edit(&a, &cookie, &path, &extra).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{extra}");
        assert!(!body.contains("hunter2"), "{body}");
    }
    let c = config_of(&a, &w).await;
    assert!(c.method.is_none() && c.headers.is_empty() && c.proxy.is_none());
}

#[tokio::test]
async fn tags_can_carry_request_options_too() {
    let a = app().await;
    let t = Tag::new("shop");
    a.store.add_tag(&t).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/tags/{}/edit", t.id);
    let extra = format!("title=shop&headers={}", enc("X-Shop: 1"));
    let (status, body) = post_edit(&a, &cookie, &path, &extra).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let tags = a.store.list_tags().await.unwrap();
    assert_eq!(
        tags[0].config.headers,
        vec![("X-Shop".to_string(), "1".to_string())]
    );
}

#[tokio::test]
async fn notification_templates_are_saved_shown_again_and_validated() {
    let a = app().await;
    let w = watch();
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let path = format!("/watches/{}/edit", w.id);

    let extra = format!(
        "{BASE}&notification_title={}&notification_body={}",
        enc("Changed: {{ watch_title }}"),
        enc("{{ diff }}")
    );
    let (status, body) = post_edit(&a, &cookie, &path, &extra).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let c = config_of(&a, &w).await;
    assert_eq!(
        c.notification_title.as_deref(),
        Some("Changed: {{ watch_title }}")
    );
    assert_eq!(c.notification_body.as_deref(), Some("{{ diff }}"));

    let page = send(&a, get(&path, Some(&cookie))).await.body;
    assert!(page.contains("name=\"notification_title\""), "{page}");
    assert!(
        page.contains("Changed: {{ watch_title }}") || page.contains("Changed: &#123;&#123;"),
        "{page}"
    );

    // 構文の誤り・未定義の変数は保存できず、元の設定が残る
    for bad in ["{% for %}", "{{ no_such_variable }}"] {
        let extra = format!("{BASE}&notification_body={}", enc(bad));
        let (status, _) = post_edit(&a, &cookie, &path, &extra).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(
        config_of(&a, &w).await.notification_body.as_deref(),
        Some("{{ diff }}")
    );

    // 空欄で既定に戻る
    let (status, _) = post_edit(&a, &cookie, &path, BASE).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let c = config_of(&a, &w).await;
    assert!(c.notification_title.is_none() && c.notification_body.is_none());
}
