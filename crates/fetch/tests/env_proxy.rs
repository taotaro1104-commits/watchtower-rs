//! 環境変数のプロキシ (HTTP_PROXY 等) が SSRF 対策を迂回しないこと。
//! 環境変数を書き換えるので、このファイルにはテストを 1 つだけ置く。

use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{FetchRequest, Fetcher, HttpFetcher, SsrfPolicy};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn environment_proxy_is_ignored() {
    // 到達不能なプロキシ。環境のプロキシを使ってしまうと取得に失敗する。
    std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    std::env::set_var("http_proxy", "http://127.0.0.1:9");
    std::env::set_var("ALL_PROXY", "http://127.0.0.1:9");

    let s = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
        .mount(&s)
        .await;
    let f = HttpFetcher::new(SsrfPolicy::allow_loopback());
    let req = FetchRequest::get(Url::parse(&format!("{}/a", s.uri())).unwrap());
    let r = f
        .fetch(&req, CancellationToken::new())
        .await
        .expect("must connect directly");
    assert_eq!(r.body, "direct");
}
