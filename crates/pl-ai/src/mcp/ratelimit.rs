//! Rate limit per key (token, user, client IP): sliding window for requests/minute
//! and creations/hour, plus general quotas ([`Budget`]).
//! In-process only (several PaperRust processes count separately – noted in the contract).
//!
//! Security review final M3: both tables have a **hard upper limit** on the
//! number of keys ([`MAX_KEYS`]); empty windows are removed **on a timer** (at most every
//! [`PRUNE_INTERVAL`], not on every call). If the table is still full after cleanup,
//! a **new** key is rejected (fail-closed: `Err`/exhausted), existing keys
//! keep counting normally.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Maximum number of keys counted at the same time per table.
pub const MAX_KEYS: usize = 50_000;
/// Interval between two cleanup runs (remove empty windows).
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(10);

/// Key table with upper limit and timed cleanup.
struct Keyed<V> {
    map: HashMap<String, V>,
    max_keys: usize,
    next_prune: Option<Instant>,
}

impl<V: Default> Keyed<V> {
    fn new(max_keys: usize) -> Self {
        Self { map: HashMap::new(), max_keys, next_prune: None }
    }

    /// Entry for `key`; `None` = table full (fail-closed). `alive` checks (and trims) an
    /// entry during cleanup.
    fn entry(&mut self, key: &str, now: Instant, mut alive: impl FnMut(&mut V) -> bool) -> Option<&mut V> {
        let due = self.next_prune.is_none_or(|t| now >= t);
        if due {
            self.map.retain(|_, v| alive(v));
            self.next_prune = Some(now + PRUNE_INTERVAL);
        }
        if !self.map.contains_key(key) && self.map.len() >= self.max_keys {
            return None;
        }
        Some(self.map.entry(key.to_string()).or_default())
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

#[derive(Default)]
struct Windows {
    requests: VecDeque<Instant>,
    creates: VecDeque<Instant>,
}

/// Limits and counters.
pub struct RateLimiter {
    per_minute: u32,
    creates_per_hour: u32,
    inner: Mutex<Keyed<Windows>>,
}

fn prune(q: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
        q.pop_front();
    }
}

impl RateLimiter {
    pub fn new(per_minute: u32, creates_per_hour: u32) -> Self {
        Self::with_max_keys(per_minute, creates_per_hour, MAX_KEYS)
    }

    /// Like [`new`](Self::new) with its own key upper limit (tests).
    pub fn with_max_keys(per_minute: u32, creates_per_hour: u32, max_keys: usize) -> Self {
        Self { per_minute, creates_per_hour, inner: Mutex::new(Keyed::new(max_keys)) }
    }

    /// Counts a request. `Err(seconds)` = limit reached, wait time until the next slot.
    pub fn check_request(&self, key: &str) -> Result<(), u64> {
        self.check_at(key, Instant::now(), false)
    }

    /// Counts a creation (in addition to the request).
    pub fn check_create(&self, key: &str) -> Result<(), u64> {
        self.check_at(key, Instant::now(), true)
    }

    /// Number of counted keys (tests/diagnostics).
    pub fn key_count(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub(crate) fn check_at(&self, key: &str, now: Instant, create: bool) -> Result<(), u64> {
        let Ok(mut g) = self.inner.lock() else { return Err(60) };
        let Some(w) = g.entry(key, now, |w| {
            prune(&mut w.requests, now, Duration::from_secs(60));
            prune(&mut w.creates, now, Duration::from_secs(3600));
            !(w.requests.is_empty() && w.creates.is_empty())
        }) else {
            return Err(PRUNE_INTERVAL.as_secs().max(1));
        };
        let (q, limit, window) = if create {
            (&mut w.creates, self.creates_per_hour, Duration::from_secs(3600))
        } else {
            (&mut w.requests, self.per_minute, Duration::from_secs(60))
        };
        prune(q, now, window);
        if q.len() as u64 >= u64::from(limit) {
            let wait = q.front().map(|t| window.saturating_sub(now.duration_since(*t)).as_secs() + 1).unwrap_or(1);
            return Err(wait);
        }
        q.push_back(now);
        Ok(())
    }
}

/// General quota: at most `limit` units per sliding `window` and key
/// (security wave 3 M1/M3: manual AI runs per user, failed MCP logins per
/// client IP and as a global emergency brake).
pub struct Budget {
    limit: u32,
    window: Duration,
    inner: Mutex<Keyed<VecDeque<Instant>>>,
}

impl Budget {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self::with_max_keys(limit, window, MAX_KEYS)
    }

    /// Like [`new`](Self::new) with its own key upper limit (tests).
    pub fn with_max_keys(limit: u32, window: Duration, max_keys: usize) -> Self {
        Self { limit, window, inner: Mutex::new(Keyed::new(max_keys)) }
    }

    fn wait(&self, q: &VecDeque<Instant>, now: Instant) -> u64 {
        q.front().map(|t| self.window.saturating_sub(now.duration_since(*t)).as_secs() + 1).unwrap_or(1)
    }

    /// `None` = locked/table full (callers treat this as exhausted).
    fn with_queue<R>(&self, key: &str, now: Instant, f: impl FnOnce(&mut VecDeque<Instant>) -> R) -> Option<R> {
        let mut g = self.inner.lock().ok()?;
        let w = self.window;
        let q = g.entry(key, now, |q| {
            prune(q, now, w);
            !q.is_empty()
        })?;
        prune(q, now, self.window);
        Some(f(q))
    }

    /// Number of counted keys (tests/diagnostics).
    pub fn key_count(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Takes `n` units at once (all or nothing). `Err(seconds)` = not enough free.
    pub fn try_take(&self, key: &str, n: u32) -> Result<(), u64> {
        self.try_take_at(key, n, Instant::now())
    }

    pub(crate) fn try_take_at(&self, key: &str, n: u32, now: Instant) -> Result<(), u64> {
        self.with_queue(key, now, |q| {
            if q.len() as u64 + u64::from(n) > u64::from(self.limit) {
                return Err(if q.is_empty() { self.window.as_secs().max(1) } else { self.wait(q, now) });
            }
            q.extend(std::iter::repeat_n(now, n as usize));
            Ok(())
        })
        .unwrap_or(Err(60))
    }

    /// Is the quota exhausted? (`Some(seconds)`), without consuming anything.
    pub fn exhausted(&self, key: &str) -> Option<u64> {
        self.exhausted_at(key, Instant::now())
    }

    pub(crate) fn exhausted_at(&self, key: &str, now: Instant) -> Option<u64> {
        self.with_queue(key, now, |q| (q.len() as u64 >= u64::from(self.limit)).then(|| self.wait(q, now))).unwrap_or(Some(60))
    }

    /// Consumes one unit without checking (e.g. count a failed attempt).
    pub fn record(&self, key: &str) {
        self.record_at(key, Instant::now());
    }

    pub(crate) fn record_at(&self, key: &str, now: Instant) {
        let _ = self.with_queue(key, now, |q| q.push_back(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window() {
        let r = RateLimiter::new(2, 1);
        let t0 = Instant::now();
        assert!(r.check_at("a", t0, false).is_ok());
        assert!(r.check_at("a", t0, false).is_ok());
        assert!(r.check_at("a", t0, false).is_err());
        assert!(r.check_at("b", t0, false).is_ok(), "pro Token getrennt");
        assert!(r.check_at("a", t0 + Duration::from_secs(61), false).is_ok());
        assert!(r.check_at("a", t0, true).is_ok());
        assert!(r.check_at("a", t0, true).is_err());
    }

    #[test]
    fn budget_all_or_nothing() {
        let b = Budget::new(5, Duration::from_secs(3600));
        let t0 = Instant::now();
        assert!(b.try_take_at("u1", 3, t0).is_ok());
        assert!(b.try_take_at("u1", 3, t0).is_err(), "3 + 3 > 5: nichts verbraucht");
        assert!(b.try_take_at("u1", 2, t0).is_ok());
        assert!(b.exhausted_at("u1", t0).is_some());
        assert!(b.try_take_at("u2", 5, t0).is_ok(), "je Schlüssel getrennt");
        assert!(b.try_take_at("u3", 6, t0).is_err(), "mehr als das Limit nie");
        assert!(b.try_take_at("u1", 5, t0 + Duration::from_secs(3601)).is_ok(), "Fenster gleitet");
        b.record_at("f", t0);
        assert!(b.exhausted_at("f", t0).is_none());
    }

    /// M3: hard upper limit on the number of keys (new keys fail-closed, existing ones keep
    /// counting), cleanup of empty windows only on a timer.
    #[test]
    fn key_cap_and_timed_prune() {
        let r = RateLimiter::with_max_keys(5, 1, 3);
        let t0 = Instant::now();
        for k in ["a", "b", "c"] {
            assert!(r.check_at(k, t0, false).is_ok());
        }
        assert!(r.check_at("d", t0, false).is_err(), "Tabelle voll → neuer Schlüssel abgewiesen");
        assert!(r.check_at("a", t0, false).is_ok(), "bestehender Schlüssel zählt weiter");
        assert_eq!(r.key_count(), 3);
        // after the windows and the cleanup interval have expired there is room again
        let t1 = t0 + Duration::from_secs(61);
        assert!(r.check_at("d", t1, false).is_ok());
        assert_eq!(r.key_count(), 1);

        let b = Budget::with_max_keys(2, Duration::from_secs(60), 2);
        b.record_at("x", t0);
        b.record_at("y", t0);
        assert!(b.exhausted_at("z", t0).is_some(), "voll → als erschöpft behandeln");
        assert!(b.try_take_at("z", 1, t0).is_err());
        assert!(b.exhausted_at("x", t0).is_none());
        // within the interval no full scan: expired keys stay until the next run
        let _ = b.exhausted_at("x", t0 + PRUNE_INTERVAL / 2);
        assert_eq!(b.key_count(), 2);
        assert!(b.exhausted_at("z", t0 + Duration::from_secs(61)).is_none());
        assert_eq!(b.key_count(), 1);
    }

    /// Many different keys: memory stays bounded.
    #[test]
    fn many_keys_bounded() {
        let r = RateLimiter::new(10, 1);
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS + 1000) {
            let _ = r.check_at(&format!("pre:{i}"), t0, false);
        }
        assert_eq!(r.key_count(), MAX_KEYS);
    }
}
