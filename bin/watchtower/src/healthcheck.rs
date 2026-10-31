//! `healthcheck`: 自分自身の `/healthz` に問い合わせる。コンテナの HEALTHCHECK 用。
//!
//! distroless のイメージには curl も shell も無いので、バイナリ自身が確かめる。
//! 依存を増やさず、生の HTTP/1.1 で 1 回だけ問い合わせる。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(3);
/// 応答として読む最大バイト数 (想定は数十バイト)。
const MAX_RESPONSE_BYTES: u64 = 8 * 1024;

/// 待ち受けが `0.0.0.0` / `::` でも、自分自身にはループバックで問い合わせる。
fn loopback_of(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(a) if a.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(a) if a.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    };
    SocketAddr::new(ip, listen.port())
}

pub async fn check(listen: SocketAddr) -> Result<()> {
    let addr = loopback_of(listen);
    tokio::time::timeout(TIMEOUT, probe(addr))
        .await
        .map_err(|_| anyhow::anyhow!("no answer from {addr} within {TIMEOUT:?}"))?
}

async fn probe(addr: SocketAddr) -> Result<()> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .with_context(|| format!("cannot connect to {addr}"))?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut raw = Vec::new();
    stream
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut raw)
        .await?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.lines().next().unwrap_or_default();
    if !status
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200")
    {
        bail!("unhealthy: {status}");
    }
    if body.trim() != "ok" {
        bail!("unhealthy: unexpected body");
    }
    Ok(())
}
