mod app;
mod cli;
mod config;
mod healthcheck;
mod notifications;
mod picker;
mod web_server;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::cli::{Cli, Command, WatchCommand};
use crate::config::AppConfig;

fn init_logging(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if json {
        builder.json().init();
    } else {
        builder.compact().init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = AppConfig::load(cli.config.as_deref()).context("loading configuration")?;
    init_logging(cfg.log_json);
    // 転送先があるときだけ、未知のスキームを保存時に受け付ける (CLI の検証にも効く)
    watchtower_notify::allow_foreign_targets(cfg.apprise_url()?.is_some());
    cfg.browser_url()?; // 不正な設定は起動時に弾く
                        // ヘルスチェックは DB を開かずに、待ち受けアドレスだけを使う
    if matches!(cli.command, Command::Healthcheck) {
        return healthcheck::check(cfg.listen).await;
    }
    // 復元は DB を開く前に行う (開くと空の DB が作られてしまう)
    if let Command::Restore { file, force } = &cli.command {
        return app::restore(&cfg, file, *force).await;
    }
    let app = App::open(cfg).await?;

    match cli.command {
        Command::Run => app.run().await,
        Command::Watch(WatchCommand::Add(a)) => app.add(a).await,
        Command::Watch(WatchCommand::List) => app.list().await,
        Command::Watch(WatchCommand::Remove { id }) => app.remove(id).await,
        Command::Check(a) => app.check(a).await,
        Command::Import { file } => app.import(&file).await,
        Command::SetPassword => app.set_password().await,
        Command::ApiKey => app.api_key().await,
        Command::RssToken => app.rss_token().await,
        Command::Backup { file } => app.backup(&file).await,
        Command::Healthcheck => unreachable!("handled before the app is opened"),
        Command::Restore { .. } => unreachable!("handled before the app is opened"),
    }
}
