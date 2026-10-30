//! 実バイナリで RSS フィードのトークン発行と配信を確かめる。

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

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
        .env("WATCHTOWER_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

async fn http_get(port: u16, path: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
async fn rss_token_prints_a_feed_url_that_the_running_server_accepts() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let printed = cmd(d.path(), port).arg("rss-token").output().await.unwrap();
    assert!(printed.status.success());
    let url = out(&printed)
        .split_whitespace()
        .find(|w| w.contains("/rss?token=rss_"))
        .unwrap_or_else(|| panic!("no feed URL in: {}", out(&printed)))
        .to_string();
    assert!(
        url.starts_with(&format!("http://127.0.0.1:{port}/rss?token=")),
        "{url}"
    );
    let path = url
        .strip_prefix(&format!("http://127.0.0.1:{port}"))
        .unwrap()
        .to_string();

    let mut child = cmd(d.path(), port).arg("run").spawn().unwrap();
    for _ in 0..80 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ok = http_get(port, &path).await;
    let bad = http_get(port, "/rss?token=rss_wrong").await;
    child.kill().await.unwrap();

    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("<rss version=\"2.0\""), "{ok}");
    assert!(bad.starts_with("HTTP/1.1 401"), "{bad}");
}

#[tokio::test]
async fn each_call_issues_a_new_token() {
    let d = tempfile::tempdir().unwrap();
    let a = cmd(d.path(), free_port())
        .arg("rss-token")
        .output()
        .await
        .unwrap();
    let b = cmd(d.path(), free_port())
        .arg("rss-token")
        .output()
        .await
        .unwrap();
    assert_ne!(out(&a), out(&b));
}
