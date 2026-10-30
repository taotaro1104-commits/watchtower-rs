//! 実バイナリで API キーの発行と `/api/v1` の提供を確かめる。

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
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// `api-key` が表示したキー (`wt_...`) を取り出す。
fn key_from(o: &Output) -> String {
    out(o)
        .split_whitespace()
        .find(|w| w.starts_with("wt_"))
        .unwrap_or_else(|| panic!("no key in output: {}", out(o)))
        .to_string()
}

async fn http(port: u16, request: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

fn get(path: &str, key: Option<&str>) -> String {
    let auth = key
        .map(|k| format!("x-api-key: {k}\r\n"))
        .unwrap_or_default();
    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Connection: close\r\n\r\n")
}

async fn wait_until_listening(port: u16) {
    for _ in 0..80 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not start listening on {port}");
}

#[tokio::test]
async fn api_key_command_issues_a_new_key_each_time() {
    let d = tempfile::tempdir().unwrap();
    let first = cmd(d.path(), free_port())
        .arg("api-key")
        .output()
        .await
        .unwrap();
    assert!(first.status.success());
    let second = cmd(d.path(), free_port())
        .arg("api-key")
        .output()
        .await
        .unwrap();
    assert_ne!(key_from(&first), key_from(&second));
    assert!(key_from(&first).len() >= 35);
}

#[tokio::test]
async fn run_serves_the_api_and_enforces_the_key() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let key = key_from(&cmd(d.path(), port).arg("api-key").output().await.unwrap());

    let mut child = cmd(d.path(), port).arg("run").spawn().unwrap();
    wait_until_listening(port).await;

    let no_key = http(port, &get("/api/v1/systeminfo", None)).await;
    let wrong = http(port, &get("/api/v1/systeminfo", Some("wt_wrong"))).await;
    let ok = http(port, &get("/api/v1/systeminfo", Some(&key))).await;
    child.kill().await.unwrap();

    assert!(no_key.starts_with("HTTP/1.1 401"), "{no_key}");
    assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("\"watch_count\":0"), "{ok}");
}

#[tokio::test]
async fn a_watch_created_through_the_api_is_visible_in_the_cli() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let key = key_from(&cmd(d.path(), port).arg("api-key").output().await.unwrap());
    let mut child = cmd(d.path(), port).arg("run").spawn().unwrap();
    wait_until_listening(port).await;

    let body = r#"{"url":"https://example.com/api-made","title":"From API"}"#;
    let req = format!(
        "POST /api/v1/watch HTTP/1.1\r\nHost: localhost\r\nx-api-key: {key}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let created = http(port, &req).await;
    child.kill().await.unwrap();
    assert!(created.starts_with("HTTP/1.1 201"), "{created}");

    let list = cmd(d.path(), port)
        .args(["watch", "list"])
        .output()
        .await
        .unwrap();
    assert!(out(&list).contains("From API"), "{}", out(&list));
}
