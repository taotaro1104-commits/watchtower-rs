//! スケジューラとの連携と、終了時の挙動。

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use http_body_util::BodyExt;
use tokio::sync::mpsc;
use tower::ServiceExt;
use watchtower_scheduler::Command;
use watchtower_store::Store;
use watchtower_web::{router, AppState, WebConfig};

async fn app_with_scheduler(capacity: usize) -> (App, mpsc::Receiver<Command>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let (tx, rx) = mpsc::channel(capacity);
    let cfg = WebConfig {
        sse_poll: Duration::from_millis(40),
        ..Default::default()
    };
    let state = AppState::with_scheduler(store.clone(), cfg, Some(tx));
    state.set_password(PASSWORD).await.unwrap();
    let app = App {
        router: router(state.clone()),
        state,
        store,
        _dir: dir,
    };
    (app, rx)
}

fn multipart(csrf: &str, json: &str) -> axum::http::Request<Body> {
    let b = "XBOUNDARYX";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.json\"\r\n\
         Content-Type: application/json\r\n\r\n{json}\r\n--{b}--\r\n"
    );
    axum::http::Request::builder()
        .method("POST")
        .uri("/settings/import")
        .header(
            axum::http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={b}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn imported_watches_are_scheduled_without_dropping_any_command() {
    // 取り込み件数 (40) が通知用チャネルの容量 (4) を大きく超えても、取りこぼさない
    let (a, mut rx) = app_with_scheduler(4).await;
    let consumer = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(Command::Upsert(id)) = rx.recv().await {
            seen.push(id);
            tokio::time::sleep(Duration::from_millis(5)).await; // 少し遅い消費側
        }
        seen
    });
    let cookie = login(&a).await;
    let t = csrf(&a, &cookie).await;
    let entries: Vec<String> = (0..40)
        .map(|i| {
            format!(
                r#""{:08x}-0000-4000-8000-000000000000": {{"url": "https://example.com/{i}"}}"#,
                i + 1
            )
        })
        .collect();
    let json = format!(r#"{{"watching": {{{}}}}}"#, entries.join(","));
    let mut req = multipart(&t, &json);
    req.headers_mut()
        .insert(axum::http::header::COOKIE, cookie.parse().unwrap());
    let r = a.router.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    drop(a); // 送信側を閉じて消費側を終わらせる
    let seen = tokio::time::timeout(Duration::from_secs(10), consumer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        seen.len(),
        40,
        "every imported watch must reach the scheduler"
    );
}

#[tokio::test]
async fn shutdown_closes_open_event_streams() {
    let (a, _rx) = app_with_scheduler(4).await;
    let cookie = login(&a).await;
    let res = a
        .router
        .clone()
        .oneshot(get("/events/stream", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();

    a.state.shutdown();
    let end = tokio::time::timeout(Duration::from_secs(3), body.frame()).await;
    assert!(
        matches!(end, Ok(None)),
        "the stream must end on shutdown (otherwise graceful shutdown hangs): {end:?}"
    );
}
