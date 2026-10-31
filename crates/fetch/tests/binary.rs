//! PDF などのバイナリは、文字コード変換を通さず生のバイト列のまま受け取れること。

use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{FetchRequest, Fetcher, HttpFetcher, SsrfPolicy};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fetch(server: &MockServer, p: &str) -> watchtower_fetch::FetchResponse {
    let url = Url::parse(&format!("{}{p}", server.uri())).unwrap();
    HttpFetcher::new(SsrfPolicy::allow_loopback())
        .fetch(&FetchRequest::get(url), CancellationToken::new())
        .await
        .unwrap()
}

/// すべてのバイト値を含む (UTF-8 として不正な) PDF 風のデータ。
fn binary_pdf() -> Vec<u8> {
    let mut v = b"%PDF-1.4\n".to_vec();
    v.extend(0..=255u8);
    v.extend(b"\n%%EOF\n");
    v
}

async fn serve(content_type: &str, body: Vec<u8>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path("/f"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_pdf_is_returned_byte_for_byte() {
    let bytes = binary_pdf();
    let server = serve("application/pdf", bytes.clone()).await;
    let r = fetch(&server, "/f").await;
    assert_eq!(r.raw.as_deref(), Some(bytes.as_slice()));
    assert!(r.body.is_empty(), "binary content is not decoded to text");
}

#[tokio::test]
async fn a_pdf_is_recognised_by_its_magic_even_with_a_generic_content_type() {
    let bytes = binary_pdf();
    for ct in [
        "application/octet-stream",
        "text/plain",
        "binary/octet-stream",
    ] {
        let server = serve(ct, bytes.clone()).await;
        let r = fetch(&server, "/f").await;
        assert_eq!(r.raw.as_deref(), Some(bytes.as_slice()), "{ct}");
    }
}

#[tokio::test]
async fn a_pdf_content_type_with_parameters_counts() {
    let server = serve("application/pdf; charset=binary", binary_pdf()).await;
    assert!(fetch(&server, "/f").await.raw.is_some());
}

#[tokio::test]
async fn text_and_html_are_decoded_as_before_and_carry_no_raw_copy() {
    let server = serve(
        "text/html; charset=utf-8",
        "<p>こんにちは</p>".as_bytes().to_vec(),
    )
    .await;
    let r = fetch(&server, "/f").await;
    assert!(r.raw.is_none());
    assert!(r.body.contains("こんにちは"));
}

#[tokio::test]
async fn the_size_limit_applies_to_binary_content_too() {
    let server = serve("application/pdf", vec![b'x'; 5_000]).await;
    let url = Url::parse(&format!("{}/f", server.uri())).unwrap();
    let mut req = FetchRequest::get(url);
    req.max_body_bytes = 1_000;
    let err = HttpFetcher::new(SsrfPolicy::allow_loopback())
        .fetch(&req, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(err, watchtower_fetch::FetchError::TooLarge { .. }),
        "{err:?}"
    );
}
