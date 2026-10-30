use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;

const META_SCAN_BYTES: usize = 2048;

/// BOM → HTTP ヘッダの charset → `<meta charset>` → chardetng 推定の順で UTF-8 化する。
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        return enc
            .decode_without_bom_handling(&bytes[bom_len..])
            .0
            .into_owned();
    }
    let enc = header_charset(content_type)
        .or_else(|| meta_charset(bytes))
        .unwrap_or_else(|| guess(bytes, content_type));
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

fn label_to_encoding(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.trim().as_bytes())
}

fn header_charset(content_type: Option<&str>) -> Option<&'static Encoding> {
    let ct = content_type?.to_ascii_lowercase();
    let rest = ct.split("charset=").nth(1)?;
    let label = rest
        .split(';')
        .next()?
        .trim_matches(|c| c == '"' || c == '\'');
    label_to_encoding(label)
}

fn meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let head: String = bytes
        .iter()
        .take(META_SCAN_BYTES)
        .map(|&b| {
            if b.is_ascii() {
                b.to_ascii_lowercase() as char
            } else {
                ' '
            }
        })
        .collect();
    let rest = head.split("charset").nth(1)?;
    let rest =
        rest.trim_start_matches(|c: char| c == '=' || c == '"' || c == '\'' || c.is_whitespace());
    let label: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    label_to_encoding(&label)
}

fn guess(bytes: &[u8], content_type: Option<&str>) -> &'static Encoding {
    if content_type.is_some_and(|c| c.to_ascii_lowercase().contains("json")) {
        return encoding_rs::UTF_8;
    }
    let mut det = EncodingDetector::new(Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    det.guess(None, Utf8Detection::Allow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_is_stripped() {
        assert_eq!(decode(b"\xEF\xBB\xBFhi", None), "hi");
    }

    #[test]
    fn header_wins_over_meta() {
        let (b, _, _) = encoding_rs::SHIFT_JIS.encode("<meta charset=utf-8>日本語");
        assert!(decode(&b, Some("text/html; charset=shift_jis")).contains("日本語"));
    }

    #[test]
    fn unknown_label_falls_back_to_detection() {
        assert_eq!(
            decode("plain".as_bytes(), Some("text/plain; charset=bogus")),
            "plain"
        );
    }
}
