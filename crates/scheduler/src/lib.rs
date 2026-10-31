//! 期限キュー・ワーカー制御・チェック処理。

mod backoff;
mod checker;
mod error;
mod host_limits;
mod queue;
mod runner;
mod scheduler;

pub use backoff::{apply_jitter, error_delay, random_jitter, success_delay};
pub use checker::{CheckOutcome, CheckerConfig, StoreCheckRunner};
pub use error::SchedulerError;
pub use queue::DueQueue;
pub use runner::{CheckRunner, RunReport, ScheduledWatch};
pub use scheduler::{Command, Scheduler, SchedulerConfig};
pub use watchtower_core::WatchId;
