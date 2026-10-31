use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

const DEFAULT_CONFIG_FILE: &str = "watchtower.toml";
const ENV_PREFIX: &str = "WATCHTOWER_";

/// 起動設定。優先順位は 環境変数 > TOML ファイル > 既定値。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub default_interval_secs: u64,
    pub global_concurrency: usize,
    pub per_host_concurrency: usize,
    pub snapshots_keep: usize,
    /// SSRF 対策の例外として取得を許可するアドレス。
    pub allowed_ips: Vec<IpAddr>,
    /// 管理画面と API が受け付ける Host 名 (DNS rebinding 対策)。`localhost`・IP アドレス・`base_url` のホストは常に許す。
    pub allowed_hosts: Vec<String>,
    pub log_json: bool,
    /// 全ウォッチ共通の通知先 (Apprise 形式 URL)。
    pub notify_urls: Vec<String>,
    /// 通知本文のリンクの基点。
    pub base_url: String,
    /// Web UI の待ち受けアドレス。
    pub listen: SocketAddr,
    /// HTTPS の背後で使うとき true (セッション Cookie に Secure を付ける)。
    pub secure_cookies: bool,
    /// 未実装のサービスの通知を転送する Apprise API サーバ (例: `http://apprise:8000`)。
    pub apprise_api_url: Option<String>,
    /// JavaScript の描画に使う Chrome の DevTools の WebSocket (例: `ws://chrome:9222/devtools/browser/<id>`)。
    /// 値にトークンを含み得るので、ログやエラーには出さない。
    pub browser_ws_url: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            default_interval_secs: 300,
            global_concurrency: 10,
            per_host_concurrency: 2,
            snapshots_keep: 100,
            allowed_ips: Vec::new(),
            allowed_hosts: Vec::new(),
            log_json: true,
            notify_urls: Vec::new(),
            base_url: "http://localhost:5000".into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 5000)),
            secure_cookies: false,
            apprise_api_url: None,
            browser_ws_url: None,
        }
    }
}

impl AppConfig {
    pub fn load(file: Option<&Path>) -> Result<Self, Box<figment::Error>> {
        Figment::from(Serialized::defaults(Self::default()))
            .merge(Toml::file(file.unwrap_or(Path::new(DEFAULT_CONFIG_FILE))))
            .merge(Env::prefixed(ENV_PREFIX).ignore(&["config"]))
            .extract()
            .map_err(Box::new)
    }

    /// ブラウザの接続先。ws(s):// で、ユーザー情報を含まないものだけ受け付ける (値はエラーに載せない)。
    pub fn browser_url(&self) -> anyhow::Result<Option<String>> {
        let Some(raw) = self
            .browser_ws_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        let url = url::Url::parse(raw.trim())
            .map_err(|_| anyhow::anyhow!("browser_ws_url is not a valid URL"))?;
        if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none() {
            anyhow::bail!("browser_ws_url must be a ws:// or wss:// URL");
        }
        if !url.username().is_empty() || url.password().is_some() {
            anyhow::bail!("browser_ws_url must not contain credentials");
        }
        Ok(Some(url.to_string()))
    }

    /// 転送先の Apprise API の URL。http(s) で、ユーザー情報を含まないものだけ受け付ける。
    pub fn apprise_url(&self) -> anyhow::Result<Option<url::Url>> {
        let Some(raw) = self
            .apprise_api_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        let url = url::Url::parse(raw.trim())
            .map_err(|_| anyhow::anyhow!("apprise_api_url is not a valid URL"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            anyhow::bail!("apprise_api_url must be an http(s) URL");
        }
        if !url.username().is_empty() || url.password().is_some() {
            anyhow::bail!("apprise_api_url must not contain credentials");
        }
        Ok(Some(url))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("watchtower.db")
    }

    pub fn snapshot_dir(&self) -> PathBuf {
        self.data_dir.join("snapshots")
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment::Jail のクロージャ型が固定されているため
mod tests {
    use super::*;
    use figment::Jail;

    #[test]
    fn defaults_apply_without_file_or_env() {
        Jail::expect_with(|_| {
            assert_eq!(AppConfig::load(None).unwrap(), AppConfig::default());
            Ok(())
        });
    }

    #[test]
    fn toml_overrides_defaults_and_env_overrides_toml() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "watchtower.toml",
                "default_interval_secs = 60\nglobal_concurrency = 3\nlog_json = false\n",
            )?;
            let c = AppConfig::load(None).unwrap();
            assert_eq!(
                (c.default_interval_secs, c.global_concurrency, c.log_json),
                (60, 3, false)
            );

            jail.set_env("WATCHTOWER_GLOBAL_CONCURRENCY", "7");
            let c = AppConfig::load(None).unwrap();
            assert_eq!(c.global_concurrency, 7);
            assert_eq!(c.default_interval_secs, 60);
            Ok(())
        });
    }

    #[test]
    fn explicit_config_path_is_used() {
        Jail::expect_with(|jail| {
            jail.create_file("custom.toml", "snapshots_keep = 5")?;
            let c = AppConfig::load(Some(Path::new("custom.toml"))).unwrap();
            assert_eq!(c.snapshots_keep, 5);
            Ok(())
        });
    }

    #[test]
    fn allowed_ips_parse_from_env_list() {
        Jail::expect_with(|jail| {
            jail.set_env("WATCHTOWER_ALLOWED_IPS", r#"["127.0.0.1", "::1"]"#);
            let c = AppConfig::load(None).unwrap();
            assert_eq!(c.allowed_ips.len(), 2);
            jail.set_env("WATCHTOWER_ALLOWED_IPS", "[]");
            assert!(AppConfig::load(None).unwrap().allowed_ips.is_empty());
            Ok(())
        });
    }

    #[test]
    fn invalid_value_is_an_error() {
        Jail::expect_with(|jail| {
            jail.set_env("WATCHTOWER_GLOBAL_CONCURRENCY", "many");
            assert!(AppConfig::load(None).is_err());
            Ok(())
        });
    }

    #[test]
    fn paths_derive_from_data_dir() {
        let c = AppConfig {
            data_dir: "x".into(),
            ..Default::default()
        };
        assert_eq!(c.db_path(), PathBuf::from("x").join("watchtower.db"));
        assert_eq!(c.snapshot_dir(), PathBuf::from("x").join("snapshots"));
    }
}
