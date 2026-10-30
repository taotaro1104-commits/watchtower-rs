#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Html,
    Json,
    Text,
}

impl ContentKind {
    /// Content-Type を優先し、無ければ先頭バイトで判定する。
    pub fn detect(content_type: Option<&str>, body: &str) -> Self {
        let ct = content_type.unwrap_or("").to_ascii_lowercase();
        if ct.contains("json") {
            return Self::Json;
        }
        if ct.contains("html") {
            return Self::Html;
        }
        if ct.starts_with("text/") {
            return Self::Text;
        }
        let head = body.trim_start();
        if (head.starts_with('{') || head.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(body).is_ok()
        {
            Self::Json
        } else if head.starts_with('<') {
            Self::Html
        } else {
            Self::Text
        }
    }
}
