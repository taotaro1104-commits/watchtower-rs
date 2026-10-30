use std::time::Duration;

use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{FetchError, FetchRequest, Fetcher, HttpFetcher, SsrfPolicy};
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(server: &MockServer, p: &str) -> FetchRequest {
    FetchRequest::get(Url::parse(&format!("{}{p}", server.uri())).unwrap())
}

/// wiremock は 127.0.0.1 で待ち受けるため、テストではループバックを許可する。
fn fetcher() -> HttpFetcher {
    HttpFetcher::new(SsrfPolicy::allow_loopback())
}

async fn get(
    f: &HttpFetcher,
    r: &FetchRequest,
) -> Result<watchtower_fetch::FetchResponse, FetchError> {
    f.fetch(r, CancellationToken::new()).await
}

#[tokio::test]
async fn fetches_utf8_body_and_metadata() {
    let s = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("こんにちは", "text/plain; charset=utf-8"),
        )
        .mount(&s)
        .await;
    let r = get(&fetcher(), &req(&s, "/a")).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "こんにちは");
    assert_eq!(r.content_type.as_deref(), Some("text/plain; charset=utf-8"));
    assert!(r.final_url.as_str().ends_with("/a"));
}

#[tokio::test]
async fn sends_method_headers_and_body() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-token", "abc"))
        .and(body_string("payload"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&s)
        .await;
    let mut r = req(&s, "/p");
    r.method = http::Method::POST;
    r.headers.push(("x-token".into(), "abc".into()));
    r.body = Some(b"payload".to_vec());
    assert_eq!(get(&fetcher(), &r).await.unwrap().body, "ok");
}

#[tokio::test]
async fn non_success_status_is_an_error() {
    let s = MockServer::start().await;
    Mock::given(path("/x"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s)
        .await;
    assert!(matches!(
        get(&fetcher(), &req(&s, "/x")).await,
        Err(FetchError::Status(503))
    ));
}

#[tokio::test]
async fn accepted_status_list_is_honoured() {
    let s = MockServer::start().await;
    Mock::given(path("/x"))
        .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
        .mount(&s)
        .await;
    let mut r = req(&s, "/x");
    r.accepted_statuses = vec![404];
    assert_eq!(get(&fetcher(), &r).await.unwrap().status, 404);
}

#[tokio::test]
async fn body_over_limit_is_rejected() {
    let s = MockServer::start().await;
    Mock::given(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(2048)))
        .mount(&s)
        .await;
    let mut r = req(&s, "/big");
    r.max_body_bytes = 1024;
    assert!(matches!(
        get(&fetcher(), &r).await,
        Err(FetchError::TooLarge { .. })
    ));
}

#[tokio::test]
async fn decodes_charset_from_header() {
    let s = MockServer::start().await;
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("日本語のページ");
    Mock::given(path("/sj"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sjis.into_owned(), "text/html; charset=Shift_JIS"),
        )
        .mount(&s)
        .await;
    assert_eq!(
        get(&fetcher(), &req(&s, "/sj")).await.unwrap().body,
        "日本語のページ"
    );
}

#[tokio::test]
async fn decodes_charset_from_meta_tag() {
    let s = MockServer::start().await;
    let html = "<html><head><meta charset=\"euc-jp\"></head><body>日本語のページ</body></html>";
    let (euc, _, _) = encoding_rs::EUC_JP.encode(html);
    Mock::given(path("/euc"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(euc.into_owned(), "text/html"))
        .mount(&s)
        .await;
    assert!(get(&fetcher(), &req(&s, "/euc"))
        .await
        .unwrap()
        .body
        .contains("日本語のページ"));
}

#[tokio::test]
async fn timeout_is_reported() {
    let s = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&s)
        .await;
    let mut r = req(&s, "/slow");
    r.timeout = Duration::from_millis(200);
    assert!(matches!(
        get(&fetcher(), &r).await,
        Err(FetchError::Timeout)
    ));
}

#[tokio::test]
async fn cancellation_aborts_the_fetch() {
    let s = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&s)
        .await;
    let token = CancellationToken::new();
    token.cancel();
    let res = fetcher().fetch(&req(&s, "/slow"), token).await;
    assert!(matches!(res, Err(FetchError::Cancelled)));
}

#[tokio::test]
async fn default_policy_blocks_loopback_literal() {
    let s = MockServer::start().await;
    let f = HttpFetcher::new(SsrfPolicy::default());
    let res = get(&f, &req(&s, "/a")).await;
    assert!(matches!(res, Err(FetchError::Blocked(_))), "{res:?}");
}

#[tokio::test]
async fn redirect_to_blocked_address_is_refused() {
    let s = MockServer::start().await;
    // ループバックのみ許可したポリシーでも、メタデータ用 IP へのリダイレクトは拒否される。
    Mock::given(path("/r"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://169.254.169.254/latest"),
        )
        .mount(&s)
        .await;
    assert!(get(&fetcher(), &req(&s, "/r")).await.is_err());
}
