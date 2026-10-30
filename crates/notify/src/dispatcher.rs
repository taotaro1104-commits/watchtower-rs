//! 変化の検知結果から、送るべき通知を決めて outbox に積む。

use time::OffsetDateTime;
use watchtower_core::{ConfigLayer, EffectiveConfig, WatchId};
use watchtower_process::DiffResult;
use watchtower_store::{NewNotification, Store};

use crate::{Message, NotificationContext, NotifyError, NotifyTarget, TemplateSet};

const MAX_STORED_TITLE_CHARS: usize = 500;
const MAX_STORED_BODY_CHARS: usize = 16_000;

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_string(),
    }
}

/// 内容が変わったことを表す入力。初回の基準保存やエラーは対象外。
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub watch_id: WatchId,
    pub diff: DiffResult,
    /// `trigger_text` に一致した、または価格が `price_below` 以下になった。
    pub triggered: bool,
}

pub struct Dispatcher {
    store: Store,
    global: ConfigLayer,
    templates: TemplateSet,
    /// 通知本文のプレビューリンクの基点 (例: `http://host:5000`)。
    base_url: String,
}

impl Dispatcher {
    pub fn new(
        store: Store,
        global: ConfigLayer,
        templates: TemplateSet,
        base_url: String,
    ) -> Self {
        Self {
            store,
            global,
            templates,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// 送るべき通知を outbox に積み、積んだ件数を返す。
    pub async fn on_change(
        &self,
        ev: &ChangeEvent,
        now: OffsetDateTime,
    ) -> Result<usize, NotifyError> {
        let Some(watch) = self.store.get_watch(ev.watch_id).await? else {
            return Ok(0);
        };
        let tags = self.store.tag_layers(&watch.tags).await?;
        // 設定ファイルの共通設定に、設定画面で保存した共通の通知先を足す
        let mut global = self.global.clone();
        global
            .notify_urls
            .extend(self.store.global_notify_urls().await?);
        let cfg = EffectiveConfig::resolve(&watch.config, &tags, &global);
        if cfg.notify_muted || cfg.notify_urls.is_empty() {
            return Ok(0);
        }
        // トリガーテキストか価格の閾値を設定したウォッチは、条件に合ったときだけ通知する。
        let gated = !cfg.trigger_text.is_empty() || cfg.price_below.is_some();
        if gated && !ev.triggered {
            return Ok(0);
        }

        let ctx = NotificationContext {
            watch_url: watch.url.to_string(),
            watch_title: watch.title.clone().unwrap_or_else(|| watch.url.to_string()),
            diff: format_diff(&ev.diff),
            diff_added: ev.diff.added.join("\n"),
            diff_removed: ev.diff.removed.join("\n"),
            current_snapshot: self.current_text(ev.watch_id).await,
            preview_url: format!("{}/preview/{}", self.base_url, watch.id),
        };
        let msg = self.templates.render(&ctx).unwrap_or_else(|e| {
            // 描画に失敗しても変化の通知自体は落とさない。
            tracing::warn!(watch = %watch.id, "template failed, using default: {e}");
            TemplateSet::default()
                .render(&ctx)
                .expect("default template renders")
        });

        // 保存する量に上限を設ける (送信先ごとの上限は送信時にさらに切り詰める)
        let msg = Message {
            title: truncate_chars(&msg.title, MAX_STORED_TITLE_CHARS),
            body: truncate_chars(&msg.body, MAX_STORED_BODY_CHARS),
        };
        let mut queued = 0;
        for raw in &cfg.notify_urls {
            if let Err(e) = NotifyTarget::parse(raw) {
                tracing::warn!(watch = %watch.id, "skipping invalid notification URL: {e}");
                continue;
            }
            let n = NewNotification {
                watch_id: Some(watch.id),
                target: raw.clone(),
                title: msg.title.clone(),
                body: msg.body.clone(),
            };
            self.store.enqueue_notification(&n, now).await?;
            queued += 1;
        }
        Ok(queued)
    }

    async fn current_text(&self, id: WatchId) -> String {
        let rec = match self.store.latest_snapshot(id).await {
            Ok(Some(rec)) => rec,
            Ok(None) => return String::new(),
            Err(e) => {
                tracing::warn!(watch = %id, "latest snapshot lookup failed: {e}");
                return String::new();
            }
        };
        self.store.read_snapshot(&rec).await.unwrap_or_else(|e| {
            tracing::warn!(watch = %id, "snapshot unreadable for notification: {e}");
            String::new()
        })
    }
}

/// 削除行 (`- `) → 追加行 (`+ `) の順。CLI の表示と同じ形。
fn format_diff(d: &DiffResult) -> String {
    d.removed
        .iter()
        .map(|l| format!("- {l}"))
        .chain(d.added.iter().map(|l| format!("+ {l}")))
        .collect::<Vec<_>>()
        .join("\n")
}
