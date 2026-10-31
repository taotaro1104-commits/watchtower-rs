//! `healthcheck`: コンテナの HEALTHCHECK 用。distroless には curl も shell も無いので、バイナリ自身が確かめる。

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cmd(data: &Path, listen: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        .env("WATCHTOWER_LISTEN", listen)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

/// 1 回だけ応答して終わる偽のサーバ。
async fn fake_server(status_line: &'static str, body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = s.read(&mut buf).await;
        let resp = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = s.write_all(resp.as_bytes()).await;
    });
    port
}

#[tokio::test]
async fn a_running_server_is_healthy() {
    let d = tempfile::tempdir().unwrap();
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let mut server = cmd(d.path(), &listen).arg("run").spawn().unwrap();
    for _ in 0..80 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ok = cmd(d.path(), &listen)
        .arg("healthcheck")
        .output()
        .await
        .unwrap();
    server.kill().await.unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

#[tokio::test]
async fn nothing_listening_is_unhealthy_and_fails_fast() {
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let r = cmd(d.path(), &format!("127.0.0.1:{}", free_port()))
        .arg("healthcheck")
        .output()
        .await
        .unwrap();
    assert!(!r.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn an_error_status_or_a_wrong_body_is_unhealthy() {
    let d = tempfile::tempdir().unwrap();
    for (status, body) in [
        ("500 Internal Server Error", "ok"),
        ("200 OK", "not ok"),
        ("404 Not Found", ""),
    ] {
        let port = fake_server(status, body).await;
        let r = cmd(d.path(), &format!("127.0.0.1:{port}"))
            .arg("healthcheck")
            .output()
            .await
            .unwrap();
        assert!(!r.status.success(), "{status} / {body}");
    }
}

#[tokio::test]
async fn a_good_response_from_any_server_on_the_port_counts_as_healthy() {
    let d = tempfile::tempdir().unwrap();
    let port = fake_server("200 OK", "ok").await;
    let r = cmd(d.path(), &format!("127.0.0.1:{port}"))
        .arg("healthcheck")
        .output()
        .await
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}

#[tokio::test]
async fn a_wildcard_listen_address_is_checked_through_loopback() {
    // コンテナでは 0.0.0.0 で待ち受ける。ヘルスチェックは自分自身 (127.0.0.1) に問い合わせる。
    let d = tempfile::tempdir().unwrap();
    let port = fake_server("200 OK", "ok").await;
    let r = cmd(d.path(), &format!("0.0.0.0:{port}"))
        .arg("healthcheck")
        .output()
        .await
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let d = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (_s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await; // 応答せずに握り続ける
    });
    let started = Instant::now();
    let r = cmd(d.path(), &format!("127.0.0.1:{port}"))
        .arg("healthcheck")
        .output()
        .await
        .unwrap();
    assert!(!r.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}
