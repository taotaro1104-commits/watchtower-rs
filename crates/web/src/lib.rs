//! Web UI (axum + askama)。認証・CSRF・エスケープ・セキュリティヘッダを備える。

mod auth;
mod edit;
mod events;
mod handlers;
mod pick;
mod settings;
mod sse;
mod tags;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use watchtower_scheduler::Command;
use watchtower_store::Store;

use crate::auth::{LoginLimiter, Sessions};
pub use crate::pick::PagePicker;

const PASSWORD_SETTING: &str = "web_password_hash";
/// フォームの本文サイズ上限。
const MAX_FORM_BYTES: usize = 64 * 1024;
const MAX_CONCURRENT_VERIFY: usize = 4;
/// スケジューラのキューが空くのを待つ上限。
const SCHEDULER_SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub session_ttl: Duration,
    /// HTTPS の背後に置くときは true (Cookie に Secure を付ける)。
    pub secure_cookies: bool,
    pub max_login_failures: u32,
    pub login_window: Duration,
    /// SSE が履歴の増加を確認する間隔。
    pub sse_poll: Duration,
    /// 同時に開いておける SSE 接続の数。
    pub max_streams: usize,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            session_ttl: Duration::from_secs(12 * 3600),
            secure_cookies: false,
            max_login_failures: 5,
            login_window: Duration::from_secs(300),
            sse_poll: Duration::from_secs(2),
            max_streams: 20,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("password must be at least {0} characters")]
    WeakPassword(usize),
    #[error("hashing failed: {0}")]
    Hash(String),
    #[error(transparent)]
    Store(#[from] watchtower_store::StoreError),
}

struct Inner {
    store: Store,
    cfg: WebConfig,
    sessions: Sessions,
    limiter: LoginLimiter,
    /// 同時に行う Argon2 検証の枠 (メモリ・CPU の枯渇を防ぐ)。
    verify_slots: Semaphore,
    stream_slots: Arc<Semaphore>,
    /// バックアップは同時に 1 つだけ作る。
    backup_slot: Semaphore,
    /// 終了要求。開いたままの SSE 接続を閉じさせるために使う。
    shutdown: CancellationToken,
    scheduler: Option<mpsc::Sender<Command>>,
    /// ビジュアルセレクタ用 (ブラウザがあるときだけ)。
    picker: Option<Arc<dyn PagePicker>>,
    /// 描画は同時に 1 件だけ。
    pick_slot: Semaphore,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl AppState {
    pub fn new(store: Store, cfg: WebConfig) -> Self {
        Self::with_scheduler(store, cfg, None)
    }

    /// 追加・削除をスケジューラへ即時に伝える。
    pub fn with_scheduler(
        store: Store,
        cfg: WebConfig,
        scheduler: Option<mpsc::Sender<Command>>,
    ) -> Self {
        let limiter = LoginLimiter::new(cfg.max_login_failures, cfg.login_window);
        let stream_slots = Arc::new(Semaphore::new(cfg.max_streams));
        Self(Arc::new(Inner {
            store,
            cfg,
            sessions: Sessions::default(),
            limiter,
            verify_slots: Semaphore::new(MAX_CONCURRENT_VERIFY),
            stream_slots,
            backup_slot: Semaphore::new(1),
            shutdown: CancellationToken::new(),
            scheduler,
            picker: None,
            pick_slot: Semaphore::new(1),
        }))
    }

    /// ビジュアルセレクタを有効にする。状態を複製する前 (組み立て中) に呼ぶこと。
    #[must_use]
    pub fn with_picker(mut self, picker: Arc<dyn PagePicker>) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("with_picker must be called before the state is cloned")
            .picker = Some(picker);
        self
    }

    /// ログインパスワードを設定 (または変更) する。ハッシュだけを保存する。
    pub async fn set_password(&self, password: &str) -> Result<(), WebError> {
        if password.chars().count() < auth::MIN_PASSWORD_CHARS {
            return Err(WebError::WeakPassword(auth::MIN_PASSWORD_CHARS));
        }
        let pw = password.to_string();
        let hash = tokio::task::spawn_blocking(move || auth::hash_password(&pw))
            .await
            .map_err(|e| WebError::Hash(e.to_string()))?
            .map_err(WebError::Hash)?;
        self.0
            .store
            .set_setting(PASSWORD_SETTING, &Value::String(hash))
            .await?;
        Ok(())
    }

    pub(crate) async fn password_hash(&self) -> Option<String> {
        match self.0.store.get_setting(PASSWORD_SETTING).await {
            Ok(Some(Value::String(h))) => Some(h),
            Ok(_) => None,
            Err(e) => {
                tracing::error!("reading password setting failed: {e}");
                None
            }
        }
    }

    /// 終了を始める。開いている SSE 接続を閉じる (これが無いと graceful shutdown が終わらない)。
    pub fn shutdown(&self) {
        self.0.shutdown.cancel();
    }

    pub(crate) fn shutdown_token(&self) -> &CancellationToken {
        &self.0.shutdown
    }

    /// スケジューラへ伝える。キューが満杯なら空くまで (上限付きで) 待ち、黙って捨てない。
    pub(crate) async fn notify_scheduler(&self, cmd: Command) {
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

pub fn router(state: AppState) -> Router {
    let hsts = state.0.cfg.secure_cookies;
    Router::new()
        .route("/", get(handlers::index))
        .route("/login", get(handlers::login_page).post(handlers::login))
        .route("/logout", post(handlers::logout))
        .route("/watches", post(handlers::add_watch))
        .route("/watches/{id}", get(handlers::watch_page))
        .route("/watches/{id}/edit", get(edit::edit_page).post(edit::save))
        .route("/watches/{id}/delete", post(handlers::delete_watch))
        .route("/watches/{id}/pick", get(pick::page))
        .route(
            "/watches/{id}/pick/choose",
            get(pick::choose_page).post(pick::choose),
        )
        .route("/settings", get(settings::page))
        .route("/settings/notify", post(settings::save_notify))
        .route("/settings/password", post(settings::change_password))
        .route("/settings/backup", post(settings::backup))
        .route(
            "/settings/import",
            post(settings::import).layer(DefaultBodyLimit::max(settings::MAX_IMPORT_BYTES)),
        )
        .route("/tags", get(tags::list).post(tags::create))
        .route("/tags/{id}/edit", get(tags::edit_page).post(tags::save))
        .route("/tags/{id}/delete", post(tags::delete))
        .route("/events", get(events::events_page))
        .route("/events/stream", get(sse::stream))
        .route("/static/app.js", get(handlers::script))
        .route("/healthz", get(|| async { "ok" }))
        .route("/static/app.css", get(handlers::stylesheet))
        .layer(DefaultBodyLimit::max(MAX_FORM_BYTES))
        .layer(middleware::map_response(move |res: Response| async move {
            security_headers(res, hsts)
        }))
        .with_state(state)
}

fn security_headers(mut res: Response, hsts: bool) -> Response {
    let h = res.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, name: HeaderName, v: &'static str| {
        h.insert(name, HeaderValue::from_static(v));
    };
    set(
        h,
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; \
         form-action 'self'; base-uri 'none'",
    );
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    // no-referrer だと同一オリジンの POST でも Origin: null になり、自前のフォームを拒否してしまう
    set(h, header::REFERRER_POLICY, "same-origin");
    if hsts {
        set(
            h,
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        );
    }
    if !h.contains_key(header::CACHE_CONTROL) {
        set(h, header::CACHE_CONTROL, "no-store");
    }
    res
}
