//! REST API v1 (changedetection.io 互換のパス構成、`x-api-key` 認証)。

mod auth;
mod error;
mod history;
mod import;
mod json;
mod metrics;
mod model;
mod openapi;
mod rss;
mod system;
mod tags;
mod watches;

pub use auth::{generate_key, hash_key};
pub use error::ApiError;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue};
use axum::middleware::{from_fn_with_state, map_response};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use tokio::sync::mpsc;
use watchtower_metrics::Metrics;
use watchtower_scheduler::Command;
use watchtower_store::Store;

/// JSON 本文の上限。
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// 一括インポートの本文の上限。
const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;
/// スケジューラのキューが空くのを待つ上限。
const SCHEDULER_SEND_TIMEOUT: Duration = Duration::from_secs(5);

struct Inner {
    store: Store,
    scheduler: Option<mpsc::Sender<Command>>,
    started: Instant,
    metrics: Arc<Metrics>,
    /// RSS のチャンネルのリンクなど、外から見える自分の基点 URL。
    base_url: String,
}

#[derive(Clone)]
pub struct ApiState(Arc<Inner>);

impl ApiState {
    pub fn new(store: Store, scheduler: Option<mpsc::Sender<Command>>) -> Self {
        Self::with_metrics(store, scheduler, Arc::new(Metrics::new()))
    }

    /// `/metrics` が読むレジストリを共有する (チェッカー・スケジューラ・通知と同じもの)。
    pub fn with_metrics(
        store: Store,
        scheduler: Option<mpsc::Sender<Command>>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self(Arc::new(Inner {
            store,
            scheduler,
            started: Instant::now(),
            metrics,
            base_url: "/".to_string(),
        }))
    }

    /// 外から見える基点 URL (`http(s)://` で始まるものだけ受け付け、それ以外は無視する)。
    pub fn with_base_url(self, base_url: &str) -> Self {
        let base = base_url.trim().trim_end_matches('/');
        let lower = base.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return self;
        }
        let i = &self.0;
        Self(Arc::new(Inner {
            store: i.store.clone(),
            scheduler: i.scheduler.clone(),
            started: i.started,
            metrics: Arc::clone(&i.metrics),
            base_url: format!("{base}/"),
        }))
    }

    /// スケジューラへ伝える。キューが満杯なら空くまで (上限付きで) 待ち、黙って捨てない。
    pub(crate) async fn notify(&self, cmd: Command) {
        let Some(tx) = &self.0.scheduler else {
            return;
        };
        match tokio::time::timeout(SCHEDULER_SEND_TIMEOUT, tx.send(cmd)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => tracing::warn!("scheduler is gone; command not delivered"),
            Err(_) => tracing::warn!("scheduler queue stayed full; command dropped"),
        }
    }
}

pub fn router(state: ApiState) -> Router {
    let v1 = Router::new()
        .route("/watch", get(watches::list).post(watches::create))
        .route(
            "/watch/{id}",
            get(watches::get_one)
                .put(watches::update)
                .delete(watches::remove),
        )
        .route("/watch/{id}/history", get(history::list))
        .route("/watch/{id}/history/{timestamp}", get(history::snapshot))
        .route("/tags", get(tags::list).post(tags::create))
        .route(
            "/import",
            axum::routing::post(import::bulk).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route("/systeminfo", get(system::info))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .layer(from_fn_with_state(state.clone(), auth::require_key));
    Router::new()
        .nest("/api/v1", v1)
        // ドキュメントはキー不要 (API のデータは含まない)
        .route("/api/openapi.json", get(openapi::openapi_json))
        .route("/api/docs", get(openapi::docs_page))
        // RSS リーダー用 (読み取り専用の別トークンを URL で受け取る)
        .route("/rss", get(rss::feed))
        // Prometheus のスクレイプ用 (キー必須。Bearer でも可)
        .route("/metrics", get(metrics::scrape))
        .layer(map_response(common_headers))
        .with_state(state)
}

/// すべての応答: キャッシュさせず、型の推測もさせない。
async fn common_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}
