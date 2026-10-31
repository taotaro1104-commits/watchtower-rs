//! ウォッチごとの取得オプション (ヘッダ・メソッド・ボディ・プロキシ) の検証・テキスト表現・層のマージ。

use watchtower_core::request::{
    headers_to_text, normalize_method, parse_headers_text, validate_body, validate_headers,
    validate_proxy, MAX_BODY_CHARS, MAX_HEADERS,
};
use watchtower_core::{ConfigLayer, EffectiveConfig};

fn h(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

#[test]
fn header_text_round_trips() {
    let text = "X-Token: abc\nAccept-Language: ja";
    let hs = parse_headers_text(text).unwrap();
    assert_eq!(hs, vec![h("X-Token", "abc"), h("Accept-Language", "ja")]);
    assert_eq!(headers_to_text(&hs), text);
}

#[test]
fn a_value_may_contain_colons() {
    let hs = parse_headers_text("Referer: https://example.com/a").unwrap();
    assert_eq!(hs, vec![h("Referer", "https://example.com/a")]);
}

#[test]
fn a_header_line_without_a_colon_names_the_line() {
    let e = parse_headers_text("ok: 1\nbroken").unwrap_err();
    assert!(e.contains("line 2"), "{e}");
}

#[test]
fn bad_header_names_and_values_are_rejected() {
    assert!(validate_headers(&[h("bad name", "x")]).is_err());
    assert!(validate_headers(&[h("", "x")]).is_err());
    assert!(validate_headers(&[h("X", "a\r\nInjected: 1")]).is_err());
    assert!(validate_headers(&[h("X", "a\0b")]).is_err());
}

#[test]
fn hop_by_hop_and_framing_headers_are_not_settable() {
    for name in ["Host", "content-length", "Transfer-Encoding", "Connection"] {
        assert!(validate_headers(&[h(name, "x")]).is_err(), "{name}");
    }
}

#[test]
fn too_many_headers_are_rejected() {
    let many: Vec<_> = (0..=MAX_HEADERS)
        .map(|i| h(&format!("X-{i}"), "v"))
        .collect();
    assert!(validate_headers(&many).is_err());
}

#[test]
fn the_error_never_echoes_the_header_value() {
    let e = validate_headers(&[h("Authorization", "Bearer s3cret\r\nx")]).unwrap_err();
    assert!(!e.contains("s3cret"), "{e}");
}

#[test]
fn methods_are_normalized_and_restricted() {
    assert_eq!(normalize_method("post").unwrap(), "POST");
    assert_eq!(normalize_method(" Get ").unwrap(), "GET");
    assert!(normalize_method("CONNECT").is_err());
    assert!(normalize_method("TRACE").is_err());
    assert!(normalize_method("not a method").is_err());
}

#[test]
fn the_body_is_size_limited() {
    assert!(validate_body("{\"a\":1}").is_ok());
    assert!(validate_body(&"x".repeat(MAX_BODY_CHARS + 1)).is_err());
}

#[test]
fn proxies_must_be_http_like_urls() {
    assert!(validate_proxy("http://proxy.example:8080").is_ok());
    assert!(validate_proxy("https://proxy.example").is_ok());
    assert!(validate_proxy("ftp://proxy.example").is_err());
    assert!(validate_proxy("not a url").is_err());
    assert!(validate_proxy("").is_err());
}

#[test]
fn the_proxy_error_never_echoes_credentials() {
    let e = validate_proxy("ftp://user:hunter2@proxy.example").unwrap_err();
    assert!(!e.contains("hunter2"), "{e}");
}

#[test]
fn headers_merge_by_name_with_the_closest_layer_winning() {
    let w = ConfigLayer {
        headers: vec![h("X-A", "watch")],
        ..Default::default()
    };
    let t = ConfigLayer {
        headers: vec![h("x-a", "tag"), h("X-B", "tag")],
        ..Default::default()
    };
    let g = ConfigLayer {
        headers: vec![h("X-B", "global"), h("X-C", "global")],
        ..Default::default()
    };
    let e = EffectiveConfig::resolve(&w, &[t], &g);
    assert_eq!(
        e.headers,
        vec![h("X-A", "watch"), h("X-B", "tag"), h("X-C", "global")]
    );
}

#[test]
fn method_body_and_proxy_take_the_first_layer_that_sets_them() {
    let w = ConfigLayer {
        method: Some("POST".into()),
        ..Default::default()
    };
    let g = ConfigLayer {
        method: Some("PUT".into()),
        body: Some("b".into()),
        proxy: Some("http://p:1".into()),
        ..Default::default()
    };
    let e = EffectiveConfig::resolve(&w, &[], &g);
    assert_eq!(e.method, "POST");
    assert_eq!(e.body.as_deref(), Some("b"));
    assert_eq!(e.proxy.as_deref(), Some("http://p:1"));
    let d = EffectiveConfig::resolve(&ConfigLayer::default(), &[], &ConfigLayer::default());
    assert_eq!(d.method, "GET");
    assert!(d.body.is_none() && d.proxy.is_none() && d.headers.is_empty());
}

#[test]
fn debug_output_hides_secrets() {
    let l = ConfigLayer {
        headers: vec![h("Authorization", "Bearer s3cret")],
        body: Some("password=hunter2".into()),
        proxy: Some("http://u:pw9@proxy:1".into()),
        ..Default::default()
    };
    let dbg = format!("{l:?} {:?}", EffectiveConfig::resolve(&l, &[], &l));
    for secret in ["s3cret", "hunter2", "pw9"] {
        assert!(!dbg.contains(secret), "{secret} leaked: {dbg}");
    }
}
