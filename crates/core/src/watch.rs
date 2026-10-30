use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::config::ConfigLayer;

pub type WatchId = Uuid;
pub type TagId = Uuid;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WatchState {
    pub next_check_at: Option<OffsetDateTime>,
    pub last_checked_at: Option<OffsetDateTime>,
    pub last_changed_at: Option<OffsetDateTime>,
    pub last_error: Option<String>,
    pub consecutive_errors: u32,
}

/// ウォッチをまとめる単位。設定の中間層 (Watch > Tag > Global) を持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub id: TagId,
    pub title: String,
    pub config: ConfigLayer,
}

impl Tag {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            config: ConfigLayer::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: WatchId,
    pub url: Url,
    pub title: Option<String>,
    pub tags: Vec<TagId>,
    pub paused: bool,
    pub config: ConfigLayer,
    pub state: WatchState,
}

impl Watch {
    pub fn new(url: Url) -> Self {
        Self {
            id: Uuid::new_v4(),
            url,
            title: None,
            tags: Vec::new(),
            paused: false,
            config: ConfigLayer::default(),
            state: WatchState::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub watch_id: WatchId,
    pub taken_at: OffsetDateTime,
    /// blake3。同一内容なら新規保存しない。
    pub content_hash: [u8; 32],
}
