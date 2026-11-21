//! Client IP for the failed-login log line, like `python-ipware` 3.0 with
//! `IpWare(proxy_list=settings.TRUSTED_PROXIES)` (gap S7).
//!
//! upstream: src/paperless/signals.py@v3.4.2 (`handle_failed_login`), src/paperless/settings/
//! __init__.py@v3.4.2 (`TRUSTED_PROXIES = get_list_from_env("PAPERLESS_TRUSTED_PROXIES")`),
//! python_ipware/python_ipware.py@3.0 (`IpWareMeta` precedence, `get_ips_from_string`,
//! `is_proxy_trusted_list_valid`, `get_best_ip`, `get_client_ip`), CPython 3.12.10
//! `ipaddress` (`is_global`, `is_loopback`, `ipv4_mapped`).
//!
//! Only `request.META` keys that Django can actually hold are read: the `HTTP_*` headers of the
//! precedence list and `REMOTE_ADDR` (the TCP peer). ipware's hyphenated/unprefixed names
//! (`X-REAL-IP`, `X_FORWARDED_FOR`, …) never appear in Django's META and are therefore skipped.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::http::HeaderMap;

/// `IpWareMeta` default precedence, reduced to the keys Django's META can contain.
const PRECEDENCE: &[&str] = &[
    "HTTP_X_FORWARDED_FOR",
    "HTTP_CLIENT_IP",
    "HTTP_X_REAL_IP",
    "HTTP_X_FORWARDED",
    "HTTP_X_CLUSTER_CLIENT_IP",
    "HTTP_FORWARDED_FOR",
    "HTTP_FORWARDED",
    "HTTP_CF_CONNECTING_IP",
    "REMOTE_ADDR",
];

/// `IpWareIpAddress.parse_ip_address`: IPv6 first (`ipv4_mapped or ip`), then IPv4
/// (port after the first `:` removed).
pub fn parse_ip_address(s: &str) -> Option<IpAddr> {
    let v6 = match s.split_once("]:") {
        Some((a, _)) => a.replace('[', "").trim().to_string(),
        None => s.trim().to_string(),
    };
    if let Ok(ip) = v6.parse::<Ipv6Addr>() {
        return Some(match ip.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(ip),
        });
    }
    let v4 = s.split(':').next().unwrap_or("").trim();
    v4.parse::<Ipv4Addr>().ok().map(IpAddr::V4)
}

/// `get_ips_from_string(ip_str, strict=False)` (leftmost order, invalid entries skipped).
fn ips_from_string(s: &str) -> Vec<IpAddr> {
    s.split(',').filter_map(|p| parse_ip_address(p.trim())).collect()
}

fn in_v4(ip: Ipv4Addr, net: [u8; 4], prefix: u32) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

fn in_v6(ip: Ipv6Addr, net: u128, prefix: u32) -> bool {
    let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
    (u128::from(ip) & mask) == (net & mask)
}

/// CPython 3.12.10 `IPv4Address.is_private`.
fn v4_is_private(ip: Ipv4Addr) -> bool {
    const NETS: &[([u8; 4], u32)] = &[
        ([0, 0, 0, 0], 8),
        ([10, 0, 0, 0], 8),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24),
        ([192, 0, 0, 170], 31),
        ([192, 0, 2, 0], 24),
        ([192, 168, 0, 0], 16),
        ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([255, 255, 255, 255], 32),
    ];
    const EXCEPTIONS: &[([u8; 4], u32)] = &[([192, 0, 0, 9], 32), ([192, 0, 0, 10], 32)];
    NETS.iter().any(|(n, p)| in_v4(ip, *n, *p)) && !EXCEPTIONS.iter().any(|(n, p)| in_v4(ip, *n, *p))
}

/// CPython 3.12.10 `IPv6Address.is_private` (not IPv4-mapped).
fn v6_is_private(ip: Ipv6Addr) -> bool {
    const NETS: &[(u128, u32)] = &[
        (0x0000_0000_0000_0000_0000_0000_0000_0001, 128),
        (0, 128),
        (0x0000_0000_0000_0000_0000_ffff_0000_0000, 96),
        (0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48),
        (0x0100_0000_0000_0000_0000_0000_0000_0000, 64),
        (0x2001_0000_0000_0000_0000_0000_0000_0000, 23),
        (0x2001_0db8_0000_0000_0000_0000_0000_0000, 32),
        (0x2002_0000_0000_0000_0000_0000_0000_0000, 16),
        (0x3fff_0000_0000_0000_0000_0000_0000_0000, 20),
        (0xfc00_0000_0000_0000_0000_0000_0000_0000, 7),
        (0xfe80_0000_0000_0000_0000_0000_0000_0000, 10),
    ];
    const EXCEPTIONS: &[(u128, u32)] = &[
        (0x2001_0001_0000_0000_0000_0000_0000_0001, 128),
        (0x2001_0001_0000_0000_0000_0000_0000_0002, 128),
        (0x2001_0003_0000_0000_0000_0000_0000_0000, 32),
        (0x2001_0004_0112_0000_0000_0000_0000_0000, 48),
        (0x2001_0020_0000_0000_0000_0000_0000_0000, 28),
        (0x2001_0030_0000_0000_0000_0000_0000_0000, 28),
    ];
    NETS.iter().any(|(n, p)| in_v6(ip, *n, *p)) && !EXCEPTIONS.iter().any(|(n, p)| in_v6(ip, *n, *p))
}

/// CPython `is_global` (IPv4: not in 100.64.0.0/10 and not private).
pub fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !in_v4(v4, [100, 64, 0, 0], 10) && !v4_is_private(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_global(IpAddr::V4(v4)),
            None => !v6_is_private(v6),
        },
    }
}

/// CPython `is_loopback`.
pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.octets()[0] == 127,
            None => u128::from(v6) == 1,
        },
    }
}

/// `IpWareProxy.is_proxy_trusted_list_valid(ip_list, strict=False)`: the last
/// `len(proxy_list)` addresses must start with the trusted prefixes (string prefix match).
fn trusted_list_valid(ips: &[IpAddr], proxies: &[String]) -> bool {
    if proxies.is_empty() {
        return true;
    }
    if ips.len().saturating_sub(1) < proxies.len() || ips.is_empty() {
        return false;
    }
    let tail = &ips[ips.len() - proxies.len()..];
    tail.iter().zip(proxies).all(|(ip, p)| ip.to_string().starts_with(p.as_str()))
}

/// `IpWare(proxy_list=trusted_proxies).get_client_ip(request.META)[0]`.
/// `headers` are the request headers, `remote_addr` the TCP peer (`REMOTE_ADDR`).
pub fn client_ip(headers: &HeaderMap, remote_addr: Option<IpAddr>, trusted_proxies: &[String]) -> Option<IpAddr> {
    let mut loopback: Vec<IpAddr> = Vec::new();
    let mut private: Vec<IpAddr> = Vec::new();
    for key in PRECEDENCE {
        let value = if *key == "REMOTE_ADDR" { remote_addr.map(|ip| ip.to_string()) } else { crate::csrf::meta_header(headers, key) };
        let Some(value) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
            continue;
        };
        let ips = ips_from_string(&value);
        if ips.is_empty() || !trusted_list_valid(&ips, trusted_proxies) {
            continue;
        }
        // get_best_ip: with a proxy list the address in front of the trusted proxies
        let best = if trusted_proxies.is_empty() {
            ips.first().copied()
        } else {
            ips.len().checked_sub(trusted_proxies.len() + 1).and_then(|i| ips.get(i)).copied()
        };
        if let Some(ip) = best {
            if is_global(ip) {
                return Some(ip);
            }
            if is_loopback(ip) {
                loopback.push(ip);
            } else {
                private.push(ip);
            }
        }
    }
    private.first().or(loopback.first()).copied()
}

/// Does `ip` match an entry from `PAPERLESS_TRUSTED_PROXIES`? Entries: `a.b.c.d/nn` or an
/// IPv6 network (CIDR), a single address (exact) or – like ipware – a text prefix (`10.0.0.`).
pub fn is_trusted_proxy(ip: IpAddr, proxies: &[String]) -> bool {
    let ip = canonical(ip);
    proxies.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).any(|p| {
        if let Some((addr, bits)) = p.split_once('/') {
            let (Ok(net), Ok(bits)) = (addr.trim().parse::<IpAddr>(), bits.trim().parse::<u32>()) else {
                return false;
            };
            return match (ip, canonical(net)) {
                (IpAddr::V4(a), IpAddr::V4(n)) => bits <= 32 && in_v4(a, n.octets(), bits),
                (IpAddr::V6(a), IpAddr::V6(n)) => bits <= 128 && in_v6(a, u128::from(n), bits),
                _ => false,
            };
        }
        match p.parse::<IpAddr>() {
            Ok(single) => canonical(single) == ip,
            Err(_) => ip.to_string().starts_with(p),
        }
    })
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Client IP for the Basic throttle (security review 2026-11-19 F7). Unlike
/// [`client_ip`] (ipware parity for the log), headers are only evaluated if the
/// TCP peer is a trusted proxy (`PAPERLESS_TRUSTED_PROXIES`): then the
/// rightmost address from `X-Forwarded-For` that is not itself a trusted proxy. Without a
/// proxy list the peer always counts – otherwise anyone could bypass the IP limit with a made-up
/// `X-Forwarded-For` or lock out foreign IPs.
pub fn throttle_client_ip(headers: &HeaderMap, remote_addr: Option<IpAddr>, trusted_proxies: &[String]) -> Option<IpAddr> {
    let peer = canonical(remote_addr?);
    if trusted_proxies.is_empty() || !is_trusted_proxy(peer, trusted_proxies) {
        return Some(peer);
    }
    let chain: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|p| parse_ip_address(p.trim()))
        .map(canonical)
        .collect();
    // from the right: first address that is not a trusted proxy; otherwise the leftmost
    chain.iter().rev().find(|ip| !is_trusted_proxy(**ip, trusted_proxies)).or(chain.first()).copied().or(Some(peer))
}

/// The log line of `handle_failed_login` (logger `paperless.auth`, level INFO).
pub fn failed_login_message(username: Option<&str>, ip: Option<IpAddr>) -> String {
    let mut msg = match username {
        None => "No authentication provided".to_string(),
        Some(u) => format!("Login failed for user `{u}`"),
    };
    match ip {
        None => msg.push_str(". Unable to determine IP address."),
        Some(ip) if is_global(ip) => msg.push_str(&format!(" from IP `{ip}`.")),
        Some(ip) => msg.push_str(&format!(" from private IP `{ip}`.")),
    }
    msg
}

/// `handle_failed_login(sender, credentials, request)`: log with the client IP.
pub fn log_failed_login(username: Option<&str>, headers: &HeaderMap, remote_addr: Option<IpAddr>, trusted_proxies: &[String]) {
    let ip = client_ip(headers, remote_addr, trusted_proxies);
    tracing::info!(target: "paperless.auth", "{}", failed_login_message(username, ip));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parse_like_ipware() {
        assert_eq!(parse_ip_address("1.2.3.4"), Some(ip("1.2.3.4")));
        assert_eq!(parse_ip_address("1.2.3.4:8080"), Some(ip("1.2.3.4")));
        assert_eq!(parse_ip_address("[2001:db8::1]:443"), Some(ip("2001:db8::1")));
        assert_eq!(parse_ip_address("::ffff:8.8.8.8"), Some(ip("8.8.8.8")));
        assert_eq!(parse_ip_address("unknown"), None);
        assert_eq!(parse_ip_address("01.2.3.4"), None);
    }

    /// Values checked against CPython 3.12.10 `ipaddress`.
    #[test]
    fn global_and_private_like_cpython() {
        for g in ["8.8.8.8", "1.1.1.1", "192.0.0.9", "2001:4860:4860::8888", "2001:20::1"] {
            assert!(is_global(ip(g)), "{g} is global");
        }
        for p in [
            "10.1.2.3",
            "127.0.0.1",
            "172.31.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "2001:db8::1",
            "0.0.0.0",
        ] {
            assert!(!is_global(ip(p)), "{p} is not global");
        }
        assert!(is_loopback(ip("127.0.0.2")));
        assert!(is_loopback(ip("::1")));
        assert!(!is_loopback(ip("10.0.0.1")));
    }

    #[test]
    fn client_ip_without_proxy_list() {
        // leftmost of X-Forwarded-For, global wins at once
        assert_eq!(client_ip(&h(&[("x-forwarded-for", "8.8.8.8, 10.0.0.1")]), Some(ip("127.0.0.1")), &[]), Some(ip("8.8.8.8")));
        // private header address preferred over loopback REMOTE_ADDR
        assert_eq!(client_ip(&h(&[("x-real-ip", "192.168.0.7")]), Some(ip("127.0.0.1")), &[]), Some(ip("192.168.0.7")));
        // nothing but the peer
        assert_eq!(client_ip(&HeaderMap::new(), Some(ip("127.0.0.1")), &[]), Some(ip("127.0.0.1")));
        assert_eq!(client_ip(&HeaderMap::new(), Some(ip("1.2.3.4")), &[]), Some(ip("1.2.3.4")));
        assert_eq!(client_ip(&HeaderMap::new(), None, &[]), None);
        assert_eq!(client_ip(&h(&[("x-forwarded-for", "garbage")]), None, &[]), None);
    }

    #[test]
    fn client_ip_with_trusted_proxies() {
        let proxies = vec!["10.0.0.".to_string()];
        // client in front of the trusted proxy
        assert_eq!(
            client_ip(&h(&[("x-forwarded-for", "9.9.9.9, 8.8.8.8, 10.0.0.5")]), Some(ip("10.0.0.5")), &proxies),
            Some(ip("8.8.8.8"))
        );
        // last hop is not trusted: header skipped; REMOTE_ADDR alone has too few entries
        assert_eq!(client_ip(&h(&[("x-forwarded-for", "8.8.8.8, 172.16.0.1")]), Some(ip("10.0.0.5")), &proxies), None);
    }

    /// Security review 2026-11-19 F7: without a proxy list only the TCP peer counts for the throttle;
    /// `X-Forwarded-For` is only taken from trusted proxies.
    #[test]
    fn throttle_ip_ignores_spoofed_forwarded_for() {
        let xff = h(&[("x-forwarded-for", "8.8.8.8")]);
        // no proxy list: header has no effect
        assert_eq!(throttle_client_ip(&xff, Some(ip("203.0.113.9")), &[]), Some(ip("203.0.113.9")));
        assert_eq!(throttle_client_ip(&xff, Some(ip("127.0.0.1")), &[]), Some(ip("127.0.0.1")));
        assert_eq!(throttle_client_ip(&xff, None, &[]), None);
        // proxy list, but peer is not a proxy: header has no effect
        let proxies = vec!["10.0.0.0/8".to_string()];
        assert_eq!(throttle_client_ip(&xff, Some(ip("203.0.113.9")), &proxies), Some(ip("203.0.113.9")));
        // trusted proxy: rightmost untrusted address (left entries can be forged)
        let chain = h(&[("x-forwarded-for", "1.1.1.1, 8.8.8.8, 10.1.2.3")]);
        assert_eq!(throttle_client_ip(&chain, Some(ip("10.0.0.5")), &proxies), Some(ip("8.8.8.8")));
        assert_eq!(throttle_client_ip(&h(&[("x-forwarded-for", "8.8.4.4")]), Some(ip("::ffff:10.0.0.5")), &proxies), Some(ip("8.8.4.4")));
        // trusted proxy without header: the peer
        assert_eq!(throttle_client_ip(&HeaderMap::new(), Some(ip("10.0.0.5")), &proxies), Some(ip("10.0.0.5")));
    }

    #[test]
    fn trusted_proxy_matching() {
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_trusted_proxy(ip("172.18.0.2"), &p(&["172.18.0.2"])));
        assert!(!is_trusted_proxy(ip("172.18.0.20"), &p(&["172.18.0.2"])));
        assert!(is_trusted_proxy(ip("172.18.5.1"), &p(&["172.16.0.0/12"])));
        assert!(is_trusted_proxy(ip("10.0.0.7"), &p(&["10.0.0."])));
        assert!(is_trusted_proxy(ip("fd00::1"), &p(&["fd00::/8"])));
        assert!(!is_trusted_proxy(ip("8.8.8.8"), &p(&["10.0.0.0/8", "bad/xx", ""])));
    }

    #[test]
    fn message_like_handle_failed_login() {
        assert_eq!(failed_login_message(Some("bob"), None), "Login failed for user `bob`. Unable to determine IP address.");
        assert_eq!(failed_login_message(Some("bob"), Some(ip("8.8.8.8"))), "Login failed for user `bob` from IP `8.8.8.8`.");
        assert_eq!(failed_login_message(None, Some(ip("10.0.0.1"))), "No authentication provided from private IP `10.0.0.1`.");
    }
}
