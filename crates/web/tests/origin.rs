//! ログインの同一オリジン検査。実ブラウザで見つかった不具合の回帰テストを含む。
//!
//! `Referrer-Policy: no-referrer` だと、ブラウザは同一オリジンのフォーム POST でも
//! `Origin: null` を送る。その結果、自分自身のログインフォームが拒否されていた。

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;

fn login_with(headers: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::HOST, "127.0.0.1:5077");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Body::from(format!("password={}", enc(PASSWORD))))
        .unwrap()
}

async fn status(a: &App, req: Request<Body>) -> StatusCode {
    send(a, req).await.status
}

#[tokio::test]
async fn referrer_policy_keeps_same_origin_posts_identifiable() {
    let a = app().await;
    let r = send(&a, get("/login", None)).await;
    let policy = r.headers.get("referrer-policy").unwrap().to_str().unwrap();
    // no-referrer だとブラウザは同一オリジンの POST でも Origin: null を送ってしまう
    assert_eq!(policy, "same-origin");
}

#[tokio::test]
async fn a_browser_style_same_origin_login_succeeds() {
    let a = app().await;
    let req = login_with(&[
        ("origin", "http://127.0.0.1:5077"),
        ("sec-fetch-site", "same-origin"),
        ("sec-fetch-mode", "navigate"),
    ]);
    assert_eq!(status(&a, req).await, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn null_and_foreign_origins_are_refused() {
    let a = app().await;
    for origin in ["null", "https://evil.example", "http://127.0.0.1:9999"] {
        let req = login_with(&[("origin", origin)]);
        assert_eq!(status(&a, req).await, StatusCode::FORBIDDEN, "{origin}");
    }
}

#[tokio::test]
async fn fetch_metadata_marks_cross_site_requests() {
    let a = app().await;
    for site in ["cross-site", "same-site"] {
        // Origin ヘッダが落とされていても、Sec-Fetch-Site で弾く
        let req = login_with(&[("sec-fetch-site", site)]);
        assert_eq!(status(&a, req).await, StatusCode::FORBIDDEN, "{site}");
    }
    // ユーザー自身の操作 (アドレス欄からの移動など) は "none"
    let req = login_with(&[("sec-fetch-site", "none")]);
    assert_eq!(status(&a, req).await, StatusCode::SEE_OTHER);
}
