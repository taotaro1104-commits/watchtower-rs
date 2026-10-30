//! 通知層。Apprise 形式の宛先 URL、テンプレート、送信。

mod apprise;
mod dispatcher;
mod error;
mod foreign;
mod mail;
mod notifier;
mod request;
mod smtp;
mod target;
mod template;
mod worker;

pub use apprise::AppriseApiNotifier;
pub use dispatcher::{ChangeEvent, Dispatcher};
pub use error::NotifyError;
pub use foreign::allow_foreign_targets;
pub use notifier::{HttpNotifier, Notifier};
pub use smtp::{CompositeNotifier, SmtpNotifier};
pub use target::NotifyTarget;
pub use template::{NotificationContext, TemplateSet};
pub use worker::{OutboxWorker, RetryPolicy};

/// 送信直前の通知。テンプレート描画後のプレーンテキスト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
}
