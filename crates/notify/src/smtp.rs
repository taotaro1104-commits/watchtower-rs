//! SMTP での送信。接続先は SSRF ポリシーで検査し、検査した IP に直接つなぐ。

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor};
use tokio_util::sync::CancellationToken;
use watchtower_fetch::SsrfPolicy;

use crate::mail::{MailTarget, Security};
use crate::{Message, Notifier, NotifyError, NotifyTarget};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// I/O ごとのタイムアウトでは、少しずつ応答する相手で長引くので、全体にも上限を設ける。
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SUBJECT_CHARS: usize = 200;
const MAX_BODY_CHARS: usize = 100_000;

pub struct SmtpNotifier {
    policy: Arc<SsrfPolicy>,
    total_timeout: Duration,
}

impl SmtpNotifier {
    pub fn new(policy: Arc<SsrfPolicy>) -> Self {
        Self {
            policy,
            total_timeout: TOTAL_TIMEOUT,
        }
    }

    /// 1 通の送信 (名前解決から応答まで) にかける時間の上限を変える。
    pub fn with_total_timeout(mut self, total: Duration) -> Self {
        self.total_timeout = total;
        self
    }

    /// 名前解決し、ポリシーが許す最初のアドレスを返す。
    async fn pick_address(&self, host: &str, port: u16) -> Result<IpAddr, NotifyError> {
        let candidates: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port)))
                .await
                .map_err(|_| NotifyError::Delivery("could not resolve the SMTP host".into()))?
                .map_err(|_| NotifyError::Delivery("could not resolve the SMTP host".into()))?
                .map(|a| a.ip())
                .collect(),
        };
        candidates
            .into_iter()
            .find(|ip| self.policy.is_allowed(*ip))
            .ok_or_else(|| NotifyError::Delivery("destination blocked by SSRF policy".into()))
    }

    async fn deliver(&self, mail: &MailTarget, msg: &Message) -> Result<(), NotifyError> {
        let email = build_message(mail, msg)?;
        let ip = self.pick_address(&mail.host, mail.port).await?;

        // 検査した IP に直接つなぎ、証明書の検証にはホスト名を使う (再解決による差し替えを防ぐ)
        let tls_params = || {
            TlsParameters::new_rustls(mail.host.clone())
                .map_err(|_| NotifyError::Delivery("TLS setup failed".into()))
        };
        let tls = match mail.security {
            Security::None => Tls::None,
            Security::StartTls => Tls::Required(tls_params()?),
            Security::Tls => Tls::Wrapper(tls_params()?),
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(ip.to_string())
            .port(mail.port)
            .tls(tls)
            .timeout(Some(SEND_TIMEOUT));
        if let Some((user, pass)) = &mail.credentials {
            builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
        }
        builder
            .build()
            .send(email)
            .await
            .map(drop)
            .map_err(|e| NotifyError::Delivery(describe(&e)))
    }
}

fn single_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

fn build_message(mail: &MailTarget, msg: &Message) -> Result<Mail, NotifyError> {
    let mut b = Mail::builder()
        .from(Mailbox::new(None, mail.from.clone()))
        // ヘッダに改行を持ち込ませない
        .subject(single_line(&msg.title, MAX_SUBJECT_CHARS));
    for to in &mail.to {
        b = b.to(Mailbox::new(None, to.clone()));
    }
    let body: String = msg.body.chars().take(MAX_BODY_CHARS).collect();
    b.header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|_| NotifyError::Delivery("could not build the message".into()))
}

/// サーバの応答文や接続先は載せず、種別だけを返す。
fn describe(e: &lettre::transport::smtp::Error) -> String {
    if e.is_permanent() {
        "SMTP server rejected the message".into()
    } else if e.is_transient() {
        "SMTP server reported a temporary failure".into()
    } else if e.is_timeout() {
        "timed out".into()
    } else if e.is_tls() {
        "TLS error".into()
    } else {
        "SMTP connection failed".into()
    }
}

#[async_trait]
impl Notifier for SmtpNotifier {
    async fn send(
        &self,
        target: &NotifyTarget,
        msg: &Message,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError> {
        let Some(mail) = target.mail() else {
            return Err(NotifyError::Delivery("not an email target".into()));
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(NotifyError::Cancelled),
            res = tokio::time::timeout(self.total_timeout, self.deliver(mail, msg)) => {
                res.unwrap_or_else(|_| Err(NotifyError::Delivery("timed out".into())))
            }
        }
    }
}

/// 宛先の種類で HTTP と SMTP を振り分ける。
pub struct CompositeNotifier {
    http: Arc<dyn Notifier>,
    smtp: Arc<dyn Notifier>,
}

impl CompositeNotifier {
    pub fn new(http: Arc<dyn Notifier>, smtp: Arc<dyn Notifier>) -> Self {
        Self { http, smtp }
    }
}

#[async_trait]
impl Notifier for CompositeNotifier {
    async fn send(
        &self,
        target: &NotifyTarget,
        msg: &Message,
        cancel: CancellationToken,
    ) -> Result<(), NotifyError> {
        if target.is_mail() {
            self.smtp.send(target, msg, cancel).await
        } else {
            self.http.send(target, msg, cancel).await
        }
    }
}
