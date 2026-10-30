//! 悪意ある HTML による CPU 枯渇を避けるための入れ子深さチェック。
//!
//! html5ever / html2text は入れ子の深さに対して二次以上で遅くなる (深さ 5000 で数秒)。
//! ブラウザ (Chrome は 512) と同様に上限を設け、パース前に安価な走査で弾く。

pub const MAX_HTML_DEPTH: usize = 512;

/// 閉じタグが省略できる / 子を持たない要素。深さとして数えない。
const NO_DEPTH: &[&str] = &[
    // void
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", // 終了タグ省略可 (暗黙に閉じられる)
    "p", "li", "dt", "dd", "tr", "td", "th", "tbody", "thead", "tfoot", "option", "optgroup",
    "colgroup", "caption", "html", "head", "body",
];

fn counts_for_depth(name: &[u8]) -> bool {
    !NO_DEPTH
        .iter()
        .any(|t| t.as_bytes().eq_ignore_ascii_case(name))
}

/// タグの入れ子が `limit` を超えそうなら true。誤検知より見逃しを許す概算。
pub fn exceeds_depth(html: &str, limit: usize) -> bool {
    let b = html.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&c| c == b'<') {
        i += off + 1;
        if b[i..].starts_with(b"!--") {
            i = find(b, i, b"-->").map_or(b.len(), |p| p + 3);
            continue;
        }
        let closing = b.get(i) == Some(&b'/');
        if closing {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let name = &b[start..i];
        if name.is_empty() || !name[0].is_ascii_alphabetic() {
            continue;
        }
        let self_closing = skip_to_tag_end(b, &mut i);
        if !counts_for_depth(name) {
            continue;
        }
        if closing {
            depth = depth.saturating_sub(1);
        } else if !self_closing {
            depth += 1;
            if depth > limit {
                return true;
            }
        }
    }
    false
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// `>` まで進める (属性値の引用符内は無視)。`/>` で終わるなら true。
fn skip_to_tag_end(b: &[u8], i: &mut usize) -> bool {
    let mut quote: Option<u8> = None;
    let mut prev = 0u8;
    while *i < b.len() {
        let c = b[*i];
        *i += 1;
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'>') => return prev == b'/',
            _ => {}
        }
        prev = c;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nesting_not_siblings() {
        assert!(!exceeds_depth("<div><div></div></div><div></div>", 2));
        assert!(exceeds_depth("<div><div><div>", 2));
    }

    #[test]
    fn comments_and_quoted_attributes_are_skipped() {
        assert!(!exceeds_depth(
            "<!-- <div><div><div> --><a title=\"<div><div><div>\">x</a>",
            1
        ));
    }

    #[test]
    fn self_closing_is_not_depth() {
        assert!(!exceeds_depth("<svg><path/><path /><path/></svg>", 1));
    }

    #[test]
    fn garbage_does_not_panic() {
        for s in ["<", "<<<", "</", "<div", "<!--", "<a b=\"", "< div>", "<1>"] {
            let _ = exceeds_depth(s, 1);
        }
    }
}
