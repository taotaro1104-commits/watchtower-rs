//! 実バイナリで `/metrics` が実際のチェックを数えること。

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

fn cmd(data: &Path, port: u16) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{port}"))
        // wiremock は 127.0.0.1 で待ち受けるため許可する
        .env("WATCHTOWER_ALLOWED_IPS", r#"["127.0.0.1"]"#)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

async fn scrape(port: u16, key: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "GET /metrics HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

fn value(body: &str, name: &str) -> f64 {
    body.lines()
        .find(|l| !l.starts_with('#') && l.split([' ', '{']).next() == Some(name))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap_or_else(|| panic!("{name} missing in:\n{body}"))
        .trim()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn metrics_count_real_checks_and_fetches() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("hello", "text/plain"))
        .mount(&server)
        .await;

    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let key = out(&cmd(d.path(), port).arg("api-key").output().await.unwrap())
        .split_whitespace()
        .find(|w| w.starts_with("wt_"))
        .unwrap()
        .to_string();
    let url = format!("{}/p", server.uri());
    let add = cmd(d.path(), port)
        .args(["watch", "add", &url, "--interval", "1"])
        .output()
        .await
        .unwrap();
    assert!(add.status.success());

    let mut child = cmd(d.path(), port).arg("run").spawn().unwrap();
    let mut body = String::new();
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            body = scrape(port, &key).await;
            if body.contains("watchtower_checks_total")
                && value(&body, "watchtower_checks_total") >= 2.0
            {
                break;
            }
        }
    }
    child.kill().await.unwrap();

    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(value(&body, "watchtower_checks_total") >= 2.0, "{body}");
    assert_eq!(value(&body, "watchtower_check_failures_total"), 0.0);
    assert!(value(&body, "watchtower_fetch_seconds_count") >= 2.0);
    assert!(value(&body, "watchtower_queue_wait_seconds_count") >= 2.0);
    assert_eq!(value(&body, "watchtower_watches"), 1.0);
}
