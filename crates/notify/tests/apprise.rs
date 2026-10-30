//! Apprise API への転送 (未実装のサービスを外部サーバで吸収する)。
//!
//! 外部転送を有効にするのはプロセス全体の設定なので、このファイルのテストはすべて有効にして走らせる。
//! 無効のときの挙動は `apprise_off.rs` で確かめる。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{HttpFetcher, SsrfPolicy};
use watchtower_notify::{
    allow_foreign_targets, AppriseApiNotifier, CompositeNotifier, HttpNotifier, Message, Notifier,
    NotifyError, NotifyTarget, SmtpNotifier,
};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FOREIGN: &str = "pbul://o.SECRETtoken123";

fn on() {
    allow_foreign_targets(true);
}

fn msg() -> Message {
    Message {
        title: "価格が変わりました".into(),
        body: "- 10\n+ 20".into(),
    }
}

fn apprise(base: &str, policy: SsrfPolicy) -> AppriseApiNotifier {
    AppriseApiNotifier::new(
        Url::parse(base).unwrap(),
        Arc::new(HttpFetcher::new(policy)),
    )
}

// ---- 宛先の解釈 -------------------------------------------------------------

#[test]
fn unknown_schemes_are_accepted_as_foreign_targets_when_enabled() {
    on();
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    assert!(t.is_foreign() && !t.is_mail());
    assert_eq!(t.scheme(), "foreign");
}

#[test]
fn foreign_targets_never_reveal_their_credentials() {
    on();
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    for s in [t.to_string(), format!("{t:?}")] {
        assert!(!s.contains("SECRET") && !s.contains("o."), "{s}");
        assert!(s.contains("pbul"), "{s}");
    }
}

#[test]
fn native_schemes_stay_native() {
    on();
    for url in ["discord://1/tok", "json://h.example.com/x", "ntfy://topic"] {
        assert!(!NotifyTarget::parse(url).unwrap().is_foreign(), "{url}");
    }
}

#[test]
fn malformed_foreign_targets_are_still_rejected() {
    on();
    let long = format!("pbul://{}", "a".repeat(3000));
    for bad in [
        "pbul://has space",
        "pbul://tab\there",
        "pbul://new\nline",
        "pbul://nul\0byte",
        "pbul://",
        "://nothing",
        "not a url",
        long.as_str(),
    ] {
        assert!(NotifyTarget::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn foreign_targets_have_no_direct_http_request() {
    on();
    assert!(NotifyTarget::parse(FOREIGN)
        .unwrap()
        .build_request(&msg())
        .is_err());
}

// ---- 送信 -------------------------------------------------------------------

#[tokio::test]
async fn the_message_and_the_original_url_are_posted_to_the_apprise_api() {
    on();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/notify"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(serde_json::json!({
            "urls": FOREIGN,
            "title": "価格が変わりました",
            "body": "- 10\n+ 20",
            "type": "info",
        })))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let n = apprise(&server.uri(), SsrfPolicy::allow_loopback());
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    n.send(&t, &msg(), CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn a_path_prefix_in_the_base_url_is_kept() {
    on();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/apprise/notify"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let n = apprise(
        &format!("{}/apprise/", server.uri()),
        SsrfPolicy::allow_loopback(),
    );
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    n.send(&t, &msg(), CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn failures_are_delivery_errors_without_secrets() {
    on();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(424))
        .mount(&server)
        .await;
    let n = apprise(&server.uri(), SsrfPolicy::allow_loopback());
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    let err = n
        .send(&t, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
    assert!(err.to_string().contains("424"), "{err}");
    assert!(!err.to_string().contains("SECRET"), "{err}");
}

#[tokio::test]
async fn the_apprise_server_is_subject_to_the_ssrf_policy() {
    on();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let n = apprise(&server.uri(), SsrfPolicy::default());
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    let err = n
        .send(&t, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
}

#[tokio::test]
async fn native_targets_are_refused_by_the_apprise_notifier() {
    on();
    let n = apprise("http://127.0.0.1:1", SsrfPolicy::allow_loopback());
    let t = NotifyTarget::parse("json://h.example.com/x").unwrap();
    assert!(n.send(&t, &msg(), CancellationToken::new()).await.is_err());
}

// ---- 振り分け ---------------------------------------------------------------

fn composite(apprise_server: Option<&MockServer>) -> CompositeNotifier {
    let policy = SsrfPolicy::allow_loopback();
    let http = Arc::new(HttpNotifier::new(Arc::new(HttpFetcher::new(
        policy.clone(),
    ))));
    let smtp = Arc::new(SmtpNotifier::new(Arc::new(policy.clone())));
    let c = CompositeNotifier::new(http, smtp);
    match apprise_server {
        Some(s) => c.with_apprise(Arc::new(apprise(&s.uri(), policy))),
        None => c,
    }
}

#[tokio::test]
async fn foreign_targets_are_routed_to_apprise_and_native_ones_are_not() {
    on();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/notify"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let hook = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&hook)
        .await;

    let c = composite(Some(&server));
    let foreign = NotifyTarget::parse(FOREIGN).unwrap();
    let native = NotifyTarget::parse(&format!("json://{}/hook", hook.address())).unwrap();
    c.send(&foreign, &msg(), CancellationToken::new())
        .await
        .unwrap();
    c.send(&native, &msg(), CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn foreign_targets_without_an_apprise_server_fail_with_a_clear_reason() {
    on();
    let c = composite(None);
    let t = NotifyTarget::parse(FOREIGN).unwrap();
    let err = c
        .send(&t, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)));
    assert!(err.to_string().contains("Apprise"), "{err}");
    assert!(!err.to_string().contains("SECRET"));
}
