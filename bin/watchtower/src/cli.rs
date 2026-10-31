use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "watchtower",
    version,
    about = "changedetection.io 互換のウォッチサーバ"
)]
pub struct Cli {
    /// TOML 設定ファイル (既定: ./watchtower.toml があれば読む)
    #[arg(long, env = "WATCHTOWER_CONFIG", global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// スケジューラを起動して定期チェックを続ける
    Run,
    /// ウォッチの管理
    #[command(subcommand)]
    Watch(WatchCommand),
    /// 今すぐチェックして結果を表示する
    Check(CheckArgs),
    /// 本家 changedetection.io の url-watches.json を取り込む
    Import { file: PathBuf },
    /// Web UI のログインパスワードを設定する (標準入力から 1 行読む)
    SetPassword,
    /// REST API のキーを新しく発行する (以前のキーは無効になる)
    ApiKey,
    /// RSS フィードのトークンを新しく発行する (以前のものは無効になる)
    RssToken,
    /// 自分自身の /healthz に問い合わせる (コンテナの HEALTHCHECK 用。正常なら終了コード 0)
    Healthcheck,
    /// DB とスナップショットを 1 つの zip に書き出す (稼働中でも可。既存のファイルは上書きしない)
    Backup { file: PathBuf },
    /// `backup` で作った zip から復元する (既存のデータがあるときは --force が必要)
    Restore {
        file: PathBuf,
        /// 既存のデータを消して置き換える
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WatchCommand {
    Add(AddArgs),
    List,
    Remove { id: Uuid },
}

#[derive(Debug, Args)]
pub struct AddArgs {
    pub url: String,
    #[arg(long)]
    pub title: Option<String>,
    /// チェック間隔 (秒)
    #[arg(long)]
    pub interval: Option<u64>,
    /// 対象を絞る CSS セレクタ (JSON は `json:$.path`)。複数指定可
    #[arg(long)]
    pub include: Vec<String>,
    /// 除外する CSS セレクタ。複数指定可
    #[arg(long)]
    pub exclude: Vec<String>,
    /// 含む行を無視するテキスト。複数指定可
    #[arg(long)]
    pub ignore: Vec<String>,
    /// 含まれていたら triggered と判定するテキスト。複数指定可
    #[arg(long)]
    pub trigger: Vec<String>,
    /// 通知先 (Apprise 形式 URL。例: discord://id/token)。複数指定可
    #[arg(long)]
    pub notify: Vec<String>,
    /// 取得にブラウザ (JavaScript の描画) を使う。`browser_ws_url` の設定が要る
    #[arg(long)]
    pub browser: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub id: Option<Uuid>,
    /// 一時停止していない全ウォッチをチェックする
    #[arg(long)]
    pub all: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("watchtower").chain(args.iter().copied()))
    }

    #[test]
    fn parses_add_with_repeated_options() {
        let cli = parse(&[
            "watch",
            "add",
            "https://e.com",
            "--include",
            ".a",
            "--include",
            ".b",
            "--interval",
            "60",
        ])
        .unwrap();
        let Command::Watch(WatchCommand::Add(a)) = cli.command else {
            panic!("not add")
        };
        assert_eq!(a.include, vec![".a", ".b"]);
        assert_eq!(a.interval, Some(60));
    }

    #[test]
    fn check_requires_id_or_all_but_not_both() {
        assert!(parse(&["check"]).is_err());
        assert!(parse(&["check", "--all"]).is_ok());
        let id = Uuid::new_v4().to_string();
        assert!(parse(&["check", &id]).is_ok());
        assert!(parse(&["check", &id, "--all"]).is_err());
        assert!(parse(&["check", "not-a-uuid"]).is_err());
    }

    #[test]
    fn config_flag_is_global() {
        assert!(parse(&["watch", "list", "--config", "a.toml"]).is_ok());
    }
}
