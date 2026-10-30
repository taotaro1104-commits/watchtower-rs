//! 設定画面からのバックアップのダウンロード。

mod common;

use std::io::{Cursor, Read};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use http_body_util::BodyExt;
use tower::ServiceExt;
use url::Url;
use watchtower_core::Watch;

async fn download(
    a: &App,
    cookie: &str,
    form: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/settings/backup")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .body(Body::from(form.to_string()))
        .unwrap();
    let res = a.router.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    (
        parts.status,
        parts.headers,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

#[tokio::test]
async fn the_backup_is_a_zip_attachment_with_the_database() {
    let a = app().await;
    a.store
        .add_watch(&Watch::new(Url::parse("https://example.com/").unwrap()))
        .await
        .unwrap();
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);
    let (status, headers, body) = download(&a, &cookie, &format!("csrf={t}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/zip");
    let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"watchtower-backup-"),
        "{disposition}"
    );
    assert!(disposition.ends_with(".zip\""), "{disposition}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");

    let mut z = zip::ZipArchive::new(Cursor::new(body)).unwrap();
    let mut db = z.by_name("watchtower.db").unwrap();
    let mut head = [0u8; 15];
    db.read_exact(&mut head).unwrap();
    assert_eq!(
        &head, b"SQLite format 3",
        "the entry is a real SQLite database"
    );
}

#[tokio::test]
async fn downloading_needs_login_and_a_csrf_token() {
    let a = app().await;
    let cookie = login(&a).await;
    let anonymous = a
        .router
        .clone()
        .oneshot(post("/settings/backup", None, "csrf=x"))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);

    let (status, _, body) = download(&a, &cookie, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.starts_with(b"PK"));
    let (status, _, _) = download(&a, &cookie, "csrf=wrong").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_get_does_not_start_a_backup() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = send(&a, get("/settings/backup", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn the_settings_page_offers_the_download() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = send(&a, get("/settings", Some(&cookie))).await;
    assert!(r.body.contains("action=\"/settings/backup\""), "{}", r.body);
}

#[tokio::test]
async fn no_temporary_files_are_left_behind() {
    let a = app().await;
    let cookie = login(&a).await;
    let t = enc(&csrf(&a, &cookie).await);
    let before = std::fs::read_dir(std::env::temp_dir()).unwrap().count();
    let (status, _, _) = download(&a, &cookie, &format!("csrf={t}")).await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let after = std::fs::read_dir(std::env::temp_dir()).unwrap().count();
    assert!(
        after <= before + 1,
        "temp dir grew from {before} to {after}"
    );
}
