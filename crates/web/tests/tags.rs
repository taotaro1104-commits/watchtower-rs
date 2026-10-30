//! タグの管理画面と、ウォッチへのタグ割り当て。

mod common;

use axum::http::StatusCode;
use common::*;
use url::Url;
use watchtower_core::{Tag, Watch};

fn watch(u: &str) -> Watch {
    Watch::new(Url::parse(u).unwrap())
}

#[tokio::test]
async fn tags_page_requires_login_and_lists_tags_with_counts_escaped() {
    let a = app().await;
    assert_eq!(
        send(&a, get("/tags", None)).await.status,
        StatusCode::SEE_OTHER
    );

    let tag = Tag::new("<i>Shops</i>");
    a.store.add_tag(&tag).await.unwrap();
    let mut w = watch("https://example.com/");
    w.tags = vec![tag.id];
    a.store.add_watch(&w).await.unwrap();

    let cookie = login(&a).await;
    let r = send(&a, get("/tags", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(is_escaped(&r.body, "i"), "{}", r.body);
    assert!(!r.body.contains("<i>Shops</i>"));
    assert!(r.body.contains(&format!("/tags/{}/edit", tag.id)));
    assert!(r.body.contains("1 watch"), "{}", r.body);
}

#[tokio::test]
async fn creating_a_tag_validates_title_and_csrf() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);

    let forged = send(&a, post("/tags", Some(&cookie), "title=Shops")).await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    for bad in [
        format!("csrf={t}&title="),
        format!("csrf={t}&title=%20%20"),
        format!("csrf={t}&title={}", "x".repeat(101)),
    ] {
        assert_eq!(
            send(&a, post("/tags", Some(&cookie), &bad)).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(a.store.list_tags().await.unwrap().is_empty());

    let ok = send(
        &a,
        post("/tags", Some(&cookie), &format!("csrf={t}&title=Shops")),
    )
    .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(a.store.list_tags().await.unwrap()[0].title, "Shops");
}

#[tokio::test]
async fn editing_a_tag_updates_title_and_shared_settings() {
    let a = app().await;
    let tag = Tag::new("Old");
    a.store.add_tag(&tag).await.unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);

    let page = send(&a, get(&format!("/tags/{}/edit", tag.id), Some(&cookie))).await;
    assert_eq!(page.status, StatusCode::OK);

    let form = format!(
        "csrf={t}&title=New&interval=600&notify_muted=on&include_filters={}&exclude_filters=.ad\
         &ignore_text=cookie&trigger_text=sold+out&notify_urls={}",
        enc(".price\r\n#main"),
        enc("json://h.example.com/x")
    );
    let r = send(
        &a,
        post(&format!("/tags/{}/edit", tag.id), Some(&cookie), &form),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);

    let got = a.store.get_tag(tag.id).await.unwrap().unwrap();
    assert_eq!(got.title, "New");
    assert_eq!(got.config.check_interval_secs, Some(600));
    assert_eq!(got.config.notify_muted, Some(true));
    assert_eq!(got.config.include_filters, vec![".price", "#main"]);
    assert_eq!(got.config.exclude_filters, vec![".ad"]);
    assert_eq!(got.config.ignore_text, vec!["cookie"]);
    assert_eq!(got.config.trigger_text, vec!["sold out"]);
    assert_eq!(got.config.notify_urls, vec!["json://h.example.com/x"]);
}

#[tokio::test]
async fn invalid_tag_edits_change_nothing_and_do_not_echo_secrets() {
    let a = app().await;
    let tag = Tag::new("Keep");
    a.store.add_tag(&tag).await.unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);
    let path = format!("/tags/{}/edit", tag.id);
    for extra in [
        "interval=0".to_string(),
        format!("notify_urls={}", enc("bogus://SECRETtoken/x")),
        "title=".to_string(),
    ] {
        let body = if extra.starts_with("title=") {
            format!("csrf={t}&{extra}")
        } else {
            format!("csrf={t}&title=Changed&{extra}")
        };
        let r = send(&a, post(&path, Some(&cookie), &body)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{extra}");
        assert!(!r.body.contains("SECRET"));
    }
    assert_eq!(a.store.get_tag(tag.id).await.unwrap().unwrap(), tag);
}

#[tokio::test]
async fn deleting_a_tag_detaches_it_from_watches() {
    let a = app().await;
    let tag = Tag::new("Gone");
    a.store.add_tag(&tag).await.unwrap();
    let mut w = watch("https://example.com/");
    w.tags = vec![tag.id];
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);
    let path = format!("/tags/{}/delete", tag.id);

    assert_eq!(
        send(&a, post(&path, Some(&cookie), "")).await.status,
        StatusCode::FORBIDDEN
    );
    let r = send(&a, post(&path, Some(&cookie), &format!("csrf={t}"))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(a.store.list_tags().await.unwrap().is_empty());
    assert!(a
        .store
        .get_watch(w.id)
        .await
        .unwrap()
        .unwrap()
        .tags
        .is_empty());
}

#[tokio::test]
async fn unknown_tag_ids_are_404() {
    let a = app().await;
    let cookie = login(&a).await;
    for p in [
        "/tags/00000000-0000-0000-0000-000000000000/edit",
        "/tags/not-a-uuid/edit",
    ] {
        assert_eq!(
            send(&a, get(p, Some(&cookie))).await.status,
            StatusCode::NOT_FOUND,
            "{p}"
        );
    }
}

#[tokio::test]
async fn watches_can_be_assigned_tags_from_the_edit_form() {
    let a = app().await;
    let (t1, t2) = (Tag::new("One"), Tag::new("Two"));
    a.store.add_tag(&t1).await.unwrap();
    a.store.add_tag(&t2).await.unwrap();
    let w = watch("https://example.com/a");
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);

    let form = format!(
        "csrf={t}&url=https%3A%2F%2Fexample.com%2Fa&tags={}&tags={}",
        t2.id, t1.id
    );
    let r = send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &form),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(
        a.store.get_watch(w.id).await.unwrap().unwrap().tags,
        vec![t2.id, t1.id]
    );

    // 編集画面ではチェック済みで表示される
    let page = send(&a, get(&format!("/watches/{}/edit", w.id), Some(&cookie))).await;
    assert!(
        page.body.contains(&format!("value=\"{}\" checked", t1.id)),
        "{}",
        page.body
    );

    // 外すと空になる
    let none = format!("csrf={t}&url=https%3A%2F%2Fexample.com%2Fa");
    send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &none),
    )
    .await;
    assert!(a
        .store
        .get_watch(w.id)
        .await
        .unwrap()
        .unwrap()
        .tags
        .is_empty());
}

#[tokio::test]
async fn assigning_unknown_tags_is_rejected() {
    let a = app().await;
    let w = watch("https://example.com/a");
    a.store.add_watch(&w).await.unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);
    let form = format!(
        "csrf={t}&url=https%3A%2F%2Fexample.com%2Fa&tags=00000000-0000-0000-0000-000000000000"
    );
    let r = send(
        &a,
        post(&format!("/watches/{}/edit", w.id), Some(&cookie), &form),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
