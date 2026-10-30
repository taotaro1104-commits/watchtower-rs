//! 宛先 URL の解釈と、スキームごとのリクエスト生成。

use serde_json::Value;
use watchtower_notify::{Message, NotifyError, NotifyTarget};

fn msg() -> Message {
    Message {
        title: "価格が変わりました".into(),
        body: "- 10\n+ 20".into(),
    }
}

fn body_json(t: &str) -> (watchtower_fetch::FetchRequest, Value) {
    let req = NotifyTarget::parse(t)
        .unwrap()
        .build_request(&msg())
        .unwrap();
    let v = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    (req, v)
}

fn header<'a>(req: &'a watchtower_fetch::FetchRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[test]
fn json_and_jsons_map_to_http_and_https_post() {
    let (req, v) = body_json("json://example.com/hook");
    assert_eq!(req.url.as_str(), "http://example.com/hook");
    assert_eq!(req.method, http::Method::POST);
    assert_eq!(header(&req, "content-type"), Some("application/json"));
    assert_eq!(v["title"], "価格が変わりました");
    assert_eq!(v["message"], "- 10\n+ 20");

    let (req, _) = body_json("jsons://example.com:8443/a?x=1");
    assert_eq!(req.url.as_str(), "https://example.com:8443/a?x=1");
}

#[test]
fn discord_uses_the_webhook_endpoint_and_truncates() {
    let (req, v) = body_json("discord://123456/abcDEF-_");
    assert_eq!(
        req.url.as_str(),
        "https://discord.com/api/webhooks/123456/abcDEF-_"
    );
    assert!(v["content"]
        .as_str()
        .unwrap()
        .contains("価格が変わりました"));

    let long = Message {
        title: "t".into(),
        body: "あ".repeat(5000),
    };
    let req = NotifyTarget::parse("discord://1/x")
        .unwrap()
        .build_request(&long)
        .unwrap();
    let v: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    assert!(v["content"].as_str().unwrap().chars().count() <= 2000);
}

#[test]
fn slack_joins_three_tokens() {
    let (req, v) = body_json("slack://T000/B000/XXXX");
    assert_eq!(
        req.url.as_str(),
        "https://hooks.slack.com/services/T000/B000/XXXX"
    );
    assert!(v["text"].as_str().unwrap().contains("- 10"));
}

#[test]
fn telegram_keeps_the_colon_in_the_bot_token() {
    let (req, v) = body_json("tgram://123456:ABC-def/-100123");
    assert_eq!(
        req.url.as_str(),
        "https://api.telegram.org/bot123456:ABC-def/sendMessage"
    );
    assert_eq!(v["chat_id"], "-100123");
    assert!(v["text"].as_str().unwrap().contains("価格が変わりました"));
}

#[test]
fn ntfy_publishes_json_so_non_ascii_titles_survive() {
    let (req, v) = body_json("ntfy://alerts");
    assert_eq!(req.url.as_str(), "https://ntfy.sh/");
    assert_eq!(v["topic"], "alerts");
    assert_eq!(v["title"], "価格が変わりました");

    let (req, _) = body_json("ntfy://localhost:8080/alerts");
    assert_eq!(req.url.as_str(), "http://localhost:8080/");
    let (req, v) = body_json("ntfys://ntfy.example.com/alerts");
    assert_eq!(req.url.as_str(), "https://ntfy.example.com/");
    assert_eq!(v["topic"], "alerts");
}

#[test]
fn gotify_sends_the_token_in_a_header_not_the_url() {
    let (req, v) = body_json("gotify://gotify.example.com/AbC123");
    assert_eq!(req.url.as_str(), "http://gotify.example.com/message");
    assert_eq!(header(&req, "x-gotify-key"), Some("AbC123"));
    assert!(!req.url.as_str().contains("AbC123"));
    assert_eq!(v["title"], "価格が変わりました");

    let (req, _) = body_json("gotifys://gotify.example.com/AbC123");
    assert_eq!(req.url.scheme(), "https");
}

#[test]
fn malformed_targets_are_rejected_at_parse_time() {
    for bad in [
        "not a url",
        "json://",
        "discord://123456",
        "discord://1/../x",
        "discord://1/a b",
        "slack://T/B",
        "tgram://onlytoken",
        "ntfy://",
        "gotify://host",
    ] {
        assert!(
            matches!(NotifyTarget::parse(bad), Err(NotifyError::InvalidUrl(_))),
            "{bad} must be InvalidUrl"
        );
    }
}

#[test]
fn unknown_schemes_are_reported_as_unsupported() {
    assert!(matches!(
        NotifyTarget::parse("foo://x/y"),
        Err(NotifyError::UnsupportedScheme(s)) if s == "foo"
    ));
}

#[test]
fn scheme_matching_is_case_insensitive() {
    assert!(NotifyTarget::parse("DISCORD://1/x").is_ok());
}

#[test]
fn display_and_debug_never_reveal_credentials() {
    for t in [
        "discord://123456/SECRETtoken",
        "slack://T000/B000/SECRETtoken",
        "tgram://123456:SECRETtoken/42",
        "gotify://h.example.com/SECRETtoken",
        "json://user:SECRETtoken@example.com/hook?key=SECRETtoken",
    ] {
        let t = NotifyTarget::parse(t).unwrap();
        assert!(!t.to_string().contains("SECRET"), "{t}");
        assert!(!format!("{t:?}").contains("SECRET"), "{t:?}");
    }
}

#[test]
fn slack_text_cannot_trigger_mentions_or_links_from_page_content() {
    let msg = Message {
        title: "t".into(),
        body: "<!channel> <!here> & <@U123>".into(),
    };
    let req = NotifyTarget::parse("slack://T0/B0/X")
        .unwrap()
        .build_request(&msg)
        .unwrap();
    let v: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    let text = v["text"].as_str().unwrap();
    assert!(!text.contains('<') && !text.contains('>'), "{text}");
    assert!(
        text.contains("&lt;!channel&gt;") && text.contains("&amp;"),
        "{text}"
    );
}
