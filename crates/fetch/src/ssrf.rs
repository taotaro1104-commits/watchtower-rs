use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use thiserror::Error;
use url::{Host, Url};

use crate::FetchError;

#[derive(Debug, Error)]
#[error("{0}")]
pub(crate) struct BlockedAddr(pub String);

/// プライベート / ループバック / リンクローカル宛てのフェッチを既定で拒否する。
#[derive(Debug, Clone, Default)]
pub struct SsrfPolicy {
    allowed_ips: Vec<IpAddr>,
}

impl SsrfPolicy {
    pub fn with_allowed_ips(ips: Vec<IpAddr>) -> Self {
        Self {
            allowed_ips: ips.into_iter().map(normalize).collect(),
        }
    }

    pub fn allow_loopback() -> Self {
        Self::with_allowed_ips(vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ])
    }

    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        self.allowed_ips.contains(&normalize(ip)) || !is_blocked_ip(ip)
    }

    /// URL のホストが IP リテラルならここで判定する。ドメイン名は DNS 解決後に判定する。
    pub fn check_url(&self, url: &Url) -> Result<(), FetchError> {
        let ip = match url.host() {
            Some(Host::Ipv4(a)) => IpAddr::V4(a),
            Some(Host::Ipv6(a)) => IpAddr::V6(a),
            Some(Host::Domain(_)) => return Ok(()),
            None => return Err(FetchError::InvalidRequest(format!("no host in {url}"))),
        };
        if self.is_allowed(ip) {
            Ok(())
        } else {
            Err(FetchError::Blocked(ip.to_string()))
        }
    }
}

/// IPv4-mapped (`::ffff:a.b.c.d`) を IPv4 に直す。
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// IPv6 に埋め込まれた IPv4 (NAT64 / 6to4 / IPv4 互換) を取り出す。
fn embedded_v4(a: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = a.segments();
    let o = a.octets();
    let nat64 = s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0; 4]; // 64:ff9b::/96
    let sixto4 = s[0] == 0x2002; // 2002::/16
    let compat = s[..6] == [0; 6] && s[6..] != [0, 0] && s[6..] != [0, 1]; // ::a.b.c.d (:: と ::1 を除く)
    if nat64 || compat {
        Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
    } else if sixto4 {
        Some(Ipv4Addr::new(o[2], o[3], o[4], o[5]))
    } else {
        None
    }
}

pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match normalize(ip) {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || o[0] == 0 // 0.0.0.0/8
                || o[0] >= 240 // 240.0.0.0/4 予約
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15
        }
        IpAddr::V6(a) => {
            if let Some(v4) = embedded_v4(a) {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let seg0 = a.segments()[0];
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 ユニークローカル
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 リンクローカル
                || (seg0 & 0xffc0) == 0xfec0 // fec0::/10 サイトローカル (廃止)
        }
    }
}

/// DNS 解決後のアドレスで判定するリゾルバ。許可されないアドレスは結果から除く。
pub(crate) struct SafeResolver(pub Arc<SsrfPolicy>);

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = Arc::clone(&self.0);
        Box::pin(async move {
            let host = name.as_str().to_string();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let allowed: Vec<SocketAddr> = resolved.filter(|a| policy.is_allowed(a.ip())).collect();
            if allowed.is_empty() {
                return Err(Box::new(BlockedAddr(host)) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_private_loopback_and_link_local() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn blocks_embedded_and_reserved_ranges() {
        for s in [
            "64:ff9b::a9fe:a9fe", // NAT64 -> 169.254.169.254
            "64:ff9b::7f00:1",    // NAT64 -> 127.0.0.1
            "2002:7f00:1::",      // 6to4 -> 127.0.0.1
            "2002:a00:1::",       // 6to4 -> 10.0.0.1
            "::127.0.0.1",        // IPv4 互換
            "::10.0.0.1",
            "fec0::1", // site-local
            "0.1.2.3",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "255.255.255.254",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn embedded_public_addresses_stay_allowed() {
        for s in [
            "64:ff9b::808:808",
            "2002:808:808::",
            "198.20.0.1",
            "223.255.255.255",
        ] {
            assert!(!is_blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn allowlist_entries_are_normalized() {
        let p = SsrfPolicy::with_allowed_ips(vec![ip("::ffff:10.0.0.5")]);
        assert!(p.is_allowed(ip("10.0.0.5")));
        assert!(p.is_allowed(ip("::ffff:10.0.0.5")));
    }

    #[test]
    fn allows_public_addresses() {
        for s in ["93.184.216.34", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(!is_blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn allowlist_overrides_block() {
        let p = SsrfPolicy::with_allowed_ips(vec![ip("10.0.0.5")]);
        assert!(p.is_allowed(ip("10.0.0.5")));
        assert!(!p.is_allowed(ip("10.0.0.6")));
    }

    #[test]
    fn check_url_handles_literals_and_domains() {
        let p = SsrfPolicy::default();
        assert!(p
            .check_url(&"http://127.0.0.1:80/".parse().unwrap())
            .is_err());
        assert!(p.check_url(&"http://[::1]/".parse().unwrap()).is_err());
        assert!(p.check_url(&"http://example.com/".parse().unwrap()).is_ok());
    }
}
