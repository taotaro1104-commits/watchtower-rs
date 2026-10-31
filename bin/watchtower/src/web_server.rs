//! Web UI の起動。

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use watchtower_api::ApiState;
use watchtower_metrics::Metrics;
use watchtower_scheduler::Command;
use watchtower_store::Store;
use watchtower_web::host::{guard, HostPolicy};
use watchtower_web::{router, AppState, WebConfig};

use watchtower_fetch::SsrfPolicy;

use crate::config::AppConfig;
use crate::picker::CdpPicker;

fn web_config(cfg: &AppConfig) -> WebConfig {
    WebConfig {
        // base_url が https なら Secure Cookie を既定にする
        secure_cookies: cfg.secure_cookies
            || cfg.base_url.to_ascii_lowercase().starts_with("https://"),
        ..Default::default()
    }
}

/// パスワードを保存する (`set-password` コマンド用)。
pub async fn set_password(cfg: &AppConfig, store: Store, password: &str) -> Result<()> {
    AppState::new(store, web_config(cfg))
        .set_password(password)
        .await?;
    Ok(())
}

/// API キーを発行し直す (`api-key` コマンド用)。平文を返すのはこの 1 回だけ。
pub async fn rotate_api_key(store: Store) -> Result<String> {
    Ok(ApiState::new(store, None).rotate_key().await?)
}

/// RSS フィードのトークンを発行し直す (`rss-token` コマンド用)。
pub async fn rotate_rss_token(store: Store) -> Result<String> {
    Ok(ApiState::new(store, None).rotate_rss_token().await?)
}

/// 待ち受けを開始する。ポートを確保できなければここで失敗する。
pub async fn spawn(
    cfg: &AppConfig,
    store: Store,
    scheduler: mpsc::Sender<Command>,
    stop: CancellationToken,
    metrics: Arc<Metrics>,
) -> Result<JoinHandle<()>> {
    // 設定の誤りは待ち受けの前に分かるようにする
    let hosts = HostPolicy::new(&cfg.base_url, &cfg.allowed_hosts)
        .with_context(|| "invalid base_url or allowed_hosts")?;
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("binding {}", cfg.listen))?;
    tracing::info!(addr = %cfg.listen, "web UI listening");
    let (store_for_api, scheduler_for_api) = (store.clone(), scheduler.clone());
    let mut state = AppState::with_scheduler(store, web_config(cfg), Some(scheduler));
    // ブラウザの設定があるときだけ、ビジュアルセレクタを使えるようにする (main で検証済み)
    if let Ok(Some(ws)) = cfg.browser_url() {
        let policy = SsrfPolicy::with_allowed_ips(cfg.allowed_ips.clone());
        state = state.with_picker(Arc::new(CdpPicker::new(ws, policy)));
    }
    let api = watchtower_api::router(
        ApiState::with_metrics(store_for_api, Some(scheduler_for_api), metrics)
            .with_base_url(&cfg.base_url),
    );
    let app = router(state.clone())
        .merge(api)
        .layer(axum::middleware::from_fn_with_state(hosts, guard));
    Ok(tokio::spawn(async move {
        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            stop.cancelled().await;
            // 開いたままの SSE 接続を閉じないと、graceful shutdown が終わらない
            state.shutdown();
        })
        .await;
        if let Err(e) = served {
            tracing::error!("web server stopped: {e}");
        }
    }))
}
