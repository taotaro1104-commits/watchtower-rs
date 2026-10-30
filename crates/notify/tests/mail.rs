//! メール通知 (mailto / mailtos)。偽の SMTP サーバで送信内容を確かめる。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use watchtower_fetch::SsrfPolicy;
use watchtower_notify::{Message, Notifier, NotifyError, NotifyTarget, SmtpNotifier};

fn msg() -> Message {
    Message {
        title: "価格が変わりました".into(),
        body: "- 10\n+ 20".into(),
    }
}

// ---- 宛先 URL の解釈 -------------------------------------------------------

#[test]
fn mailto_urls_parse_with_defaults_and_options() {
    let t = NotifyTarget::parse(
        "mailto://user%40example.com:p%40ss@smtp.example.com/?to=a@example.org,b@example.org",
    )
    .unwrap();
    assert_eq!(t.scheme(), "mailto");
    assert!(t.is_mail());

    for ok in [
        "mailto://smtp.example.com:2525/?to=a@example.org&from=w@example.com&security=none",
        "mailtos://u@example.com:pw@smtp.example.com/?to=a@example.org",
        "MAILTO://smtp.example.com/?to=a@example.org&from=w@example.com",
    ] {
        assert!(NotifyTarget::parse(ok).is_ok(), "{ok}");
    }
}

#[test]
fn malformed_mail_targets_are_rejected() {
    for bad in [
        "mailto://",
        "mailto://smtp.example.com/",
        "mailto://smtp.example.com/?to=",
        "mailto://smtp.example.com/?to=not-an-address",
        "mailto://smtp.example.com/?to=a@example.org", // from が決まらない
        "mailto://smtp.example.com/?to=a@example.org&from=nope",
        "mailto://smtp.example.com/?to=a@example.org&from=w@example.com&security=weird",
        "mailto://smtp.example.com/?to=a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c,a@b.c&from=w@example.com",
        "mailto://smtp.example.com:99999/?to=a@example.org&from=w@example.com",
    ] {
        assert!(
            matches!(NotifyTarget::parse(bad), Err(NotifyError::InvalidUrl(_))),
            "{bad}"
        );
    }
}

#[test]
fn display_and_debug_hide_credentials_and_addresses() {
    let t = NotifyTarget::parse(
        "mailto://bob%40example.com:SECRETpass@smtp.example.com/?to=alice@example.org",
    )
    .unwrap();
    for s in [t.to_string(), format!("{t:?}")] {
        assert!(
            !s.contains("SECRET") && !s.contains("bob") && !s.contains("alice"),
            "{s}"
        );
        assert!(s.contains("smtp.example.com"), "{s}");
    }
}

#[test]
fn mail_targets_have_no_http_request() {
    let t =
        NotifyTarget::parse("mailto://h.example.com/?to=a@example.org&from=w@example.com").unwrap();
    assert!(t.build_request(&msg()).is_err());
}

// ---- 偽 SMTP サーバ -------------------------------------------------------

struct Fake {
    port: u16,
    handle: JoinHandle<Vec<String>>,
}

/// 1 接続だけ受けて、クライアントが送った行 (DATA 本文を含む) を返す。
async fn fake_smtp(auth: bool) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (r, mut w) = sock.into_split();
        let mut r = BufReader::new(r);
        let mut seen = Vec::new();
        w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let l = line.trim_end().to_string();
            seen.push(l.clone());
            let up = l.to_ascii_uppercase();
            if up.starts_with("EHLO") {
                let caps = if auth {
                    "250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
                } else {
                    "250-fake\r\n250 8BITMIME\r\n"
                };
                w.write_all(caps.as_bytes()).await.unwrap();
            } else if up.starts_with("AUTH") {
                w.write_all(b"235 ok\r\n").await.unwrap();
            } else if up.starts_with("DATA") {
                w.write_all(b"354 go\r\n").await.unwrap();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    if line == ".\r\n" {
                        break;
                    }
                    seen.push(format!("DATA| {}", line.trim_end()));
                }
                w.write_all(b"250 queued\r\n").await.unwrap();
            } else if up.starts_with("QUIT") {
                w.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                w.write_all(b"250 ok\r\n").await.unwrap();
            }
        }
        seen
    });
    Fake { port, handle }
}

fn notifier() -> SmtpNotifier {
    SmtpNotifier::new(Arc::new(SsrfPolicy::allow_loopback()))
}

async fn deliver(url: &str, m: &Message) -> Result<(), NotifyError> {
    let t = NotifyTarget::parse(url).unwrap();
    notifier().send(&t, m, CancellationToken::new()).await
}

async fn transcript(f: Fake) -> String {
    tokio::time::timeout(Duration::from_secs(5), f.handle)
        .await
        .expect("server finished")
        .unwrap()
        .join("\n")
}

#[tokio::test]
async fn a_message_is_delivered_to_every_recipient() {
    let f = fake_smtp(false).await;
    let url = format!(
        "mailto://127.0.0.1:{}/?to=a@example.org,b@example.org&from=watch@example.com&security=none",
        f.port
    );
    deliver(&url, &msg()).await.unwrap();
    let t = transcript(f).await;
    assert!(t.contains("MAIL FROM:<watch@example.com>"), "{t}");
    assert!(
        t.contains("RCPT TO:<a@example.org>") && t.contains("RCPT TO:<b@example.org>"),
        "{t}"
    );
    assert!(t.contains("DATA| Subject:"), "{t}");
}

#[tokio::test]
async fn non_ascii_text_is_encoded_and_headers_cannot_be_injected() {
    let f = fake_smtp(false).await;
    let url = format!(
        "mailto://127.0.0.1:{}/?to=a@example.org&from=watch@example.com&security=none",
        f.port
    );
    let evil = Message {
        title: "hi\r\nBcc: attacker@example.net".into(),
        body: "body\r\n.\r\nRCPT TO:<attacker@example.net>".into(),
    };
    deliver(&url, &evil).await.unwrap();
    let t = transcript(f).await;
    assert!(!t.contains("DATA| Bcc:"), "{t}");
    // SMTP コマンドとして届いた RCPT は宛先の 1 件だけ。本文中の文字列は DATA の一部で、
    // 単独の "." は二重化されて DATA を途中で終わらせない。
    let commands = t.lines().filter(|l| l.starts_with("RCPT TO")).count();
    assert_eq!(commands, 1, "{t}");
    assert!(t.contains("DATA| .."), "dot-stuffing expected: {t}");

    let f = fake_smtp(false).await;
    let url = format!(
        "mailto://127.0.0.1:{}/?to=a@example.org&from=watch@example.com&security=none",
        f.port
    );
    deliver(&url, &msg()).await.unwrap();
    assert!(transcript(f).await.contains("DATA| Subject:"));
}

#[tokio::test]
async fn private_smtp_hosts_are_blocked_without_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
    });
    let strict = SmtpNotifier::new(Arc::new(SsrfPolicy::default()));
    let t = NotifyTarget::parse(&format!(
        "mailto://127.0.0.1:{port}/?to=a@example.org&from=w@example.com&security=none"
    ))
    .unwrap();
    let err = strict
        .send(&t, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
    assert!(err.to_string().contains("blocked"), "{err}");
    assert!(!accepted.await.unwrap(), "no connection may be attempted");
}

#[tokio::test]
async fn delivery_errors_never_contain_credentials() {
    // 何も待ち受けていないポートへ
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!(
        "mailto://bob%40example.com:SECRETpass@127.0.0.1:{port}/?to=alice@example.org&security=starttls"
    );
    let err = deliver(&url, &msg()).await.unwrap_err();
    let s = err.to_string();
    assert!(
        !s.contains("SECRET") && !s.contains("bob") && !s.contains("alice"),
        "{s}"
    );
}

#[test]
fn credentials_with_an_unencrypted_connection_are_refused_when_saved() {
    let url = "mailto://bob%40example.com:pw@smtp.example.com/?to=a@example.org&security=none";
    assert!(matches!(
        NotifyTarget::parse(url),
        Err(NotifyError::InvalidUrl(_))
    ));
    // 認証なしなら暗号化なしも許す (社内リレーなど)
    assert!(NotifyTarget::parse(
        "mailto://smtp.example.com/?to=a@example.org&from=w@example.com&security=none"
    )
    .is_ok());
}

#[tokio::test]
async fn cancellation_is_reported_as_cancelled() {
    let f = fake_smtp(false).await;
    let url = format!(
        "mailto://127.0.0.1:{}/?to=a@example.org&from=watch@example.com&security=none",
        f.port
    );
    let t = NotifyTarget::parse(&url).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = notifier().send(&t, &msg(), cancel).await.unwrap_err();
    assert!(matches!(err, NotifyError::Cancelled), "{err:?}");
    f.handle.abort();
}

#[tokio::test]
async fn non_mail_targets_are_refused_by_the_smtp_notifier() {
    let t = NotifyTarget::parse("json://h.example.com/x").unwrap();
    let err = notifier()
        .send(&t, &msg(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)));
}
