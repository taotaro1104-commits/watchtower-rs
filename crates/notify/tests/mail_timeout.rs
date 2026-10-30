//! SMTP 送信全体の時間制限 (I/O ごとのタイムアウトだけでは、少しずつ応答する相手で長引く)。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use watchtower_fetch::SsrfPolicy;
use watchtower_notify::{Message, Notifier, NotifyError, NotifyTarget, SmtpNotifier};

#[tokio::test]
async fn a_server_that_trickles_replies_cannot_hold_a_send_forever() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // 1 バイトずつ、ゆっくり挨拶を送り続ける
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        for b in b"220 trickling along very slowly........................................" {
            if sock.write_all(&[*b]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let notifier = SmtpNotifier::new(Arc::new(SsrfPolicy::allow_loopback()))
        .with_total_timeout(Duration::from_millis(400));
    let target = NotifyTarget::parse(&format!(
        "mailto://127.0.0.1:{port}/?to=a@example.org&from=w@example.com&security=none"
    ))
    .unwrap();
    let msg = Message {
        title: "t".into(),
        body: "b".into(),
    };
    let started = Instant::now();
    let err = notifier
        .send(&target, &msg, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, NotifyError::Delivery(_)), "{err:?}");
    assert!(err.to_string().contains("timed out"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
