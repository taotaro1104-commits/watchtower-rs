//! 実バイナリでブラウザ取得を使うウォッチを確かめる。

use std::path::Path;
use std::process::Output;

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
async fn a_malformed_browser_url_stops_startup_without_echoing_it() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        "not a url",
        "http://chrome:9222/devtools/browser/SECRET-TOKEN",
        "ftp://x/SECRET-TOKEN",
    ] {
        let o = cmd(d.path())
            .env("WATCHTOWER_BROWSER_WS_URL", bad)
            .args(["watch", "list"])
            .output()
            .await
            .unwrap();
        assert!(!o.status.success(), "{bad}");
        assert!(!err(&o).contains("SECRET"), "{}", err(&o));
    }
}

#[tokio::test]
async fn a_browser_watch_without_a_configured_browser_fails_with_a_clear_message() {
    let d = tempfile::tempdir().unwrap();
    let add = cmd(d.path())
        .args(["watch", "add", "https://example.com/", "--browser"])
        .output()
        .await
        .unwrap();
    assert!(add.status.success(), "{}", err(&add));
    let check = cmd(d.path())
        .args(["check", &id_from_add(&add)])
        .output()
        .await
        .unwrap();
    assert!(!check.status.success());
    assert!(out(&check).contains("browser"), "{}", out(&check));
}

/// 実際の Chrome が要る。`WATCHTOWER_TEST_CHROME_WS` に webSocketDebuggerUrl を入れて手動で実行する。
///   cargo test -p watchtower --test browser -- --ignored
#[tokio::test]
#[ignore = "needs a real Chrome (set WATCHTOWER_TEST_CHROME_WS)"]
async fn a_javascript_page_is_snapshotted_as_rendered() {
    let ws = std::env::var("WATCHTOWER_TEST_CHROME_WS").expect("WATCHTOWER_TEST_CHROME_WS");
    let site = MockServer::start().await;
    let page = |msg: &str| {
        format!(
            "<html><body><div id=x>loading</div><script>document.getElementById('x').textContent='{msg}';</script></body></html>"
        )
    };
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(page("price is 100"), "text/html"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&site)
        .await;
    Mock::given(path("/p"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(page("price is 80"), "text/html"))
        .with_priority(2)
        .mount(&site)
        .await;

    let d = tempfile::tempdir().unwrap();
    let url = format!("{}/p", site.uri());
    let add = cmd(d.path())
        .args(["watch", "add", &url, "--browser"])
        .output()
        .await
        .unwrap();
    let id = id_from_add(&add);
    let run = |d: &Path, id: &str| {
        let mut c = cmd(d);
        c.env("WATCHTOWER_BROWSER_WS_URL", &ws).args(["check", id]);
        c
    };
    let first = run(d.path(), &id).output().await.unwrap();
    assert!(first.status.success(), "{} {}", out(&first), err(&first));
    assert!(out(&first).contains("baseline"), "{}", out(&first));

    let second = run(d.path(), &id).output().await.unwrap();
    let text = out(&second);
    assert!(text.contains("changed"), "{text}");
    assert!(
        text.contains("- price is 100") && text.contains("+ price is 80"),
        "the diff must be about the RENDERED text: {text}"
    );
}
