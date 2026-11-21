//! allauth rate limiting in the shared Django cache.
//!
//! upstream: allauth 65.20.1 core/internal/ratelimit.py (`parse_rates`, `get_cache_key`,
//! `cache_lock` = `cache.add("<key>:lock", True, 10)` with 1 s wait time, `apply_rate`,
//! `_consume_single_rate`, `SingleRateLimitUsage.rollback`, `clear`, `handler429`),
//! core/internal/httpkit.py (`get_client_ip`, `clean_client_ip`, `get_client_ip_from_xff`).
//! Same lock as allauth. Keys carry the time window since 2026-11-15 (security fix taken from
//! allauth 65.21.1, own implementation) – no longer identical to allauth 65.20.1; Django no longer runs alongside.

use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cache::{Cache, CacheError, PICKLED_TRUE, pickle_float_list, unpickle_float_list};

/// `Rate(amount, duration, per)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub amount: usize,
    pub duration: f64,
    pub per: Per,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Per {
    Ip,
    User,
    Key,
}

fn parse_duration(d: &str) -> Option<f64> {
    let unit = d.chars().last()?;
    let mult = match unit {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => return None,
    };
    let v = &d[..d.len() - 1];
    let n = if v.is_empty() { 1.0 } else { v.parse::<f64>().ok()? };
    Some(n * mult)
}

/// `parse_rates("10/m/ip,5/300s/key")`.
pub fn parse_rates(rates: &str) -> Vec<Rate> {
    rates
        .trim()
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .filter_map(|p| {
            let parts: Vec<&str> = p.trim().split('/').collect();
            let (amount, dur, per) = match parts.as_slice() {
                [a, d] => (*a, *d, "ip"),
                [a, d, p] => (*a, *d, *p),
                _ => return None,
            };
            Some(Rate {
                amount: amount.parse().ok()?,
                duration: parse_duration(dur)?,
                per: match per {
                    "ip" => Per::Ip,
                    "user" => Per::User,
                    "key" => Per::Key,
                    _ => return None,
                },
            })
        })
        .collect()
}

/// Source of the limit (IP, user id, arbitrary key).
#[derive(Debug, Clone, Default)]
pub struct Subject<'a> {
    pub ip: Option<&'a str>,
    pub user_id: Option<i64>,
    pub key: Option<&'a str>,
}

/// Time window as key part: whole seconds without decimals (`300s`), otherwise like `f64` (`0.5s`).
fn window_part(duration: f64) -> String {
    if duration.fract() == 0.0 && duration.abs() < 1e15 { format!("{}s", duration as i64) } else { format!("{duration}s") }
}

/// `get_cache_key(app="allauth.account", action, rate, key, user)`, additionally with the time window of the
/// rate: two limits for the same action and source with different windows (e.g. `5/m/ip,20/h/ip`)
/// count separately instead of sharing one counter (otherwise the short window trims the long one's
/// history when writing). Idea from allauth 65.21.1. Format:
/// `allauth:rl:allauth.account:<action>:<window>:<source>`.
pub fn cache_key(action: &str, rate: &Rate, s: &Subject<'_>) -> Option<String> {
    let source = match rate.per {
        Per::Ip => format!("ip:{}", s.ip?),
        Per::User => format!("user:{}", s.user_id?),
        Per::Key => {
            let h = Sha256::digest(s.key?.as_bytes());
            h.iter().map(|b| format!("{b:02x}")).collect()
        }
    };
    let window = window_part(rate.duration);
    Some(format!("allauth:rl:allauth.account:{action}:{window}:{source}"))
}

/// `apply_rate(history, now, rate)` → (allowed, new history).
pub fn apply_rate(mut history: Vec<f64>, now: f64, rate: &Rate, dry_run: bool) -> (bool, Vec<f64>) {
    while history.last().is_some_and(|&t| t <= now - rate.duration) {
        history.pop();
    }
    let allowed = history.len() < rate.amount;
    if allowed && !dry_run {
        history.insert(0, now);
    }
    (allowed, history)
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// One consumed unit (for `rollback`).
#[derive(Debug, Clone)]
pub struct Usage {
    pub cache_key: String,
    pub timestamp: f64,
    pub duration: f64,
}

/// `cache_lock(key)`: `add("<key>:lock", True, 10)`, wait up to 1 s.
async fn lock(cache: &Cache, key: &str) -> Result<bool, CacheError> {
    let lock_key = format!("{key}:lock");
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut retry = 0.01f64;
    loop {
        if cache.add(&lock_key, PICKLED_TRUE, 10).await? {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs_f64(retry).min(remaining)).await;
        retry = (retry * 2.0).min(0.1);
    }
}

async fn unlock(cache: &Cache, key: &str) {
    let _ = cache.delete(&format!("{key}:lock")).await;
}

async fn history(cache: &Cache, key: &str) -> Result<Vec<f64>, CacheError> {
    Ok(match cache.get(key).await? {
        Some(raw) => unpickle_float_list(&raw).unwrap_or_default(),
        None => Vec::new(),
    })
}

/// `consume(request, action=…, key=…, user=…)` (POST; GET is not limited by allauth).
/// `Ok(None)` = limited.
pub async fn consume(cache: &Cache, action: &str, rates: &str, s: &Subject<'_>, dry_run: bool) -> Result<Option<Vec<Usage>>, CacheError> {
    let mut usages = Vec::new();
    for rate in parse_rates(rates) {
        let Some(key) = cache_key(action, &rate, s) else {
            // ImproperlyConfigured in allauth – does not occur with paperless
            continue;
        };
        if !lock(cache, &key).await? {
            return Ok(None);
        }
        let result = async {
            let h = history(cache, &key).await?;
            let t = now();
            let (allowed, h) = apply_rate(h, t, &rate, dry_run);
            if allowed && !dry_run {
                cache.set(&key, &pickle_float_list(&h), rate.duration as u64).await?;
            }
            Ok::<_, CacheError>(allowed.then_some(Usage { cache_key: key.clone(), timestamp: t, duration: rate.duration }))
        }
        .await;
        unlock(cache, &key).await;
        match result? {
            Some(u) => usages.push(u),
            None => return Ok(None),
        }
    }
    Ok(Some(usages))
}

/// `RateLimitUsage.rollback()`.
pub async fn rollback(cache: &Cache, usages: &[Usage]) -> Result<(), CacheError> {
    for u in usages {
        if lock(cache, &u.cache_key).await? {
            let r = async {
                let h: Vec<f64> = history(cache, &u.cache_key).await?.into_iter().filter(|t| *t != u.timestamp).collect();
                cache.set(&u.cache_key, &pickle_float_list(&h), u.duration as u64).await
            }
            .await;
            unlock(cache, &u.cache_key).await;
            r?;
        }
    }
    Ok(())
}

/// `ratelimit.clear(request, action=…, key=…)`.
pub async fn clear(cache: &Cache, action: &str, rates: &str, s: &Subject<'_>) -> Result<(), CacheError> {
    for rate in parse_rates(rates) {
        if let Some(key) = cache_key(action, &rate, s)
            && lock(cache, &key).await?
        {
            let r = cache.delete(&key).await;
            unlock(cache, &key).await;
            r?;
        }
    }
    Ok(())
}

/// `handler429` without `429.html` (paperless/allauth ship none): status 429, `text/html`.
pub const HTML_429: &str = "<html>\n    <head><title>Too Many Requests</title></head>\n    <body>\n        <h1>429 Too Many Requests</h1>\n        <p>You have sent too many requests. Please try again later.</p>\n    </body>\n</html>";

/// `clean_client_ip`: strip port, normalize IP (`ipaddress.ip_address`).
pub fn clean_client_ip(ip: &str) -> Option<String> {
    let ip = ip.trim();
    let (domain, port) = pl_authz::csrf::split_domain_port(ip);
    let candidate = if !port.is_empty() && !domain.is_empty() {
        domain.trim_start_matches('[').trim_end_matches(']').to_string()
    } else {
        ip.to_string()
    };
    candidate.parse::<std::net::IpAddr>().ok().map(|a| a.to_string())
}

/// `truncate_ip`: IPv6 to the /64 prefix (`RATE_LIMIT_IPV6_PREFIX`).
pub fn truncate_ip(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(a)) => {
            let seg = a.segments();
            std::net::Ipv6Addr::new(seg[0], seg[1], seg[2], seg[3], 0, 0, 0, 0).to_string()
        }
        _ => ip.to_string(),
    }
}

/// `get_client_ip(request)` (trusted header or X-Forwarded-For with proxy count, otherwise
/// `REMOTE_ADDR`). `Err` = ImproperlyConfigured (too few XFF entries → 500).
/// Error: too few `X-Forwarded-For` entries for the proxy count.
#[derive(Debug, PartialEq, Eq)]
pub struct BadProxyCount;

pub fn client_ip(
    trusted_header: Option<&str>,
    trusted_proxy_count: i64,
    header: impl Fn(&str) -> Option<String>,
    remote_addr: Option<&str>,
) -> Result<Option<String>, BadProxyCount> {
    let ip = if let Some(h) = trusted_header {
        header(h)
    } else {
        let xff = header("x-forwarded-for");
        let from_xff = match xff {
            Some(x) if trusted_proxy_count > 0 => {
                let ips: Vec<&str> = x.split(',').collect();
                if (ips.len() as i64) < trusted_proxy_count {
                    return Err(BadProxyCount);
                }
                Some(ips[ips.len() - trusted_proxy_count as usize].to_string())
            }
            _ => None,
        };
        from_xff.filter(|s| !s.is_empty()).or_else(|| remote_addr.map(str::to_string))
    };
    Ok(ip.and_then(|i| clean_client_ip(&i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        let r = parse_rates("10/m/ip,5/300s/key");
        assert_eq!(r[0], Rate { amount: 10, duration: 60.0, per: Per::Ip });
        assert_eq!(r[1], Rate { amount: 5, duration: 300.0, per: Per::Key });
        assert_eq!(parse_rates("30/m")[0].per, Per::Ip);
        assert!(parse_rates("").is_empty());
    }

    #[test]
    fn keys_like_allauth() {
        let rate = parse_rates("5/300s/key")[0];
        // sha256("127.0.0.1:8120:admin")
        let k = cache_key("login_failed", &rate, &Subject { key: Some("127.0.0.1:8120:admin"), ..Default::default() }).unwrap();
        assert!(k.starts_with("allauth:rl:allauth.account:login_failed:300s:"));
        assert_eq!(k.len(), "allauth:rl:allauth.account:login_failed:300s:".len() + 64);
        let ip = cache_key("login", &parse_rates("30/m/ip")[0], &Subject { ip: Some("1.2.3.4"), ..Default::default() }).unwrap();
        assert_eq!(ip, "allauth:rl:allauth.account:login:60s:ip:1.2.3.4");
        let user = cache_key("x", &parse_rates("1/0.5s/user")[0], &Subject { user_id: Some(7), ..Default::default() }).unwrap();
        assert_eq!(user, "allauth:rl:allauth.account:x:0.5s:user:7");
    }

    /// Two limits with different windows for the same action and source count separately.
    #[tokio::test]
    async fn windows_count_separately() {
        // process-local cache; the pool is never connected (lazy)
        let env = pl_core::config::Env::from_pairs([("PAPERLESS_DBHOST", "127.0.0.1"), ("PAPERLESS_SECRET_KEY", "test-only")]);
        let state = pl_core::AppState::new(pl_core::Config::from_env_map(env).unwrap()).unwrap();
        let cache = Cache::backend_only(&state);
        let rates = parse_rates("2/m/ip,3/h/ip");
        let s = Subject { ip: Some("1.2.3.4"), ..Default::default() };
        let minute = cache_key("login", &rates[0], &s).unwrap();
        let hour = cache_key("login", &rates[1], &s).unwrap();
        assert_ne!(minute, hour);
        for _ in 0..2 {
            assert!(consume(&cache, "login", "2/m/ip,3/h/ip", &s, false).await.unwrap().is_some());
        }
        // a separate counter per window with two hits
        let hits = |raw: Option<Vec<u8>>| raw.and_then(|r| unpickle_float_list(&r).ok()).map(|h| h.len());
        assert_eq!(hits(cache.get(&minute).await.unwrap()), Some(2));
        assert_eq!(hits(cache.get(&hour).await.unwrap()), Some(2));
        // third time: the minute limit (2) kicks in, the hour counter stays at 2
        assert!(consume(&cache, "login", "2/m/ip,3/h/ip", &s, false).await.unwrap().is_none());
        assert_eq!(hits(cache.get(&hour).await.unwrap()), Some(2));
        // only the hour limit: keeps counting alone up to 3
        assert!(consume(&cache, "login", "3/h/ip", &s, false).await.unwrap().is_some());
        assert_eq!(hits(cache.get(&hour).await.unwrap()), Some(3));
        assert!(consume(&cache, "login", "3/h/ip", &s, false).await.unwrap().is_none());
    }

    #[test]
    fn window() {
        let rate = Rate { amount: 2, duration: 60.0, per: Per::Ip };
        let (a, h) = apply_rate(vec![], 100.0, &rate, false);
        assert!(a);
        let (b, h) = apply_rate(h, 101.0, &rate, false);
        assert!(b);
        let (c, h) = apply_rate(h, 102.0, &rate, false);
        assert!(!c);
        assert_eq!(h, vec![101.0, 100.0]);
        let (d, h) = apply_rate(h, 160.5, &rate, false);
        assert!(d);
        assert_eq!(h, vec![160.5, 101.0]);
    }

    #[test]
    fn ips() {
        assert_eq!(clean_client_ip("127.0.0.1").as_deref(), Some("127.0.0.1"));
        assert_eq!(clean_client_ip("127.0.0.1:5555").as_deref(), Some("127.0.0.1"));
        assert_eq!(clean_client_ip("[::1]:80").as_deref(), Some("::1"));
        assert_eq!(clean_client_ip("nonsense"), None);
        assert_eq!(truncate_ip("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::");
        let h = |_: &str| Some("9.9.9.9, 10.0.0.1".to_string());
        assert_eq!(client_ip(None, 1, h, Some("127.0.0.1")), Ok(Some("10.0.0.1".into())));
        assert_eq!(client_ip(None, 2, h, Some("127.0.0.1")), Ok(Some("9.9.9.9".into())));
        assert_eq!(client_ip(None, 3, h, Some("127.0.0.1")), Err(BadProxyCount));
        assert_eq!(client_ip(None, 0, h, Some("127.0.0.1")), Ok(Some("127.0.0.1".into())));
    }
}
