//! What counts as "the user's LAN" for `relay/lan`, `relay/ws` and TCP first-use approvals:
//! RFC 1918 (10/8, 172.16/12, 192.168/16), link-local (169.254/16, fe80::/10), loopback
//! (127/8, ::1), IPv6 unique-local (fc00::/7, the v6 RFC 1918), and names that resolve only to
//! those (`.local` mDNS names included). Public addresses are refused.
//!
//! DNS rebinding: a name is resolved and every address checked; the connection then goes to a
//! checked address — reqwest through [`LanResolver`], which re-checks at connect time, and the
//! WebSocket relay by connecting to the resolved address itself. A name (other than
//! `localhost`) that resolves to loopback is refused, so loopback is reachable only by a literal
//! address or `localhost`, where the port is checked against the companion's and the app's
//! own ports (a page must not reach the app's REST API through the relay).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, RwLock};

/// Test-only DNS answers (name → addresses), consulted before the system resolver.
pub type DnsOverride = Arc<RwLock<HashMap<String, Vec<IpAddr>>>>;

pub fn v4_lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local() || ip.is_loopback()
}

pub fn v6_lan(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_lan(v4);
    }
    let s = ip.segments()[0];
    ip.is_loopback() || (s & 0xffc0) == 0xfe80 || (s & 0xfe00) == 0xfc00
}

pub fn ip_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => v4_lan(a),
        IpAddr::V6(a) => v6_lan(a),
    }
}

/// The host part as an IP literal (`[fe80::1]` or `192.168.1.2`).
pub fn literal(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[').trim_end_matches(']').parse().ok()
}

/// A plausible host name or IP literal (letters, digits, `-`, `.`, `_`; v6 in brackets).
pub fn host_syntax_ok(host: &str) -> bool {
    if literal(host).is_some() {
        return true;
    }
    !host.is_empty() && host.len() <= 253 && !host.starts_with('.') && !host.starts_with('-')
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
}

/// Resolve `host` and require every address to be on the LAN. `Err` is the message for the
/// page. A literal is checked as is.
pub async fn resolve(host: &str, dns: &DnsOverride) -> Result<Vec<IpAddr>, String> {
    let host = host.to_ascii_lowercase();
    if let Some(ip) = literal(&host) {
        return if ip_lan(ip) { Ok(vec![ip]) } else { Err(format!("{ip} is a public address: relay/lan reaches only your local network (RFC 1918, link-local, loopback, .local)")) };
    }
    if !host_syntax_ok(&host) {
        return Err(format!("{host:?} is not a host name"));
    }
    let over = dns.read().unwrap().get(&host).cloned();
    let addrs: Vec<IpAddr> = match over {
        Some(a) => a,
        None => match tokio::time::timeout(std::time::Duration::from_secs(5), tokio::net::lookup_host((host.as_str(), 0))).await {
            Ok(Ok(it)) => it.map(|s| s.ip()).collect(),
            Ok(Err(e)) => return Err(format!("cannot resolve {host}: {e}")),
            Err(_) => return Err(format!("timed out resolving {host}")),
        },
    };
    if addrs.is_empty() {
        return Err(format!("{host} has no addresses"));
    }
    if let Some(bad) = addrs.iter().find(|a| !ip_lan(**a)) {
        return Err(format!("{host} resolves to the public address {bad}: relay/lan reaches only your local network"));
    }
    if host != "localhost" && addrs.iter().any(|a| a.is_loopback()) {
        return Err(format!("{host} resolves to loopback: use 127.0.0.1 or localhost explicitly"));
    }
    Ok(addrs)
}

/// Ports on this machine no relay may reach: the companion's and the app's (whatever this
/// process runs on, plus the defaults, since a scratch instance may sit beside the real app).
pub fn own_port_refused(addrs: &[IpAddr], port: u16, own: &[u16]) -> bool {
    addrs.iter().any(|a| a.is_loopback() || a.is_unspecified()) && (own.contains(&port) || (7317..=7320).contains(&port) || port == 8790)
}

/// reqwest's resolver for the LAN client: the same check as [`resolve`], at connect time.
pub struct LanResolver(pub DnsOverride);

impl reqwest::dns::Resolve for LanResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let dns = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            match resolve(&host, &dns).await {
                Ok(a) => {
                    let it: reqwest::dns::Addrs = Box::new(a.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect::<Vec<_>>().into_iter());
                    Ok(it)
                }
                Err(e) => Err(e.into()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classify() {
        for ok in ["10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.20", "169.254.1.1", "127.0.0.1", "::1", "fe80::1", "fd00::5", "::ffff:192.168.1.1"] {
            assert!(ip_lan(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "1.1.1.1", "2001:4860::8888", "::ffff:8.8.8.8", "0.0.0.0", "224.0.0.1"] {
            assert!(!ip_lan(bad.parse().unwrap()), "{bad}");
        }
    }

    #[tokio::test]
    async fn names() {
        let dns: DnsOverride = Default::default();
        dns.write().unwrap().insert("tv.local".into(), vec!["192.168.1.9".parse().unwrap()]);
        dns.write().unwrap().insert("evil.local".into(), vec!["8.8.8.8".parse().unwrap()]);
        dns.write().unwrap().insert("mixed.test".into(), vec!["192.168.1.9".parse().unwrap(), "8.8.8.8".parse().unwrap()]);
        dns.write().unwrap().insert("rebind.test".into(), vec!["127.0.0.1".parse().unwrap()]);
        assert!(resolve("tv.local", &dns).await.is_ok());
        assert!(resolve("TV.local", &dns).await.is_ok());
        assert!(resolve("evil.local", &dns).await.is_err());
        assert!(resolve("mixed.test", &dns).await.is_err());
        assert!(resolve("rebind.test", &dns).await.is_err());
        assert!(resolve("8.8.8.8", &dns).await.is_err());
        assert!(resolve("[fe80::1]", &dns).await.is_ok());
        assert!(resolve("a/b", &dns).await.is_err());
        let lo = vec!["127.0.0.1".parse().unwrap()];
        assert!(own_port_refused(&lo, 7317, &[]) && own_port_refused(&lo, 4444, &[4444]) && !own_port_refused(&lo, 5432, &[4444]));
        assert!(!own_port_refused(&["192.168.1.2".parse().unwrap()], 8790, &[]));
    }
}
