//! watchtower-rs のドメイン型と trait。I/O には依存しない。

pub mod config;
pub mod effective;
pub mod error;
pub mod watch;

pub use config::ConfigLayer;
pub use effective::EffectiveConfig;
pub use error::CoreError;
pub use watch::{Snapshot, Tag, TagId, Watch, WatchId, WatchState};
