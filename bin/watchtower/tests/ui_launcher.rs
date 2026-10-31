//! `ui`: サーバを起動し、立ち上がったら管理画面の URL を表示する (`--no-open` ではブラウザを開かない)。

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn ui_prints_a_url_that_serves_the_admin_page() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_watchtower"))
        .env("WATCHTOWER_DATA_DIR", d.path())
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{port}"))
        .env("RUST_LOG", "warn")
        .args(["ui", "--no-open"])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("no URL printed in time")
        .unwrap()
        .expect("stdout closed");
    assert_eq!(line, format!("http://127.0.0.1:{port}/"));

    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).await.unwrap();
    // パスワード未設定なら案内、設定済みならログインへの転送。どちらでも応答は返る
    assert!(raw.starts_with("HTTP/1.1 "), "{raw}");
    child.kill().await.unwrap();
}
