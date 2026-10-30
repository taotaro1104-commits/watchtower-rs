//! Web UI の起動。

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use watchtower_scheduler::Command;
use watchtower_store::Store;
use watchtower_web::{router, AppState, WebConfig};

use crate::config::AppConfig;

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

/// 待ち受けを開始する。ポートを確保できなければここで失敗する。
pub async fn spawn(
    cfg: &AppConfig,
    store: Store,
    scheduler: mpsc::Sender<Command>,
    stop: CancellationToken,
) -> Result<JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("binding {}", cfg.listen))?;
    tracing::info!(addr = %cfg.listen, "web UI listening");
    let state = AppState::with_scheduler(store, web_config(cfg), Some(scheduler));
    let app = router(state.clone());
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
