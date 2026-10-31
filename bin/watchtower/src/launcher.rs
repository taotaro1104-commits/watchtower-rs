//! `ui`: サーバを起動し、立ち上がったら既定のブラウザで管理画面を開く。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::healthcheck;

/// 立ち上がりを待つ間隔と回数 (合計で約 30 秒)。
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: u32 = 120;

/// 管理画面の URL。待ち受けが `0.0.0.0` / `::` でも、自分自身にはループバックで向かう。
pub fn ui_url(listen: SocketAddr) -> String {
    let ip = match listen.ip() {
        IpAddr::V4(a) if a.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(a) if a.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    };
    format!("http://{}/", SocketAddr::new(ip, listen.port()))
}

/// `/healthz` が応えるまで待つ。
pub async fn wait_until_up(listen: SocketAddr) -> Result<()> {
    for _ in 0..POLL_ATTEMPTS {
        if healthcheck::check(listen).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!("the server did not come up in time")
}

/// OS の既定ブラウザで開く。`url` は `ui_url` が作った IP とポートだけの文字列に限る。
pub fn open_browser(url: &str) -> Result<()> {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_listen_is_used_as_is() {
        assert_eq!(ui_url(addr("127.0.0.1:5000")), "http://127.0.0.1:5000/");
    }

    #[test]
    fn unspecified_listen_points_to_loopback() {
        assert_eq!(ui_url(addr("0.0.0.0:8080")), "http://127.0.0.1:8080/");
        assert_eq!(ui_url(addr("[::]:8080")), "http://[::1]:8080/");
    }

    #[test]
    fn ipv6_literal_is_bracketed() {
        assert_eq!(ui_url(addr("[::1]:5000")), "http://[::1]:5000/");
    }
}
