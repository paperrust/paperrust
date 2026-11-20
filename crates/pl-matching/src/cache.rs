//! Process-wide cache of compiled patterns (PERF-SCALE Q8).
//!
//! `matches()` used to recompile every keyword pattern per call and candidate
//! (suggestions over 5,000 correspondents/800 tags = thousands of regex compilations per request).
//! The key is the full pattern text including the case-sensitivity switch – a changed
//! `match` of an object yields a new key, old entries age out via the upper bound.
//! Results are identical (same pattern, same options); only successful
//! compilations are remembered, errors behave as before (incl. log message) every time.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

/// Upper bound per cache; on overflow it is cleared (simple, without LRU bookkeeping – the
/// working set of real collections is far below it).
pub const MAX_ENTRIES: usize = 50_000;

pub struct PatternCache<K, V> {
    map: Mutex<Option<HashMap<K, Arc<V>>>>,
}

impl<K: Eq + Hash, V> PatternCache<K, V> {
    pub const fn new() -> Self {
        Self { map: Mutex::new(None) }
    }

    /// Entry for `key` or `build()` (success is remembered, errors are not).
    pub fn get_or_try<E>(&self, key: K, build: impl FnOnce() -> Result<V, E>) -> Result<Arc<V>, E> {
        if let Ok(g) = self.map.lock() {
            if let Some(v) = g.as_ref().and_then(|m| m.get(&key)) {
                return Ok(Arc::clone(v));
            }
        }
        // compile outside the lock (parallel requests do not block each other)
        let v = Arc::new(build()?);
        if let Ok(mut g) = self.map.lock() {
            let m = g.get_or_insert_with(HashMap::new);
            if m.len() >= MAX_ENTRIES {
                m.clear();
            }
            m.insert(key, Arc::clone(&v));
        }
        Ok(v)
    }

    /// Number of entries (tests/diagnostics).
    pub fn len(&self) -> usize {
        self.map.lock().ok().and_then(|g| g.as_ref().map(HashMap::len)).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Eq + Hash, V> Default for PatternCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_success_not_errors() {
        let c: PatternCache<(String, bool), String> = PatternCache::new();
        let mut builds = 0;
        let a = c.get_or_try::<()>(("x".into(), true), || {
            builds += 1;
            Ok("X".into())
        });
        assert_eq!(a.as_deref().map(String::as_str), Ok("X"));
        let b = c.get_or_try::<()>(("x".into(), true), || {
            builds += 1;
            Ok("Y".into())
        });
        assert_eq!(b.as_deref().map(String::as_str), Ok("X"));
        assert_eq!(builds, 1);
        let e = c.get_or_try::<&str>(("bad".into(), false), || Err("invalid"));
        assert_eq!(e.err(), Some("invalid"));
        assert_eq!(c.len(), 1);
        // different switch = different entry
        let _ = c.get_or_try::<()>(("x".into(), false), || Ok("x".into()));
        assert_eq!(c.len(), 2);
    }
}
