mod app;
mod cli;
mod config;
mod notifications;
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
    let app = App::open(cfg).await?;

    match cli.command {
        Command::Run => app.run().await,
        Command::Watch(WatchCommand::Add(a)) => app.add(a).await,
        Command::Watch(WatchCommand::List) => app.list().await,
        Command::Watch(WatchCommand::Remove { id }) => app.remove(id).await,
        Command::Check(a) => app.check(a).await,
        Command::Import { file } => app.import(&file).await,
        Command::SetPassword => app.set_password().await,
    }
}
