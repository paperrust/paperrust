//! Sender IP lists (`192.168.1.20`, `192.168.1.0/24`, `fd00::/8`), separated by comma,
//! semicolon or whitespace. Empty list = all allowed.

use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpRule {
    net: IpAddr,
    prefix: u8,
}

impl IpRule {
    pub fn parse(s: &str) -> Option<IpRule> {
        let s = s.trim();
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip.trim(), Some(p.trim().parse::<u8>().ok()?)),
            None => (s, None),
        };
        let net: IpAddr = ip.parse().ok()?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(IpRule { net, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // treat IPv4-mapped IPv6 addresses (dual-stack socket) like IPv4
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                (u32::from(n) & mask) == (u32::from(a) & mask)
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix)) };
                (u128::from(n) & mask) == (u128::from(a) & mask)
            }
            _ => false,
        }
    }
}

/// Read the list. Invalid entries → error with the entry.
pub fn parse_list(s: &str) -> Result<Vec<IpRule>, String> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|p| !p.trim().is_empty())
        .map(|p| IpRule::parse(p).ok_or_else(|| p.trim().to_string()))
        .collect()
}

/// Allowed? (empty list = yes)
pub fn allowed(rules: &[IpRule], ip: IpAddr) -> bool {
    rules.is_empty() || rules.iter().any(|r| r.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let l = parse_list("192.168.1.20, 10.0.0.0/8 fd00::/8").unwrap();
        assert!(allowed(&l, "192.168.1.20".parse().unwrap()));
        assert!(!allowed(&l, "192.168.1.21".parse().unwrap()));
        assert!(allowed(&l, "10.200.1.1".parse().unwrap()));
        assert!(allowed(&l, "::ffff:10.1.2.3".parse().unwrap()));
        assert!(allowed(&l, "fd12::1".parse().unwrap()));
        assert!(!allowed(&l, "2001:db8::1".parse().unwrap()));
        assert!(allowed(&[], "8.8.8.8".parse().unwrap()));
        assert_eq!(parse_list("1.2.3.4/33"), Err("1.2.3.4/33".into()));
        assert_eq!(parse_list("nope"), Err("nope".into()));
        assert!(parse_list(" ").unwrap().is_empty());
        assert!(allowed(&parse_list("0.0.0.0/0").unwrap(), "1.1.1.1".parse().unwrap()));
    }
}
