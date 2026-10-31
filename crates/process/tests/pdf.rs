//! PDF のテキスト化。信頼できない入力なので、壊れたファイルでパニックしたり止まったりしないこと。

use watchtower_core::{ConfigLayer, EffectiveConfig};
use watchtower_process::{process_pdf, ProcessError};

fn cfg(f: impl FnOnce(&mut ConfigLayer)) -> EffectiveConfig {
    let mut l = ConfigLayer::default();
    f(&mut l);
    EffectiveConfig::resolve(&l, &[], &ConfigLayer::default())
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// 1 ページ・標準フォントだけの最小の PDF を組み立てる (xref のオフセットも計算する)。
fn pdf(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 12 Tf 72 720 Td 14 TL\n");
    for l in lines {
        content.push_str(&format!("({}) Tj T*\n", escape(l)));
    }
    content.push_str("ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R \
         /Resources << /Font << /F1 4 0 R >> >> >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref_at = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for o in &offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

#[test]
fn text_is_extracted_line_by_line() {
    let out = process_pdf(
        &pdf(&["Price: 19.99 USD", "Stock: available"]),
        &cfg(|_| {}),
    )
    .unwrap();
    assert!(out.text.contains("Price: 19.99 USD"), "{:?}", out.text);
    assert!(out.text.contains("Stock: available"), "{:?}", out.text);
}

#[test]
fn escaped_characters_survive() {
    let out = process_pdf(&pdf(&["a (nested) b \\ c"]), &cfg(|_| {})).unwrap();
    assert!(out.text.contains("a (nested) b"), "{:?}", out.text);
}

#[test]
fn the_hash_follows_the_text() {
    let a = process_pdf(&pdf(&["one"]), &cfg(|_| {})).unwrap();
    let same = process_pdf(&pdf(&["one"]), &cfg(|_| {})).unwrap();
    let b = process_pdf(&pdf(&["two"]), &cfg(|_| {})).unwrap();
    assert_eq!(a.hash, same.hash);
    assert_ne!(a.hash, b.hash);
}

#[test]
fn ignore_and_trigger_text_apply() {
    let bytes = pdf(&["Updated: 12:00:01", "Status: SOLD OUT"]);
    let out = process_pdf(
        &bytes,
        &cfg(|l| {
            l.ignore_text = vec!["Updated:".into()];
            l.trigger_text = vec!["sold out".into()];
        }),
    )
    .unwrap();
    assert!(!out.text.contains("Updated"), "{:?}", out.text);
    assert!(out.triggered);
}

#[test]
fn broken_input_is_an_error_not_a_panic() {
    for bad in [
        b"".as_slice(),
        b"not a pdf at all",
        b"%PDF-1.4\ngarbage that is not a document",
        b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n",
        &[0u8; 64],
    ] {
        let r = process_pdf(bad, &cfg(|_| {}));
        assert!(
            matches!(r, Err(ProcessError::Pdf(_))),
            "{:?} -> {r:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn oversized_files_are_refused_without_parsing() {
    let mut huge = b"%PDF-1.4\n".to_vec();
    huge.resize(21 * 1024 * 1024, b' ');
    let started = std::time::Instant::now();
    let r = process_pdf(&huge, &cfg(|_| {}));
    assert!(
        matches!(r, Err(ProcessError::Pdf(ref m)) if m.contains("too large")),
        "{r:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn mutated_documents_never_panic_or_hang() {
    let valid = pdf(&["Hello", "World", "Mutation testing"]);
    let started = std::time::Instant::now();
    for i in 0..valid.len().min(300) {
        // 1 バイトを書き換える
        let mut m = valid.clone();
        m[i] = m[i].wrapping_add(97);
        let _ = process_pdf(&m, &cfg(|_| {}));
        // そこで切り詰める
        let _ = process_pdf(&valid[..i], &cfg(|_| {}));
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn deeply_nested_page_trees_do_not_overflow_the_stack() {
    let mut doc = b"%PDF-1.4\n".to_vec();
    // 自分自身を親に持つ Pages (循環) と、深い入れ子の配列
    doc.extend(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    doc.extend(b"2 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Parent 2 0 R >>\nendobj\n");
    doc.extend(format!("3 0 obj\n{}\nendobj\n", "[".repeat(20_000)).bytes());
    doc.extend(b"trailer\n<< /Root 1 0 R /Size 4 >>\nstartxref\n0\n%%EOF\n");
    let r = process_pdf(&doc, &cfg(|_| {}));
    assert!(r.is_err() || r.is_ok(), "reaching this line means no crash");
}
