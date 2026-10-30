//! 実バイナリで、未実装のサービスが Apprise API へ転送されること。

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use tokio::process::Command;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cmd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{}", free_port()))
        .env("WATCHTOWER_ALLOWED_IPS", r#"["127.0.0.1"]"#)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[tokio::test]
async fn foreign_targets_are_refused_unless_an_apprise_server_is_configured() {
    let d = tempfile::tempdir().unwrap();
    let add = cmd(d.path())
        .args([
            "watch",
            "add",
            "https://example.com/",
            "--notify",
            "pbul://o.SECRETtoken",
        ])
        .output()
        .await
        .unwrap();
    assert!(!add.status.success());
    assert!(!err(&add).contains("SECRET"), "{}", err(&add));
}

#[tokio::test]
async fn a_malformed_apprise_url_stops_startup() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        "not a url",
        "ftp://apprise.example",
        "http://user:pass@apprise.example",
    ] {
        let out = cmd(d.path())
            .env("WATCHTOWER_APPRISE_API_URL", bad)
            .args(["watch", "list"])
            .output()
            .await
            .unwrap();
        assert!(!out.status.success(), "{bad}");
        assert!(
            !err(&out).contains("pass"),
            "credentials must not be echoed: {}",
            err(&out)
        );
    }
}

#[tokio::test]
async fn a_change_on_an_unsupported_service_is_forwarded_to_the_apprise_api() {
    let site = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("one", "text/plain"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&site)
        .await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("two", "text/plain"))
        .with_priority(2)
        .mount(&site)
        .await;
    let apprise = MockServer::start().await;
    Mock::given(path("/notify"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&apprise)
        .await;

    let d = tempfile::tempdir().unwrap();
    let apprise_url = apprise.uri();
    let url = format!("{}/p", site.uri());
    let add = cmd(d.path())
        .env("WATCHTOWER_APPRISE_API_URL", &apprise_url)
        .args([
            "watch",
            "add",
            &url,
            "--interval",
            "1",
            "--notify",
            "pbul://o.tok123",
        ])
        .output()
        .await
        .unwrap();
    assert!(add.status.success(), "{}", err(&add));

    let mut child = cmd(d.path())
        .env("WATCHTOWER_APPRISE_API_URL", &apprise_url)
        .arg("run")
        .spawn()
        .unwrap();
    let mut got = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        got = apprise
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.url.path() == "/notify");
        if got.is_some() {
            break;
        }
    }
    child.kill().await.unwrap();

    let req = got.expect("the Apprise API should have been called");
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["urls"], "pbul://o.tok123");
    assert!(body["body"].as_str().unwrap().contains("+ two"), "{body}");
}
