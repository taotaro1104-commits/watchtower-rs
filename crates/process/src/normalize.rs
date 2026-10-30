/// 空白の圧縮、空行除去、無視テキストを含む行の削除。
pub fn normalize(raw: &str, ignore_text: &[String]) -> String {
    raw.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .filter(|l| !ignore_text.iter().any(|t| l.contains(t.as_str())))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_triggered(text: &str, trigger_text: &[String]) -> bool {
    let lower = text.to_lowercase();
    trigger_text
        .iter()
        .any(|t| lower.contains(&t.to_lowercase()))
}
