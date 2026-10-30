//! 実際の送信 (wiremock + HttpFetcher)。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_notify::{HttpNotifier, Message, Notifier, NotifyError, NotifyTarget};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn notifier() -> HttpNotifier {
    HttpNotifier::new(Arc::new(HttpFetcher::new(SsrfPolicy::allow_loopback())))
}

fn msg() -> Message {
    Message {
        title: "T".into(),
        body: "B".into(),
    }
}

#[tokio::test]
async fn json_target_posts_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_partial_json(
            serde_json::json!({"title": "T", "message": "B"}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let target = NotifyTarget::parse(&format!("json://{}/hook", server.address())).unwrap();
    notifier()
        .send(&target, &msg(), CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn server_errors_are_reported_as_delivery_failures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let target = NotifyTarget::parse(&format!("json://{}/hook", server.address())).unwrap();
    let err = notifier()
        .send(&target, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
    assert!(err.to_string().contains("500"), "{err}");
}

#[tokio::test]
async fn private_destinations_are_blocked_by_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let strict = HttpNotifier::new(Arc::new(HttpFetcher::new(SsrfPolicy::default())));
    let target = NotifyTarget::parse(&format!("json://{}/hook", server.address())).unwrap();
    let err = strict
        .send(&target, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
}

#[tokio::test]
async fn delivery_errors_do_not_leak_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let target =
        NotifyTarget::parse(&format!("gotify://{}/SECRETtoken", server.address())).unwrap();
    let err = notifier()
        .send(&target, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("SECRET"), "{err}");
}
