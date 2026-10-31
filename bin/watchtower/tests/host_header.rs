//! 実バイナリでの Host ヘッダの許可リスト (DNS rebinding 対策)。

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start(data: &std::path::Path, port: u16, extra_env: &[(&str, &str)]) -> Child {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{port}"))
        .env("WATCHTOWER_BASE_URL", "http://watch.example.com:5000")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    for (k, v) in extra_env {
        c.env(k, v);
    }
    let child = c.arg("run").spawn().unwrap();
    for _ in 0..80 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    child
}

/// 指定した Host で /healthz を叩き、ステータス行を返す。
async fn status_line(port: u16, path: &str, host: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.lines().next().unwrap_or_default().to_string()
}

#[tokio::test]
async fn only_expected_host_names_reach_the_web_ui_and_the_api() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut server = start(
        d.path(),
        port,
        &[("WATCHTOWER_ALLOWED_HOSTS", r#"["alias.example.org"]"#)],
    )
    .await;

    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        "watch.example.com:5000".to_string(),
        "alias.example.org".to_string(),
    ] {
        let line = status_line(port, "/healthz", &host).await;
        assert!(line.contains("200"), "{host}: {line}");
    }
    for path in ["/healthz", "/login", "/api/v1/watch", "/metrics"] {
        let line = status_line(port, path, "evil.example").await;
        assert!(line.contains("421"), "{path}: {line}");
    }
    server.kill().await.unwrap();
}

#[tokio::test]
async fn a_bad_allowed_hosts_entry_stops_startup() {
    let d = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_watchtower"))
        .env("WATCHTOWER_DATA_DIR", d.path())
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{}", free_port()))
        .env("WATCHTOWER_ALLOWED_HOSTS", r#"["*.example.com"]"#)
        .env("RUST_LOG", "warn")
        .arg("run")
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("allowed_hosts"), "{err}");
}
