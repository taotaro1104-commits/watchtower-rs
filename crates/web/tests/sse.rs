//! SSE (`/events/stream`): 履歴が増えたことを画面に伝える。

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, StatusCode};
use common::*;
use http_body_util::BodyExt;
use time::OffsetDateTime;
use tower::ServiceExt;
use watchtower_web::WebConfig;

fn fast() -> WebConfig {
    WebConfig {
        sse_poll: Duration::from_millis(40),
        ..Default::default()
    }
}

async fn open_stream(a: &App, cookie: &str) -> axum::http::Response<Body> {
    a.router
        .clone()
        .oneshot(get("/events/stream", Some(cookie)))
        .await
        .unwrap()
}

/// 次のフレーム (テキスト) を `wait` だけ待つ。来なければ None、終了したら Some("")。
async fn next_text(body: &mut Body, wait: Duration) -> Option<String> {
    match tokio::time::timeout(wait, body.frame()).await {
        Err(_) => None,
        Ok(None) => Some(String::new()),
        Ok(Some(frame)) => Some(
            frame
                .unwrap()
                .into_data()
                .map(|d| String::from_utf8_lossy(&d).into_owned())
                .unwrap_or_default(),
        ),
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

#[tokio::test]
async fn stream_requires_login() {
    let a = app_with(fast()).await;
    let r = send(&a, get("/events/stream", None)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn stream_has_event_stream_headers() {
    let a = app_with(fast()).await;
    let cookie = login(&a).await;
    let res = open_stream(&a, &cookie).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    assert!(res.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-cache"));
}

#[tokio::test]
async fn new_events_are_announced_but_old_ones_are_not() {
    let a = app_with(fast()).await;
    a.store
        .record_event(None, "old", now(), None)
        .await
        .unwrap();
    let cookie = login(&a).await;
    let mut body = open_stream(&a, &cookie).await.into_body();

    assert_eq!(
        next_text(&mut body, Duration::from_millis(300)).await,
        None,
        "events that predate the connection must not be announced"
    );
    a.store
        .record_event(None, "changed", now(), Some("+1 -0"))
        .await
        .unwrap();
    let text = next_text(&mut body, Duration::from_secs(3))
        .await
        .expect("an update");
    assert!(text.contains("event: change"), "{text}");
}

#[tokio::test]
async fn announcements_carry_no_event_details() {
    let a = app_with(fast()).await;
    let cookie = login(&a).await;
    let mut body = open_stream(&a, &cookie).await.into_body();
    a.store
        .record_event(
            None,
            "check_failed",
            now(),
            Some("SECRET detail https://u:p@h/"),
        )
        .await
        .unwrap();
    let text = next_text(&mut body, Duration::from_secs(3))
        .await
        .expect("an update");
    assert!(
        !text.contains("SECRET") && !text.contains("check_failed"),
        "{text}"
    );
}

#[tokio::test]
async fn stream_ends_when_the_session_ends() {
    let a = app_with(fast()).await;
    let cookie = login(&a).await;
    let mut body = open_stream(&a, &cookie).await.into_body();
    let t = enc(&csrf(&a, &cookie).await);
    send(&a, post("/logout", Some(&cookie), &format!("csrf={t}"))).await;
    let end = next_text(&mut body, Duration::from_secs(3)).await;
    assert_eq!(
        end,
        Some(String::new()),
        "the stream should close after logout"
    );
}

#[tokio::test]
async fn concurrent_streams_are_capped() {
    let a = app_with(WebConfig {
        max_streams: 2,
        ..fast()
    })
    .await;
    let cookie = login(&a).await;
    let first = open_stream(&a, &cookie).await;
    let second = open_stream(&a, &cookie).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let third = open_stream(&a, &cookie).await;
    assert_eq!(third.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(first); // 閉じれば枠が空く
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(open_stream(&a, &cookie).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn pages_include_the_live_update_script() {
    let a = app_with(fast()).await;
    let cookie = login(&a).await;
    let r = send(&a, get("/", Some(&cookie))).await;
    assert!(r.body.contains("/static/app.js"));
    let js = send(&a, get("/static/app.js", None)).await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js.body.contains("EventSource"));
}
