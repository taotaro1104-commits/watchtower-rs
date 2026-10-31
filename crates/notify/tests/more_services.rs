//! Mattermost と Pushover の宛先 (M4 の通知サービス追加分)。

use serde_json::Value;
use watchtower_notify::{Message, NotifyTarget};

fn msg() -> Message {
    Message {
        title: "価格が変わりました".into(),
        body: "- 10\n+ 20".into(),
    }
}

fn request(target: &str, m: &Message) -> (watchtower_fetch::FetchRequest, Value) {
    let req = NotifyTarget::parse(target)
        .unwrap()
        .build_request(m)
        .unwrap();
    let v = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    (req, v)
}

#[test]
fn mattermost_posts_to_the_incoming_webhook() {
    let (req, v) = request("mmost://chat.example.com/AbC123xyz", &msg());
    assert_eq!(req.url.as_str(), "http://chat.example.com/hooks/AbC123xyz");
    assert_eq!(req.method, http::Method::POST);
    assert!(v["text"].as_str().unwrap().contains("価格が変わりました"));
    assert!(v["text"].as_str().unwrap().contains("+ 20"));

    let (req, _) = request("mmosts://chat.example.com:8065/AbC123xyz", &msg());
    assert_eq!(
        req.url.as_str(),
        "https://chat.example.com:8065/hooks/AbC123xyz"
    );
}

#[test]
fn mattermost_text_cannot_ping_everyone() {
    let m = Message {
        title: "t".into(),
        body: "@channel @here <!channel>".into(),
    };
    let (_, v) = request("mmosts://chat.example.com/tok", &m);
    let text = v["text"].as_str().unwrap();
    // 先頭に ZWSP を挟むなどして、メンションとして解釈されないこと
    assert!(
        !text.contains("@channel") && !text.contains("@here"),
        "{text}"
    );
}

#[test]
fn pushover_sends_user_and_token_in_the_body() {
    let (req, v) = request(
        "pover://uQiRzpo4DXghDmr9QzzfQu27cmVRsG@azGDORePK8gMaC0QOYAMyEEuzJnyUi",
        &msg(),
    );
    assert_eq!(req.url.as_str(), "https://api.pushover.net/1/messages.json");
    assert_eq!(v["user"], "uQiRzpo4DXghDmr9QzzfQu27cmVRsG");
    assert_eq!(v["token"], "azGDORePK8gMaC0QOYAMyEEuzJnyUi");
    assert_eq!(v["title"], "価格が変わりました");
    assert_eq!(v["message"], "- 10\n+ 20");
    assert!(!req.url.as_str().contains("uQiRz"), "no secret in the URL");
}

#[test]
fn pushover_respects_its_length_limits() {
    let long = Message {
        title: "題".repeat(400),
        body: "あ".repeat(3000),
    };
    let (_, v) = request("pover://user1@tok1", &long);
    assert!(v["title"].as_str().unwrap().chars().count() <= 250);
    assert!(v["message"].as_str().unwrap().chars().count() <= 1024);
}

#[test]
fn malformed_targets_are_rejected() {
    for bad in [
        "mmost://",
        "mmost://host",
        "mmost://host/",
        "mmost://host/a/b",
        "mmost://host/to ken",
        "mmost://host/tok?x=1",
        "pover://",
        "pover://onlyuser",
        "pover://@token",
        "pover://user@",
        "pover://user@tok@en",
        "pover://user@tok/extra",
        "pover://us er@tok",
    ] {
        assert!(NotifyTarget::parse(bad).is_err(), "{bad} must be rejected");
    }
}

#[test]
fn display_and_debug_never_show_credentials() {
    for t in [
        "mmost://chat.example.com/SECRETtoken",
        "pover://SECRETuser@SECRETtoken",
    ] {
        let target = NotifyTarget::parse(t).unwrap();
        for s in [target.to_string(), format!("{target:?}")] {
            assert!(!s.contains("SECRET"), "{s}");
        }
    }
}
