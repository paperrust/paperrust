//! `ScopedRateThrottle` with scope `login` for `/api/token/`.
//!
//! upstream: DRF 3.18.3 rest_framework/throttling.py (BaseThrottle.get_ident,
//! SimpleRateThrottle.parse_rate/allow_request/throttle_success/wait, ScopedRateThrottle),
//! rest_framework/exceptions.py (Throttled), src/paperless/settings/__init__.py@v3.4.2
//! (`DEFAULT_THROTTLE_RATES["login"] = PAPERLESS_TOKEN_THROTTLE_RATE or "5/min"`).
//!
//! The history lives in Django's `default` cache (same key, same pickle encoding, see
//! [`crate::django_cache`]), so Rust and Django count together (Review 2 M2). Only when Django
//! itself uses a process-local cache (DEBUG) the history stays in process memory.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub num_requests: usize,
    pub duration: f64,
}

/// `SimpleRateThrottle.parse_rate("5/min")`; `None` for an unparsable rate (python raises).
pub fn parse_rate(rate: &str) -> Option<Rate> {
    let (num, period) = rate.split_once('/')?;
    if period.contains('/') {
        return None;
    }
    let num_requests: usize = num.trim().parse().ok()?;
    let duration = match period.chars().next()? {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => return None,
    };
    Some(Rate { num_requests, duration })
}

/// The configured `login` rate (`Config::token_throttle_rate`, i.e. `PAPERLESS_TOKEN_THROTTLE_RATE`
/// or `5/min`).
pub fn login_rate(cfg: &pl_core::config::Config) -> Option<Rate> {
    parse_rate(&cfg.token_throttle_rate)
}

/// `SimpleRateThrottle.allow_request` on an already loaded history: `Ok(new_history)` (with
/// `now` inserted at the front, `throttle_success`) or `Err(wait)` (`throttle_failure`, nothing
/// is written).
pub fn decide(mut history: Vec<f64>, rate: Rate, now: f64) -> Result<Vec<f64>, Option<f64>> {
    while let Some(last) = history.last() {
        if *last <= now - rate.duration {
            history.pop();
        } else {
            break;
        }
    }
    if history.len() >= rate.num_requests {
        let remaining = match history.last() {
            Some(last) => rate.duration - (now - last),
            None => rate.duration,
        };
        let available = rate.num_requests as i64 - history.len() as i64 + 1;
        if available <= 0 {
            return Err(None);
        }
        return Err(Some(remaining / available as f64));
    }
    history.insert(0, now);
    Ok(history)
}

/// `allow_request` against Django's `default` cache via [`pl_core::cache::DjangoCache`]
/// (process-local `LocMemCache` stand-in or shared in PostgreSQL; same pickle bytes,
/// compare-and-set). Retried on conflict; after repeated conflicts the request counts as throttled.
pub async fn allow_cached(
    cache: &pl_core::cache::DjangoCache<'_>,
    key: &str,
    rate: Rate,
    now: f64,
) -> Result<Result<(), Option<f64>>, pl_core::ApiError> {
    for _ in 0..8 {
        let raw = cache.get(key).await?;
        let history = match &raw {
            Some(b) => crate::django_cache::unpickle_float_list(b).map_err(pl_core::ApiError::internal)?,
            None => Vec::new(),
        };
        let new = match decide(history, rate, now) {
            Ok(h) => h,
            Err(wait) => return Ok(Err(wait)),
        };
        let bytes = crate::django_cache::pickle_float_list(&new);
        // `cache.set(key, history, duration)`: Django passes the int duration as EX
        let ttl = (rate.duration as u64).max(1);
        if cache.compare_and_set(key, raw.as_deref(), &bytes, Some(ttl)).await? {
            return Ok(Ok(()));
        }
    }
    Ok(Err(None))
}

/// `BaseThrottle.get_ident` with `NUM_PROXIES = None`.
pub fn get_ident(x_forwarded_for: Option<&str>, remote_addr: Option<&str>) -> String {
    match x_forwarded_for {
        Some(xff) if !xff.is_empty() => xff.split_whitespace().collect::<String>(),
        // python: `''.join(xff.split()) if xff else remote_addr` – an empty header is falsy
        _ => remote_addr.unwrap_or("None").to_string(),
    }
}

#[derive(Default)]
pub struct Throttle {
    history: Mutex<HashMap<String, Vec<f64>>>,
}

impl Throttle {
    /// `allow_request`; `Err(wait)` when throttled (`wait` as python `wait()`, may be `None`).
    pub fn allow(&self, key: &str, rate: Rate, now: f64) -> Result<(), Option<f64>> {
        let mut map = match self.history.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        // opportunistic cleanup of expired keys (cache timeout = duration)
        if map.len() > 10_000 {
            map.retain(|_, h| h.first().is_some_and(|t| *t > now - rate.duration));
        }
        let history = map.entry(key.to_string()).or_default();
        // throttle_failure() does not write: keep the stored history unchanged
        let h = decide(history.clone(), rate, now)?;
        *history = h;
        Ok(())
    }
}

/// `Throttled(wait).detail` and the `Retry-After` value.
pub fn throttled_detail(wait: Option<f64>) -> (String, Option<u64>) {
    let base = "Request was throttled.";
    match wait {
        Some(w) => {
            let w = w.ceil().max(0.0) as u64;
            let unit = if w == 1 { "second" } else { "seconds" };
            (format!("{base} Expected available in {w} {unit}."), Some(w))
        }
        None => (base.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_window() {
        let r = parse_rate("5/min").unwrap();
        assert_eq!(r.num_requests, 5);
        assert_eq!(r.duration, 60.0);
        assert_eq!(parse_rate("10/second").unwrap().duration, 1.0);
        assert!(parse_rate("abc").is_none());
        let t = Throttle::default();
        for i in 0..5 {
            assert!(t.allow("k", r, 100.0 + i as f64).is_ok());
        }
        let wait = t.allow("k", r, 110.0).unwrap_err().unwrap();
        assert!((wait - 50.0).abs() < 1e-9);
        assert_eq!(throttled_detail(Some(49.2)).0, "Request was throttled. Expected available in 50 seconds.");
        assert_eq!(throttled_detail(Some(0.4)).0, "Request was throttled. Expected available in 1 second.");
        assert!(t.allow("k", r, 160.5).is_ok());
    }

    #[test]
    fn ident() {
        assert_eq!(get_ident(Some("1.2.3.4, 5.6.7.8"), Some("9.9.9.9")), "1.2.3.4,5.6.7.8");
        assert_eq!(get_ident(None, Some("9.9.9.9")), "9.9.9.9");
    }
}
