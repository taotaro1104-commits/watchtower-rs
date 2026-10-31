//! 実際の Chrome を相手にした確認。通常のテストでは走らせない (`#[ignore]`)。
//!
//! 使い方 (Chrome を `--remote-debugging-port` つきで起動してから):
//!   WATCHTOWER_TEST_CHROME_WS=ws://127.0.0.1:9333/devtools/browser/<id> \
//!     cargo test -p watchtower-fetch --test cdp_real -- --ignored --test-threads=1
//! `<id>` は `http://127.0.0.1:9333/json/version` の `webSocketDebuggerUrl` に書いてある。

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_fetch::{CdpFetcher, FetchError, FetchRequest, Fetcher, SsrfPolicy};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ws() -> String {
    std::env::var("WATCHTOWER_TEST_CHROME_WS")
        .expect("set WATCHTOWER_TEST_CHROME_WS to the browser's webSocketDebuggerUrl")
}

fn fetcher() -> CdpFetcher {
    // wiremock は 127.0.0.1 で待ち受けるので、ループバックだけ許可する
    CdpFetcher::new(ws(), SsrfPolicy::allow_loopback()).with_settle(Duration::from_millis(200))
}

fn req(server: &MockServer, p: &str) -> FetchRequest {
    let mut r = FetchRequest::get(Url::parse(&format!("{}{p}", server.uri())).unwrap());
    r.timeout = Duration::from_secs(20);
    r
}

fn html(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/html; charset=utf-8")
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn javascript_rendered_content_is_returned() {
    let server = MockServer::start().await;
    let page = r#"<html><body><div id="x">loading</div>
        <script>document.getElementById('x').textContent = 'rendered by javascript';</script>
        </body></html>"#;
    Mock::given(path("/p"))
        .respond_with(html(page))
        .mount(&server)
        .await;
    let r = fetcher()
        .fetch(&req(&server, "/p"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert!(r.body.contains("rendered by javascript"), "{}", r.body);
    assert!(
        r.content_type
            .as_deref()
            .is_some_and(|c| c.starts_with("text/html")),
        "{:?}",
        r.content_type
    );
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn non_ascii_text_survives() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(html("<html><body>価格: ¥1,980 🎉</body></html>"))
        .mount(&server)
        .await;
    let r = fetcher()
        .fetch(&req(&server, "/p"), CancellationToken::new())
        .await
        .unwrap();
    assert!(r.body.contains("価格: ¥1,980 🎉"), "{}", r.body);
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn a_screenshot_is_a_real_png() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(html("<html><body><h1>shot</h1></body></html>"))
        .mount(&server)
        .await;
    let r = fetcher()
        .with_screenshots(true)
        .fetch(&req(&server, "/p"), CancellationToken::new())
        .await
        .unwrap();
    let png = r.screenshot.expect("screenshot");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(png.len() > 100);
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn error_statuses_are_reported() {
    let server = MockServer::start().await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_raw("<html>no</html>", "text/html"))
        .mount(&server)
        .await;
    let err = fetcher()
        .fetch(&req(&server, "/gone"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Status(404)), "{err:?}");
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn a_redirect_to_an_internal_address_is_blocked_by_the_browser_interception() {
    let server = MockServer::start().await;
    Mock::given(path("/r"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://10.255.255.1/secret"),
        )
        .mount(&server)
        .await;
    let err = fetcher()
        .fetch(&req(&server, "/r"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(err, FetchError::Http(_) | FetchError::Blocked(_)),
        "{err:?}"
    );
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn a_redirect_to_an_allowed_address_is_followed() {
    let server = MockServer::start().await;
    Mock::given(path("/r"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/final"))
        .mount(&server)
        .await;
    Mock::given(path("/final"))
        .respond_with(html("<html><body>arrived</body></html>"))
        .mount(&server)
        .await;
    let r = fetcher()
        .fetch(&req(&server, "/r"), CancellationToken::new())
        .await
        .unwrap();
    assert!(r.body.contains("arrived"), "{}", r.body);
    assert!(r.final_url.path().ends_with("/final"), "{}", r.final_url);
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn subresources_on_internal_addresses_are_never_requested() {
    let server = MockServer::start().await;
    // ページは 127.0.0.1 (許可) の別ポートの画像と、内部アドレスの画像を読もうとする
    let internal = MockServer::start().await;
    Mock::given(path("/img.png"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0..)
        .mount(&internal)
        .await;
    let page = format!(
        r#"<html><body><img src="http://10.255.255.1/a.png"><img src="http://169.254.169.254/b.png">
           <img src="{}/img.png"></body></html>"#,
        internal.uri()
    );
    Mock::given(path("/p"))
        .respond_with(html(&page))
        .mount(&server)
        .await;
    let r = fetcher()
        .fetch(&req(&server, "/p"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn many_fetches_in_a_row_do_not_leak_tabs() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(html("<html><body>ok</body></html>"))
        .mount(&server)
        .await;
    let f = fetcher();
    for _ in 0..8 {
        f.fetch(&req(&server, "/p"), CancellationToken::new())
            .await
            .unwrap();
    }
}

const STEPS_PAGE: &str = r#"<html><body>
  <button id="go" onclick="
      setTimeout(function () {
        var d = document.createElement('div');
        d.id = 'late';
        d.textContent = 'hello ' + document.getElementById('q').value;
        document.body.appendChild(d);
      }, 300);">go</button>
  <input id="q" name="q" oninput="document.title = 'typed:' + this.value">
</body></html>"#;

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn click_type_and_wait_for_work_on_a_real_page() {
    use watchtower_core::BrowserStep;
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(html(STEPS_PAGE))
        .mount(&server)
        .await;
    let mut r = req(&server, "/p");
    r.browser_steps = vec![
        BrowserStep::Type {
            selector: "input[name=q]".into(),
            text: "world \"quoted\" 日本語".into(),
        },
        BrowserStep::Click {
            selector: "#go".into(),
        },
        BrowserStep::WaitFor {
            selector: "#late".into(),
            timeout_ms: Some(5000),
        },
    ];
    let resp = fetcher().fetch(&r, CancellationToken::new()).await.unwrap();
    assert!(
        resp.body.contains("hello world \"quoted\" 日本語")
            || resp.body.contains("hello world &quot;quoted&quot; 日本語"),
        "{}",
        resp.body
    );
    assert!(
        resp.body.contains("typed:world"),
        "the input event fired: {}",
        resp.body
    );
}

#[tokio::test]
#[ignore = "needs a real Chrome (see the module docs)"]
async fn a_missing_element_and_a_bad_selector_fail_cleanly() {
    use watchtower_core::BrowserStep;
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(html(STEPS_PAGE))
        .mount(&server)
        .await;
    for (step, expect) in [
        (
            BrowserStep::Click {
                selector: "#nothing-here".into(),
            },
            "element not found",
        ),
        (
            BrowserStep::Click {
                selector: "###".into(),
            },
            "invalid selector",
        ),
        (
            BrowserStep::WaitFor {
                selector: "#never".into(),
                timeout_ms: Some(400),
            },
            "did not appear",
        ),
    ] {
        let mut r = req(&server, "/p");
        r.browser_steps = vec![step];
        let err = fetcher()
            .fetch(&r, CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains(expect), "{err}");
    }
}
