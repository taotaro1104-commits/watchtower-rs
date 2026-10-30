//! 実ブラウザで見つかった表示崩れの回帰テスト (マークアップの形を確認する)。

mod common;

use common::*;
use time::OffsetDateTime;
use url::Url;
use watchtower_core::Watch;

#[tokio::test]
async fn diff_lines_have_no_stray_newlines_between_block_spans() {
    let a = app().await;
    let w = Watch::new(Url::parse("https://example.com/").unwrap());
    a.store.add_watch(&w).await.unwrap();
    let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    a.store
        .save_snapshot(w.id, t, "a\nb", [1; 32])
        .await
        .unwrap();
    a.store
        .save_snapshot(w.id, t, "a\nc", [2; 32])
        .await
        .unwrap();
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}", w.id), Some(&cookie))).await;
    // span を display:block にしているので、<pre> の中では間の空白 (改行も空白 1 文字も) が空行になる
    let start = r.body.find("<pre class=\"diff\">").expect("diff block");
    let end = r.body[start..].find("</pre>").expect("diff block end") + start;
    let block = &r.body[start..end];
    for (i, _) in block.match_indices("</span>") {
        let next = block[i + "</span>".len()..].chars().next();
        assert!(
            matches!(next, Some('<') | None),
            "whitespace after a diff line: {block:?}"
        );
    }
    assert!(r.body.contains("class=\"del\"") && r.body.contains("class=\"add\""));
}

#[tokio::test]
async fn navigation_links_are_grouped_so_they_do_not_spread_across_the_header() {
    let a = app().await;
    let cookie = login(&a).await;
    let r = send(&a, get("/", Some(&cookie))).await;
    assert!(r.body.contains("<nav class=\"nav\">"), "{}", r.body);
}

#[tokio::test]
async fn labels_with_inline_code_wrap_their_text_in_a_single_element() {
    let a = app().await;
    let cookie = login(&a).await;
    for path in ["/settings", "/tags"] {
        let r = send(&a, get(path, Some(&cookie))).await;
        // label は grid なので、直下のテキストと <code> が別の行になってしまう
        assert!(
            !r.body.contains("<label>Notification URLs (e.g. <code>"),
            "{path}"
        );
    }
    let r = send(&a, get("/settings", Some(&cookie))).await;
    assert!(
        r.body.contains("<span class=\"lbl\">Notification URLs"),
        "{}",
        r.body
    );
}
