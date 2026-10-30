//! 実バイナリを起動する E2E テスト。

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use tokio::process::Command;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cmd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        // wiremock は 127.0.0.1 で待ち受けるため許可する
        .env("WATCHTOWER_ALLOWED_IPS", r#"["127.0.0.1"]"#)
        .env("RUST_LOG", "warn")
        // 並列に走るテスト同士で Web UI のポートが衝突しないようにする
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{}", free_port()))
        .kill_on_drop(true);
    c
}

async fn wt(data: &Path, args: &[&str]) -> Output {
    cmd(data).args(args).output().await.unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn id_from_add(o: &Output) -> String {
    out(o).split_whitespace().last().unwrap().to_string()
}

#[tokio::test]
async fn add_then_list_shows_the_watch() {
    let d = tempfile::tempdir().unwrap();
    let add = wt(
        d.path(),
        &[
            "watch",
            "add",
            "https://example.com/p",
            "--title",
            "Example",
            "--interval",
            "120",
        ],
    )
    .await;
    assert!(add.status.success(), "{}", err(&add));
    let id = id_from_add(&add);
    let list = wt(d.path(), &["watch", "list"]).await;
    let text = out(&list);
    assert!(
        text.contains(&id) && text.contains("https://example.com/p") && text.contains("Example"),
        "{text}"
    );
}

#[tokio::test]
async fn add_rejects_non_http_urls() {
    let d = tempfile::tempdir().unwrap();
    let add = wt(d.path(), &["watch", "add", "ftp://example.com/x"]).await;
    assert!(!add.status.success());
    assert!(err(&add).contains("http"), "{}", err(&add));
    let add = wt(d.path(), &["watch", "add", "not a url"]).await;
    assert!(!add.status.success());
}

#[tokio::test]
async fn remove_deletes_the_watch() {
    let d = tempfile::tempdir().unwrap();
    let id = id_from_add(&wt(d.path(), &["watch", "add", "https://example.com/"]).await);
    assert!(wt(d.path(), &["watch", "remove", &id])
        .await
        .status
        .success());
    assert!(!out(&wt(d.path(), &["watch", "list"]).await).contains(&id));
    assert!(
        !wt(d.path(), &["watch", "remove", &id])
            .await
            .status
            .success(),
        "unknown id must fail"
    );
}

#[tokio::test]
async fn check_reports_baseline_then_change_with_diff() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("price: 10", "text/plain"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("price: 20", "text/plain"))
        .with_priority(2)
        .mount(&server)
        .await;

    let d = tempfile::tempdir().unwrap();
    let url = format!("{}/p", server.uri());
    let id = id_from_add(&wt(d.path(), &["watch", "add", &url]).await);

    let first = wt(d.path(), &["check", &id]).await;
    assert!(first.status.success(), "{}", err(&first));
    assert!(out(&first).contains("baseline"), "{}", out(&first));

    let second = wt(d.path(), &["check", "--all"]).await;
    let text = out(&second);
    assert!(second.status.success(), "{}", err(&second));
    assert!(text.contains("changed"), "{text}");
    assert!(
        text.contains("- price: 10") && text.contains("+ price: 20"),
        "{text}"
    );
}

#[tokio::test]
async fn check_fails_with_nonzero_exit_on_fetch_error() {
    let server = MockServer::start().await;
    Mock::given(path("/e"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let d = tempfile::tempdir().unwrap();
    let id = id_from_add(&wt(d.path(), &["watch", "add", &format!("{}/e", server.uri())]).await);
    let res = wt(d.path(), &["check", &id]).await;
    assert!(!res.status.success());
    assert!(
        out(&res).contains("error") && out(&res).contains("500"),
        "{}",
        out(&res)
    );
}

#[tokio::test]
async fn private_addresses_are_blocked_by_default() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x"))
        .mount(&server)
        .await;
    let d = tempfile::tempdir().unwrap();
    let url = format!("{}/p", server.uri());
    let id = id_from_add(&wt(d.path(), &["watch", "add", &url]).await);
    let mut c = cmd(d.path());
    c.env("WATCHTOWER_ALLOWED_IPS", "[]");
    let res = c.args(["check", &id]).output().await.unwrap();
    assert!(!res.status.success());
    assert!(
        out(&res).to_lowercase().contains("blocked"),
        "{}",
        out(&res)
    );
}

#[tokio::test]
async fn run_checks_watches_periodically() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("hello", "text/plain"))
        .mount(&server)
        .await;
    let d = tempfile::tempdir().unwrap();
    let url = format!("{}/p", server.uri());
    wt(d.path(), &["watch", "add", &url, "--interval", "1"]).await;

    let mut child = cmd(d.path()).arg("run").spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await;
    child.kill().await.unwrap();

    let list = out(&wt(d.path(), &["watch", "list"]).await);
    assert!(
        !list.contains("never"),
        "watch should have been checked:\n{list}"
    );
    let received = server.received_requests().await.unwrap().len();
    assert!(received >= 2, "expected repeated checks, got {received}");
}

#[tokio::test]
async fn add_rejects_zero_interval() {
    let d = tempfile::tempdir().unwrap();
    let add = wt(
        d.path(),
        &["watch", "add", "https://example.com/", "--interval", "0"],
    )
    .await;
    assert!(!add.status.success());
    assert!(err(&add).contains("interval"), "{}", err(&add));
}

#[tokio::test]
async fn add_rejects_invalid_notify_url_without_echoing_it() {
    let d = tempfile::tempdir().unwrap();
    let add = wt(
        d.path(),
        &[
            "watch",
            "add",
            "https://example.com/",
            "--notify",
            "bogus://SECRETtoken/x",
        ],
    )
    .await;
    assert!(!add.status.success());
    assert!(
        err(&add).contains("invalid notification target"),
        "{}",
        err(&add)
    );
    assert!(!err(&add).contains("SECRET"), "{}", err(&add));
}

#[tokio::test]
async fn run_delivers_a_webhook_when_the_page_changes() {
    let server = MockServer::start().await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("version one", "text/plain"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("version two", "text/plain"))
        .with_priority(2)
        .mount(&server)
        .await;
    Mock::given(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let d = tempfile::tempdir().unwrap();
    let url = format!("{}/p", server.uri());
    let hook = format!("json://{}/hook", server.address());
    let add = wt(
        d.path(),
        &["watch", "add", &url, "--interval", "1", "--notify", &hook],
    )
    .await;
    assert!(add.status.success(), "{}", err(&add));

    let mut child = cmd(d.path()).arg("run").spawn().unwrap();
    let mut delivered = None;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let reqs = server.received_requests().await.unwrap();
        delivered = reqs.into_iter().find(|r| r.url.path() == "/hook");
        if delivered.is_some() {
            break;
        }
    }
    child.kill().await.unwrap();

    let req = delivered.expect("webhook should have been called");
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("+ version two") && message.contains("- version one"),
        "{message}"
    );
}

#[tokio::test]
async fn import_loads_a_changedetection_file_and_reports_the_result() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("url-watches.json");
    std::fs::write(
        &file,
        r#"{"watching": {
          "11111111-1111-4111-8111-111111111111": {"url": "https://example.com/a", "title": "Imported"},
          "22222222-2222-4222-8222-222222222222": {"url": "ftp://bad"}
        }}"#,
    )
    .unwrap();
    let res = wt(d.path(), &["import", file.to_str().unwrap()]).await;
    assert!(res.status.success(), "{}", err(&res));
    let text = out(&res);
    assert!(
        text.contains("imported 1") && text.contains("skipped"),
        "{text}"
    );
    assert!(out(&wt(d.path(), &["watch", "list"]).await).contains("Imported"));

    let missing = wt(d.path(), &["import", "no-such-file.json"]).await;
    assert!(!missing.status.success());
}

async fn with_stdin(mut c: Command, input: &str) -> Output {
    use tokio::io::AsyncWriteExt;
    c.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).await.unwrap();
    drop(stdin);
    child.wait_with_output().await.unwrap()
}

#[tokio::test]
async fn set_password_reads_stdin_and_enforces_a_minimum_length() {
    let d = tempfile::tempdir().unwrap();
    let mut c = cmd(d.path());
    c.arg("set-password");
    let short = with_stdin(c, "short\n").await;
    assert!(!short.status.success());
    assert!(err(&short).contains("at least"), "{}", err(&short));

    let mut c = cmd(d.path());
    c.arg("set-password");
    let ok = with_stdin(c, "a long enough password\n").await;
    assert!(ok.status.success(), "{}", err(&ok));
    assert!(
        !out(&ok).contains("long enough"),
        "must not echo the password"
    );
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn http_get(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
async fn run_serves_the_web_ui() {
    let d = tempfile::tempdir().unwrap();
    let mut c = cmd(d.path());
    c.arg("set-password");
    assert!(with_stdin(c, "a long enough password\n")
        .await
        .status
        .success());

    let port = free_port();
    let mut child = cmd(d.path())
        .env("WATCHTOWER_LISTEN", format!("127.0.0.1:{port}"))
        .arg("run")
        .spawn()
        .unwrap();
    let mut health = String::new();
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            health = http_get(port, "/healthz").await;
            break;
        }
    }
    let root = http_get(port, "/").await;
    child.kill().await.unwrap();
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(
        root.starts_with("HTTP/1.1 303") && root.contains("location: /login"),
        "{root}"
    );
}
