//! Throttling of failed HTTP Basic logins (PaperRust extension, security review
//! 2026-11-18, results/release/security/BERICHT.md finding S-2).
//!
//! upstream (`PaperlessBasicAuthentication`) does not limit Basic auth: the login page locks after
//! 5 failed attempts per account (allauth `login_failed` 10/m/ip, 5/300s/key), but via `Authorization: Basic`
//! the password could be guessed any number of times. Here the same limits apply to Basic auth:
//! at most 10 failed attempts per client IP in 60 s and 5 per username (lower-cased) in 300 s.
//! Once a limit is reached, the password is not checked at all any more (which also saves the expensive
//! hash computation) and the request is rejected with 401 `Too many failed login attempts.`.
//!
//! Counters are per process (in memory, bounded size). Several server processes count separately; the
//! login page itself continues to use the shared allauth counter in PostgreSQL.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Failed attempts per client IP within the window [`IP_WINDOW`].
pub const IP_MAX: usize = 10;
pub const IP_WINDOW: Duration = Duration::from_secs(60);
/// Failed attempts per username within the window [`USER_WINDOW`].
pub const USER_MAX: usize = 5;
pub const USER_WINDOW: Duration = Duration::from_secs(300);
/// Upper limit of remembered keys (protection against memory growth through many names/IPs).
const MAX_KEYS: usize = 50_000;

/// When memory is full, `MAX_KEYS / EVICT_SHARE` (10 %) of the oldest keys are evicted.
const EVICT_SHARE: usize = 10;

/// Window of a key (`ip:` or `user:`).
fn window_of(key: &str) -> Duration {
    if key.starts_with("ip:") { IP_WINDOW } else { USER_WINDOW }
}

/// Detail text of the rejection.
pub const DETAIL: &str = "Too many failed login attempts.";

#[derive(Default)]
struct Counters {
    hits: HashMap<String, VecDeque<Instant>>,
}

impl Counters {
    fn count(&mut self, key: &str, window: Duration, now: Instant) -> usize {
        let Some(q) = self.hits.get_mut(key) else { return 0 };
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        let n = q.len();
        if n == 0 {
            self.hits.remove(key);
        }
        n
    }

    /// Make room for new keys. Security review 2026-11-19 F7: never delete all counters
    /// (otherwise a flood of new names/IPs resets the lock of an attacked account).
    /// First discard expired entries (each key with its own window); if that is not
    /// enough, evict the oldest [`EVICT_SHARE`] (by last failed attempt).
    fn make_room(&mut self, now: Instant) {
        self.hits.retain(|k, q| q.back().is_some_and(|t| now.duration_since(*t) < window_of(k)));
        if self.hits.len() < MAX_KEYS {
            return;
        }
        let evict = (MAX_KEYS / EVICT_SHARE).max(1);
        let mut last: Vec<Instant> = self.hits.values().filter_map(|q| q.back().copied()).collect();
        if last.len() <= evict {
            return;
        }
        // point in time up to which (inclusive) entries are evicted
        let (_, cutoff, _) = last.select_nth_unstable(evict - 1);
        let cutoff = *cutoff;
        let mut removed = 0usize;
        self.hits.retain(|_, q| {
            if removed < evict && q.back().is_none_or(|t| *t <= cutoff) {
                removed += 1;
                false
            } else {
                true
            }
        });
    }

    fn add(&mut self, key: String, window: Duration, max: usize, now: Instant) {
        if self.hits.len() >= MAX_KEYS && !self.hits.contains_key(&key) {
            self.make_room(now);
        }
        let q = self.hits.entry(key).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        q.push_back(now);
        while q.len() > max {
            q.pop_front();
        }
    }
}

fn counters() -> &'static Mutex<Counters> {
    static C: OnceLock<Mutex<Counters>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Counters::default()))
}

fn ip_key(ip: Option<IpAddr>) -> Option<String> {
    ip.map(|i| format!("ip:{i}"))
}

fn user_key(username: &str) -> String {
    format!("user:{}", username.to_lowercase())
}

/// Has the limit been reached for this IP or this username?
pub fn is_blocked(ip: Option<IpAddr>, username: &str) -> bool {
    is_blocked_at(ip, username, Instant::now())
}

fn is_blocked_at(ip: Option<IpAddr>, username: &str, now: Instant) -> bool {
    let mut c = counters().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = ip_key(ip)
        && c.count(&k, IP_WINDOW, now) >= IP_MAX
    {
        return true;
    }
    c.count(&user_key(username), USER_WINDOW, now) >= USER_MAX
}

/// Count a failed attempt.
pub fn record_failure(ip: Option<IpAddr>, username: &str) {
    record_failure_at(ip, username, Instant::now());
}

fn record_failure_at(ip: Option<IpAddr>, username: &str, now: Instant) {
    let mut c = counters().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = ip_key(ip) {
        c.add(k, IP_WINDOW, IP_MAX, now);
    }
    c.add(user_key(username), USER_WINDOW, USER_MAX, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        s.parse().ok()
    }

    #[test]
    fn user_limit_after_five_failures() {
        let now = Instant::now();
        let u = "Throttle-User-A";
        for _ in 0..USER_MAX {
            assert!(!is_blocked_at(ip("192.0.2.10"), u, now));
            record_failure_at(ip("192.0.2.10"), u, now);
        }
        assert!(is_blocked_at(ip("198.51.100.7"), "throttle-user-a", now), "name is case-insensitive, any IP");
        assert!(!is_blocked_at(ip("198.51.100.7"), "throttle-user-other", now));
        assert!(!is_blocked_at(ip("198.51.100.7"), u, now + USER_WINDOW), "window expires");
    }

    #[test]
    fn ip_limit_after_ten_failures() {
        let now = Instant::now();
        for i in 0..IP_MAX {
            record_failure_at(ip("203.0.113.5"), &format!("throttle-ip-{i}"), now);
        }
        assert!(is_blocked_at(ip("203.0.113.5"), "throttle-ip-fresh", now));
        assert!(!is_blocked_at(ip("203.0.113.6"), "throttle-ip-fresh", now));
        assert!(!is_blocked_at(ip("203.0.113.5"), "throttle-ip-fresh", now + IP_WINDOW));
    }

    /// Security review 2026-11-19 F7: a flood of new keys must not reset the lock of an account
    /// currently under attack (previously: everything deleted at 50,000 keys).
    #[test]
    fn key_flood_keeps_recent_lock() {
        let mut c = Counters::default();
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS - 1) {
            c.add(format!("user:old{i}"), USER_WINDOW, USER_MAX, t0);
        }
        let t1 = t0 + Duration::from_secs(1);
        for _ in 0..USER_MAX {
            c.add("user:victim".into(), USER_WINDOW, USER_MAX, t1);
        }
        let t2 = t0 + Duration::from_secs(2);
        for i in 0..1000 {
            c.add(format!("user:flood{i}"), USER_WINDOW, USER_MAX, t2);
        }
        assert!(c.hits.len() <= MAX_KEYS);
        assert_eq!(c.count("user:victim", USER_WINDOW, t2), USER_MAX, "lock of the attacked account survives");
        assert!(c.count("user:flood999", USER_WINDOW, t2) == 1);
    }

    /// Expired IP entries (60 s) are removed before fresh entries are evicted.
    #[test]
    fn expired_ip_keys_go_first() {
        let mut c = Counters::default();
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS - 1) {
            c.add(format!("ip:10.0.{}.{}", i / 256, i % 256), IP_WINDOW, IP_MAX, t0);
        }
        let t1 = t0 + Duration::from_secs(5);
        c.add("user:recent".into(), USER_WINDOW, USER_MAX, t1);
        let t2 = t0 + IP_WINDOW + Duration::from_secs(1);
        c.add("user:new".into(), USER_WINDOW, USER_MAX, t2);
        assert_eq!(c.hits.len(), 2, "only the expired ip keys were dropped");
        assert_eq!(c.count("user:recent", USER_WINDOW, t2), 1);
    }

    #[test]
    fn bounded_memory() {
        let mut c = Counters::default();
        let now = Instant::now();
        for i in 0..(MAX_KEYS + 10) {
            c.add(format!("user:k{i}"), USER_WINDOW, USER_MAX, now);
        }
        assert!(c.hits.len() <= MAX_KEYS);
        c.add("user:x".into(), USER_WINDOW, USER_MAX, now);
        for _ in 0..20 {
            c.add("user:x".into(), USER_WINDOW, USER_MAX, now);
        }
        assert!(c.hits.get("user:x").is_some_and(|q| q.len() <= USER_MAX));
    }
}
