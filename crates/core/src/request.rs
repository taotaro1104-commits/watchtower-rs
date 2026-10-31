//! ウォッチごとの取得オプション (ヘッダ・メソッド・ボディ・プロキシ) の検証とテキスト表現。
//! エラー文面には、ヘッダの値・ボディ・プロキシの認証情報を決して含めない。

use url::Url;

pub const MAX_HEADERS: usize = 30;
pub const MAX_HEADER_NAME_CHARS: usize = 100;
pub const MAX_HEADER_VALUE_CHARS: usize = 2000;
pub const MAX_BODY_CHARS: usize = 100_000;
pub const MAX_PROXY_CHARS: usize = 500;

/// 利用者が設定できないヘッダ (接続や本文の長さは HTTP クライアントが決める)。
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
];

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

pub fn validate_headers(headers: &[(String, String)]) -> Result<(), String> {
    if headers.len() > MAX_HEADERS {
        return Err(format!("at most {MAX_HEADERS} headers are allowed"));
    }
    for (i, (name, value)) in headers.iter().enumerate() {
        let n = i + 1;
        if name.is_empty()
            || name.chars().count() > MAX_HEADER_NAME_CHARS
            || !name.chars().all(is_token_char)
        {
            return Err(format!(
                "header {n}: the name is empty, too long or malformed"
            ));
        }
        if FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(format!("header {n} ({name}) cannot be set"));
        }
        if value.chars().count() > MAX_HEADER_VALUE_CHARS
            || value.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(format!(
                "header {n} ({name}): the value is too long or has control characters"
            ));
        }
    }
    Ok(())
}

/// `Name: value` を 1 行ずつ。空行は無視する。
pub fn parse_headers_text(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("line {}: expected \"Name: value\"", i + 1));
        };
        out.push((name.trim().to_string(), value.trim().to_string()));
    }
    validate_headers(&out)?;
    Ok(out)
}

pub fn headers_to_text(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 大文字にそろえ、許す方法だけを通す。
pub fn normalize_method(raw: &str) -> Result<String, String> {
    let m = raw.trim().to_ascii_uppercase();
    if METHODS.contains(&m.as_str()) {
        Ok(m)
    } else {
        Err(format!("the method must be one of {}", METHODS.join(", ")))
    }
}

pub fn validate_body(body: &str) -> Result<(), String> {
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "the body is longer than {MAX_BODY_CHARS} characters"
        ));
    }
    Ok(())
}

/// `http://` か `https://` のプロキシ。解析に失敗した理由には元の文字列を含めない (認証情報があり得る)。
pub fn validate_proxy(raw: &str) -> Result<(), String> {
    let invalid = || "the proxy must be an http:// or https:// URL".to_string();
    if raw.is_empty() || raw.chars().count() > MAX_PROXY_CHARS {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid());
    }
    Ok(())
}

/// 層をまたぐヘッダのマージ。名前は大文字小文字を区別せず、先に来た (近い) 層が勝つ。
pub fn merge_headers<'a>(
    layers: impl IntoIterator<Item = &'a Vec<(String, String)>>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in layers.into_iter().flatten() {
        if !out.iter().any(|(o, _)| o.eq_ignore_ascii_case(k)) {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}
