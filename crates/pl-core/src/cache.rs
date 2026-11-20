//! Django's `default` cache – process-local (like `LocMemCache`) or shared in PostgreSQL.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`_CACHE_BACKEND`, `KEY_PREFIX`),
//! Django 5.2 `django/core/cache/backends/base.py` (`default_key_func` = `"%s:%s:%s" % (prefix,
//! version, key)` with version 1, `DEFAULT_TIMEOUT` = 300), `backends/locmem.py` (`LocMemCache`:
//! LRU via `OrderedDict`, `MAX_ENTRIES` = 300, `CULL_FREQUENCY` = 3, `_has_expired`, `_cull`),
//! `backends/db.py` (`DatabaseCache`: table with key, value, expiry).
//!
//! * [`Backend::Local`] (default): **one** process-wide [`LocalCache`] (all crates see the same
//!   entries, e.g. invalidation from pl-documents-write for read paths in pl-documents) with the
//!   semantics of `LocMemCache`. Exception (gap S4): locks/throttles (TOTP replay, `throttle_*`,
//!   `allauth:rl:*`, [`is_guarded_key`]) **always** live in the shared table
//!   `paperrust_cache` – as in ngx, where Redis is shared by all processes, so that separate
//!   HTTP/worker processes see the same locks. If the table is missing, it is created with the
//!   (idempotent) migration; if that is not possible (permissions), the keys stay in process memory
//!   with a warning in the log, there in an area without LRU eviction (see
//!   [`LocalCache`]).
//! * [`Backend::Postgres`] (`PAPERRUST_CACHE=postgres`): table `paperrust_cache` (UNLOGGED),
//!   shared by all processes of the same installation. Required in **multi-process operation** (HTTP
//!   process and worker separate, several instances behind a load balancer): otherwise the
//!   processes share neither TOTP replay locks nor login throttles nor cache invalidations. Replaces
//!   the former Redis cache since 2026-11-14 (`PAPERLESS_CACHE_BACKEND=…RedisCache` is read as
//!   `postgres`; Redis is never connected).
//!
//! Access: [`crate::AppState::cache`]. Keys always **without** prefix (like `cache.get("…")` in
//! Django); the prefix is set by [`DjangoCache`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::ApiError;

/// Former Redis cache (`django.core.cache.backends.redis.RedisCache`): is read as
/// [`Backend::Postgres`] (whoever set it wanted a shared cache).
pub const REDIS_CACHE_BACKEND: &str = "django.core.cache.backends.redis.RedisCache";
/// `django.core.cache.backends.db.DatabaseCache` → [`Backend::Postgres`].
pub const DATABASE_CACHE_BACKEND: &str = "django.core.cache.backends.db.DatabaseCache";
/// `django.core.cache.backends.locmem.LocMemCache`.
pub const LOCMEM_CACHE_BACKEND: &str = "django.core.cache.backends.locmem.LocMemCache";
/// `BaseCache.default_timeout` (`TIMEOUT` not set).
pub const DEFAULT_TIMEOUT: u64 = 300;
/// `LocMemCache`: `OPTIONS["MAX_ENTRIES"]` (default).
pub const LOCMEM_MAX_ENTRIES: usize = 300;
/// `LocMemCache`: `OPTIONS["CULL_FREQUENCY"]` (default).
pub const LOCMEM_CULL_FREQUENCY: usize = 3;
/// Key prefix (Django's `KEY_PREFIX`; formerly `PAPERLESS_REDIS_PREFIX`, now fixed empty).
pub const KEY_PREFIX: &str = "";

/// Where Django's `default` cache lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Process memory (default, one process).
    Local,
    /// Table `paperrust_cache` in PostgreSQL (multi-process operation).
    Postgres,
}

/// Backend from `PAPERRUST_CACHE` (`local` | `postgres`); if missing, from `PAPERLESS_CACHE_BACKEND`
/// (Redis or database cache → `postgres`, anything else → `local`). Default: `local`.
pub fn backend(cfg: &Config) -> Backend {
    if let Some(v) = cfg.env.get("PAPERRUST_CACHE") {
        return match v.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" | "db" | "database" | "shared" => Backend::Postgres,
            _ => Backend::Local,
        };
    }
    match cfg.env.get("PAPERLESS_CACHE_BACKEND") {
        Some(n) if n == REDIS_CACHE_BACKEND || n == DATABASE_CACHE_BACKEND => Backend::Postgres,
        _ => Backend::Local,
    }
}

/// Migration of the cache table (expand-only, idempotent; for `pl_cli::migrate`).
pub mod migrations {
    /// `paperrust_cache`: UNLOGGED (a cache does not have to survive a crash).
    pub const M0001_CACHE: &str = "CREATE UNLOGGED TABLE IF NOT EXISTS paperrust_cache (
    cache_key text PRIMARY KEY,
    value bytea NOT NULL,
    expires timestamptz NULL
);
CREATE INDEX IF NOT EXISTS paperrust_cache_expires ON paperrust_cache (expires) WHERE expires IS NOT NULL;
";
    /// All migrations `(name, sql)`.
    pub const ALL: &[(&str, &str)] = &[("pl_core.0001_paperrust_cache", M0001_CACHE)];
}

/// `default_key_func(key, KEY_PREFIX, VERSION=1)`.
pub fn make_key(prefix: &str, key: &str) -> String {
    format!("{prefix}:1:{key}")
}

// ------------------------------------------------------------------ LocMemCache

struct Entry {
    value: Vec<u8>,
    /// `None` = never expires (`timeout=None`).
    expires: Option<Instant>,
    /// Position in the LRU order (`OrderedDict.move_to_end`).
    seq: u64,
}

#[derive(Default)]
struct LocalInner {
    map: HashMap<String, Entry>,
    order: BTreeMap<u64, String>,
    next: u64,
}

impl LocalInner {
    fn has_expired(&self, key: &str, now: Instant) -> bool {
        match self.map.get(key) {
            None => true,
            Some(e) => e.expires.is_some_and(|x| x <= now),
        }
    }

    fn touch_lru(&mut self, key: &str) {
        let seq = self.next;
        self.next += 1;
        if let Some(e) = self.map.get_mut(key) {
            self.order.remove(&e.seq);
            e.seq = seq;
            self.order.insert(seq, key.to_string());
        }
    }

    fn remove(&mut self, key: &str) -> bool {
        match self.map.remove(key) {
            Some(e) => {
                self.order.remove(&e.seq);
                true
            }
            None => false,
        }
    }

    /// `_cull`: with `CULL_FREQUENCY == 0` everything, otherwise the oldest `len // CULL_FREQUENCY`.
    fn cull(&mut self, cull_frequency: usize) {
        if cull_frequency == 0 {
            self.map.clear();
            self.order.clear();
            return;
        }
        let count = self.map.len() / cull_frequency;
        let victims: Vec<String> = self.order.values().take(count).cloned().collect();
        for k in victims {
            self.remove(&k);
        }
    }

    /// `_set`.
    fn set(&mut self, key: &str, value: Vec<u8>, expires: Option<Instant>, max_entries: usize, cull_frequency: usize) {
        if self.map.len() >= max_entries && !self.map.contains_key(key) {
            self.cull(cull_frequency);
        }
        let seq = self.next;
        self.next += 1;
        if let Some(old) = self.map.insert(key.to_string(), Entry { value, expires, seq }) {
            self.order.remove(&old.seq);
        }
        self.order.insert(seq, key.to_string());
    }
}

// ------------------------------------------------------------------ Protected area (M1)

/// Key prefixes (without `KEY_PREFIX:1:`) whose entries are locks/throttles and therefore are
/// **not** subject to LRU eviction (security review final M1): TOTP replay lock
/// (allauth `_mark_code_used`), DRF throttles (`throttle_<scope>_<ident>`), allauth rate limits
/// including `cache_lock` (`allauth:rl:…`, `…:lock`).
pub const GUARDED_KEY_PREFIXES: &[&str] = &["allauth.mfa.totp.used", "throttle_", "allauth:rl:"];

/// Hard upper limit of the protected area. Entries are small (key + short pickle list,
/// roughly 200–400 bytes), so 100,000 entries are a few tens of MiB. On overflow it **throttles**
/// (fail-closed: `add`/`compare_and_set` return `false`, new `set`s are discarded), never
/// evicting an existing entry.
pub const GUARDED_MAX_ENTRIES: usize = 100_000;

/// Does the full Django key (`<prefix>:1:<key>`) belong to the protected area?
/// Without `:1:` (direct calls of [`LocalCache`]) the key itself is checked.
pub fn is_guarded_key(full_key: &str) -> bool {
    let guarded = |k: &str| GUARDED_KEY_PREFIXES.iter().any(|p| k.starts_with(p));
    guarded(full_key) || full_key.match_indices(":1:").any(|(i, _)| guarded(&full_key[i + 3..]))
}

struct GEntry {
    value: Vec<u8>,
    expires: Option<Instant>,
}

/// Protected area: no eviction, cleanup only by expiry (sorted by expiry time;
/// effort per call = number of actually expired entries, no full scan).
#[derive(Default)]
struct GuardedInner {
    map: HashMap<String, GEntry>,
    by_expiry: BTreeSet<(Instant, String)>,
}

impl GuardedInner {
    /// Remove expired entries (at the front of `by_expiry`).
    fn purge(&mut self, now: Instant) {
        while self.by_expiry.first().is_some_and(|(t, _)| *t <= now) {
            if let Some((t, k)) = self.by_expiry.pop_first()
                && self.map.get(&k).is_some_and(|e| e.expires == Some(t))
            {
                self.map.remove(&k);
            }
        }
    }

    fn live(&self, key: &str, now: Instant) -> Option<&GEntry> {
        self.map.get(key).filter(|e| !e.expires.is_some_and(|x| x <= now))
    }

    fn remove(&mut self, key: &str) -> bool {
        match self.map.remove(key) {
            Some(e) => {
                if let Some(t) = e.expires {
                    self.by_expiry.remove(&(t, key.to_string()));
                }
                true
            }
            None => false,
        }
    }

    /// Set; `false` = upper limit reached and key new (not stored).
    fn set(&mut self, key: &str, value: Vec<u8>, expires: Option<Instant>, max: usize) -> bool {
        let existed = self.remove(key);
        if !existed && self.map.len() >= max {
            return false;
        }
        if let Some(t) = expires {
            self.by_expiry.insert((t, key.to_string()));
        }
        self.map.insert(key.to_string(), GEntry { value, expires });
        true
    }

    fn set_expiry(&mut self, key: &str, expires: Option<Instant>) {
        if let Some(e) = self.map.get_mut(key) {
            let old = std::mem::replace(&mut e.expires, expires);
            if let Some(t) = old {
                self.by_expiry.remove(&(t, key.to_string()));
            }
            if let Some(t) = expires {
                self.by_expiry.insert((t, key.to_string()));
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    lru: LocalInner,
    guarded: GuardedInner,
}

/// Process-local cache with the semantics of Django's `LocMemCache` (key = full Django
/// key incl. prefix, values = raw bytes).
///
/// Deviation (security review final M1): keys from [`GUARDED_KEY_PREFIXES`] live in
/// a separate area without LRU eviction (expiry cleanup, hard upper limit
/// [`GUARDED_MAX_ENTRIES`], fail-closed on overflow). This way nobody with many new
/// keys (e.g. `X-Forwarded-For` variants at `/api/token/`) can evict other users' TOTP replay locks or
/// failed-attempt counters. All other entries: `LocMemCache` (LRU,
/// `MAX_ENTRIES`/`CULL_FREQUENCY`).
///
/// **Only one process:** the cache is process-local. If several PaperRust processes run (several
/// instances behind a load balancer, HTTP and worker separate), they share neither locks nor
/// throttles – then `PAPERRUST_CACHE=postgres` is mandatory.
pub struct LocalCache {
    inner: Mutex<Inner>,
    max_entries: usize,
    cull_frequency: usize,
    guarded_max: usize,
}

impl Default for LocalCache {
    fn default() -> Self {
        Self::new(LOCMEM_MAX_ENTRIES, LOCMEM_CULL_FREQUENCY)
    }
}

/// `get_backend_timeout`: `None` = never, `0` = immediately expired.
fn expiry(now: Instant, timeout: Option<u64>) -> Option<Instant> {
    timeout.map(|t| if t == 0 { now } else { now + Duration::from_secs(t) })
}

fn guarded_full(max: usize) {
    tracing::warn!(max, "local cache: guarded area (throttle/lock/TOTP keys) full – request throttled (fail-closed)");
}

impl LocalCache {
    pub fn new(max_entries: usize, cull_frequency: usize) -> Self {
        Self::with_guarded_max(max_entries, cull_frequency, GUARDED_MAX_ENTRIES)
    }

    /// Like [`new`](Self::new) with its own upper limit of the protected area (tests).
    pub fn with_guarded_max(max_entries: usize, cull_frequency: usize, guarded_max: usize) -> Self {
        Self { inner: Mutex::new(Inner::default()), max_entries, cull_frequency, guarded_max }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Locked; for protected keys, expired entries removed beforehand.
    fn lock_for(&self, key: &str, now: Instant) -> (std::sync::MutexGuard<'_, Inner>, bool) {
        let guarded = is_guarded_key(key);
        let mut g = self.lock();
        if guarded {
            g.guarded.purge(now);
        }
        (g, guarded)
    }

    /// `get`: expired → delete, otherwise move to the back of the LRU.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let now = Instant::now();
        let (mut g, guarded) = self.lock_for(key, now);
        if guarded {
            return g.guarded.live(key, now).map(|e| e.value.clone());
        }
        let g = &mut g.lru;
        if g.has_expired(key, now) {
            g.remove(key);
            return None;
        }
        g.touch_lru(key);
        g.map.get(key).map(|e| e.value.clone())
    }

    /// `set`. Protected area full and key new → not stored (warning).
    pub fn set(&self, key: &str, value: &[u8], timeout: Option<u64>) {
        let now = Instant::now();
        let (m, c, gm) = (self.max_entries, self.cull_frequency, self.guarded_max);
        let (mut g, guarded) = self.lock_for(key, now);
        if guarded {
            if !g.guarded.set(key, value.to_vec(), expiry(now, timeout), gm) {
                guarded_full(gm);
            }
            return;
        }
        g.lru.set(key, value.to_vec(), expiry(now, timeout), m, c);
    }

    /// `add`: only if absent or expired. Protected area full → `false`
    /// (fail-closed: TOTP code counts as used, lock as taken).
    pub fn add(&self, key: &str, value: &[u8], timeout: Option<u64>) -> bool {
        let now = Instant::now();
        let (m, c, gm) = (self.max_entries, self.cull_frequency, self.guarded_max);
        let (mut g, guarded) = self.lock_for(key, now);
        if guarded {
            if g.guarded.live(key, now).is_some() {
                return false;
            }
            let ok = g.guarded.set(key, value.to_vec(), expiry(now, timeout), gm);
            if !ok {
                guarded_full(gm);
            }
            return ok;
        }
        if g.lru.has_expired(key, now) {
            g.lru.set(key, value.to_vec(), expiry(now, timeout), m, c);
            return true;
        }
        false
    }

    /// `touch`.
    pub fn touch(&self, key: &str, timeout: Option<u64>) -> bool {
        let now = Instant::now();
        let (mut g, guarded) = self.lock_for(key, now);
        if guarded {
            if g.guarded.live(key, now).is_none() {
                return false;
            }
            g.guarded.set_expiry(key, expiry(now, timeout));
            return true;
        }
        if g.lru.has_expired(key, now) {
            return false;
        }
        if let Some(e) = g.lru.map.get_mut(key) {
            e.expires = expiry(now, timeout);
        }
        true
    }

    /// `delete` (like `LocMemCache._delete`: present – even expired – → `true`).
    pub fn delete(&self, key: &str) -> bool {
        let mut g = self.lock();
        if is_guarded_key(key) { g.guarded.remove(key) } else { g.lru.remove(key) }
    }

    /// `incr`: missing/expired → `None` (Django: `ValueError`). Values as digits (like
    /// `RedisSerializer` for `int`).
    pub fn incr(&self, key: &str, delta: i64) -> Option<Result<i64, String>> {
        let now = Instant::now();
        let (mut g, guarded) = self.lock_for(key, now);
        let cur = if guarded {
            g.guarded.live(key, now)?.value.clone()
        } else {
            if g.lru.has_expired(key, now) {
                return None;
            }
            g.lru.map.get(key).map(|e| e.value.clone()).unwrap_or_default()
        };
        let parsed = std::str::from_utf8(&cur).ok().and_then(|s| s.trim().parse::<i64>().ok());
        let Some(v) = parsed else {
            return Some(Err("value is not an integer".into()));
        };
        let Some(n) = v.checked_add(delta) else {
            return Some(Err("integer overflow".into()));
        };
        let bytes = n.to_string().into_bytes();
        if guarded {
            if let Some(e) = g.guarded.map.get_mut(key) {
                e.value = bytes;
            }
        } else {
            if let Some(e) = g.lru.map.get_mut(key) {
                e.value = bytes;
            }
            g.lru.touch_lru(key);
        }
        Some(Ok(n))
    }

    /// Compare and set in one step (`expected = None`: key must not exist).
    /// Protected area full and key new → `false` (callers throttle).
    pub fn compare_and_set(&self, key: &str, expected: Option<&[u8]>, new: &[u8], timeout: Option<u64>) -> bool {
        let now = Instant::now();
        let (m, c, gm) = (self.max_entries, self.cull_frequency, self.guarded_max);
        let (mut g, guarded) = self.lock_for(key, now);
        if guarded {
            if g.guarded.live(key, now).map(|e| e.value.as_slice()) != expected {
                return false;
            }
            let ok = g.guarded.set(key, new.to_vec(), expiry(now, timeout), gm);
            if !ok {
                guarded_full(gm);
            }
            return ok;
        }
        let cur = if g.lru.has_expired(key, now) { None } else { g.lru.map.get(key).map(|e| e.value.as_slice()) };
        if cur != expected {
            return false;
        }
        g.lru.set(key, new.to_vec(), expiry(now, timeout), m, c);
        true
    }

    /// Number of entries (both areas; tests).
    pub fn len(&self) -> usize {
        let g = self.lock();
        g.lru.map.len() + g.guarded.map.len()
    }

    /// Number of entries in the protected area (tests/diagnostics).
    pub fn guarded_len(&self) -> usize {
        self.lock().guarded.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear everything (`clear`).
    pub fn clear(&self) {
        let mut g = self.lock();
        g.lru.map.clear();
        g.lru.order.clear();
        g.guarded.map.clear();
        g.guarded.by_expiry.clear();
    }
}

// ------------------------------------------------------------------ DjangoCache

/// Delete expired rows of `paperrust_cache` after this many write accesses per process.
const PG_PURGE_EVERY: u64 = 256;
static PG_WRITES: AtomicU64 = AtomicU64::new(0);

/// Expiry for parameter `$3`: `NULL` = never, otherwise `now() + timeout` (seconds).
const PG_EXPIRES: &str = "CASE WHEN $3::bigint IS NULL THEN NULL ELSE now() + make_interval(secs => $3::bigint) END";
/// Like [`PG_EXPIRES`] for parameter `$2`.
const PG_EXPIRES_2: &str = "CASE WHEN $2::bigint IS NULL THEN NULL ELSE now() + make_interval(secs => $2::bigint) END";
const PG_LIVE: &str = "(expires IS NULL OR expires > now())";

fn pg_timeout(timeout: Option<u64>) -> Option<i64> {
    // make_interval(secs) is double precision; limit very large values (~100 years)
    timeout.map(|t| i64::try_from(t).unwrap_or(i64::MAX).min(3_153_600_000))
}

/// State of the shared table for protected keys with the local backend (gap S4).
#[derive(Debug, Default)]
pub struct GuardTable(AtomicU8);

impl GuardTable {
    const SHARED: u8 = 1;
    const LOCAL: u8 = 2;

    /// Keep protected keys in process memory (table not usable, unit tests without DB).
    pub fn set_local(&self) {
        self.0.store(Self::LOCAL, Ordering::Relaxed);
    }
}

/// Django's `default` cache (`from django.core.cache import cache`), via [`crate::AppState::cache`].
#[derive(Clone, Copy)]
pub struct DjangoCache<'a> {
    pub(crate) backend: Backend,
    pub(crate) db: &'a PgPool,
    pub(crate) local: &'a LocalCache,
    pub(crate) guard: &'a GuardTable,
}

impl<'a> DjangoCache<'a> {
    /// Which backend applies.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// `true` for [`Backend::Postgres`] (all processes see the same entries).
    pub fn is_shared(&self) -> bool {
        self.backend == Backend::Postgres
    }

    /// Full key (`make_key`).
    pub fn key(&self, key: &str) -> String {
        make_key(KEY_PREFIX, key)
    }

    /// The process-local cache (only in use for [`Backend::Local`]).
    pub fn local(&self) -> &'a LocalCache {
        self.local
    }

    /// Backend for a full key: protected keys ([`is_guarded_key`]) go to `paperrust_cache` even with the
    /// local backend (S4). The table is checked on first access and created if
    /// needed; if that is not possible, they stay local (warning in the log). A
    /// connection error during the check is an error (like any other cache access to the DB).
    async fn backend_for(&self, full: &str) -> Result<Backend, ApiError> {
        if self.backend == Backend::Postgres || !is_guarded_key(full) {
            return Ok(self.backend);
        }
        match self.guard.0.load(Ordering::Relaxed) {
            GuardTable::SHARED => return Ok(Backend::Postgres),
            GuardTable::LOCAL => return Ok(Backend::Local),
            _ => {}
        }
        let exists: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_cache') IS NOT NULL").fetch_one(self.db).await?;
        if !exists {
            for (name, sql) in migrations::ALL {
                if let Err(e) = sqlx::raw_sql(sql).execute(self.db).await {
                    tracing::warn!(
                        migration = %name,
                        error = %e,
                        "cache: login throttling and TOTP replay locks stay per process, the shared table paperrust_cache could not be created"
                    );
                    self.guard.0.store(GuardTable::LOCAL, Ordering::Relaxed);
                    return Ok(Backend::Local);
                }
            }
        }
        self.guard.0.store(GuardTable::SHARED, Ordering::Relaxed);
        Ok(Backend::Postgres)
    }

    /// Split full keys by backend (local, PostgreSQL).
    async fn split_backend<'k>(&self, fulls: &'k [String]) -> Result<(Vec<&'k String>, Vec<&'k String>), ApiError> {
        let (mut local, mut pg) = (Vec::new(), Vec::new());
        for k in fulls {
            match self.backend_for(k).await? {
                Backend::Local => local.push(k),
                Backend::Postgres => pg.push(k),
            }
        }
        Ok((local, pg))
    }

    /// Occasionally delete expired rows (only log errors).
    async fn pg_maybe_purge(&self) {
        if !PG_WRITES.fetch_add(1, Ordering::Relaxed).is_multiple_of(PG_PURGE_EVERY) {
            return;
        }
        if let Err(e) = sqlx::query("DELETE FROM paperrust_cache WHERE expires IS NOT NULL AND expires <= now()").execute(self.db).await {
            tracing::warn!(error = %e, "cache: purging expired rows failed");
        }
    }

    /// `cache.get(key)` → raw bytes.
    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let full = self.key(key);
        match self.backend_for(&full).await? {
            Backend::Local => Ok(self.local.get(&full)),
            Backend::Postgres => Ok(sqlx::query_scalar(&format!("SELECT value FROM paperrust_cache WHERE cache_key = $1 AND {PG_LIVE}"))
                .bind(&full)
                .fetch_optional(self.db)
                .await?),
        }
    }

    /// `cache.get_many(keys)` → only existing keys (order as in `keys`).
    pub async fn get_many(&self, keys: &[&str]) -> Result<Vec<(String, Vec<u8>)>, ApiError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let fulls: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        let (local, pg) = self.split_backend(&fulls).await?;
        let mut found: HashMap<String, Vec<u8>> = HashMap::with_capacity(fulls.len());
        for k in local {
            if let Some(v) = self.local.get(k) {
                found.insert(k.clone(), v);
            }
        }
        if !pg.is_empty() {
            let rows = sqlx::query(&format!("SELECT cache_key, value FROM paperrust_cache WHERE cache_key = ANY($1) AND {PG_LIVE}"))
                .bind(&pg)
                .fetch_all(self.db)
                .await?;
            for r in rows {
                found.insert(r.try_get("cache_key")?, r.try_get("value")?);
            }
        }
        let vals: Vec<Option<Vec<u8>>> = fulls.iter().map(|k| found.remove(k)).collect();
        Ok(keys.iter().zip(vals).filter_map(|(k, v)| v.map(|v| (k.to_string(), v))).collect())
    }

    /// `cache.set(key, value, timeout)`; `timeout = None` = never expire, `Some(0)` = delete.
    pub async fn set(&self, key: &str, value: &[u8], timeout: Option<u64>) -> Result<(), ApiError> {
        let full = self.key(key);
        match self.backend_for(&full).await? {
            Backend::Local => {
                self.local.set(&full, value, timeout);
                Ok(())
            }
            Backend::Postgres => {
                if timeout == Some(0) {
                    sqlx::query("DELETE FROM paperrust_cache WHERE cache_key = $1").bind(&full).execute(self.db).await?;
                    return Ok(());
                }
                sqlx::query(&format!(
                    "INSERT INTO paperrust_cache (cache_key, value, expires) VALUES ($1, $2, {PG_EXPIRES}) \
                     ON CONFLICT (cache_key) DO UPDATE SET value = EXCLUDED.value, expires = EXCLUDED.expires"
                ))
                .bind(&full)
                .bind(value)
                .bind(pg_timeout(timeout))
                .execute(self.db)
                .await?;
                self.pg_maybe_purge().await;
                Ok(())
            }
        }
    }

    /// `cache.add(key, value, timeout)` → newly set? (An expired entry counts as missing.)
    pub async fn add(&self, key: &str, value: &[u8], timeout: Option<u64>) -> Result<bool, ApiError> {
        let full = self.key(key);
        match self.backend_for(&full).await? {
            Backend::Local => Ok(self.local.add(&full, value, timeout)),
            Backend::Postgres => {
                if timeout == Some(0) {
                    // set and immediately expired: only "would it have been new?"
                    let live: bool =
                        sqlx::query_scalar(&format!("SELECT EXISTS (SELECT 1 FROM paperrust_cache WHERE cache_key = $1 AND {PG_LIVE})"))
                            .bind(&full)
                            .fetch_one(self.db)
                            .await?;
                    return Ok(!live);
                }
                let added = sqlx::query(&format!(
                    "INSERT INTO paperrust_cache (cache_key, value, expires) VALUES ($1, $2, {PG_EXPIRES}) \
                     ON CONFLICT (cache_key) DO UPDATE SET value = EXCLUDED.value, expires = EXCLUDED.expires \
                     WHERE paperrust_cache.expires IS NOT NULL AND paperrust_cache.expires <= now() RETURNING 1"
                ))
                .bind(&full)
                .bind(value)
                .bind(pg_timeout(timeout))
                .fetch_optional(self.db)
                .await?
                .is_some();
                self.pg_maybe_purge().await;
                Ok(added)
            }
        }
    }

    /// `cache.touch(key, timeout)` (`None` = never expire).
    pub async fn touch(&self, key: &str, timeout: Option<u64>) -> Result<bool, ApiError> {
        let full = self.key(key);
        match self.backend_for(&full).await? {
            Backend::Local => Ok(self.local.touch(&full, timeout)),
            Backend::Postgres => Ok(sqlx::query(&format!(
                "UPDATE paperrust_cache SET expires = {PG_EXPIRES_2} WHERE cache_key = $1 AND {PG_LIVE} RETURNING 1"
            ))
            .bind(&full)
            .bind(pg_timeout(timeout))
            .fetch_optional(self.db)
            .await?
            .is_some()),
        }
    }

    /// `cache.delete(key)` → was it present?
    pub async fn delete(&self, key: &str) -> Result<bool, ApiError> {
        Ok(self.delete_many(&[key]).await? > 0)
    }

    /// `cache.delete_many(keys)` → number of deleted (non-expired) keys.
    pub async fn delete_many(&self, keys: &[&str]) -> Result<usize, ApiError> {
        if keys.is_empty() {
            return Ok(0);
        }
        let fulls: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        let (local, pg) = self.split_backend(&fulls).await?;
        let mut n = local.into_iter().filter(|k| self.local.delete(k)).count();
        if !pg.is_empty() {
            let live: Vec<bool> = sqlx::query_scalar(&format!("DELETE FROM paperrust_cache WHERE cache_key = ANY($1) RETURNING {PG_LIVE}"))
                .bind(&pg)
                .fetch_all(self.db)
                .await?;
            n += live.into_iter().filter(|l| *l).count();
        }
        Ok(n)
    }

    /// `cache.incr(key, delta)`; `Ok(None)` if the key is missing (Django: `ValueError`).
    pub async fn incr(&self, key: &str, delta: i64) -> Result<Option<i64>, ApiError> {
        let full = self.key(key);
        match self.backend_for(&full).await? {
            Backend::Local => match self.local.incr(&full, delta) {
                None => Ok(None),
                Some(Ok(n)) => Ok(Some(n)),
                Some(Err(e)) => Err(ApiError::internal(format!("cache incr: {e}"))),
            },
            Backend::Postgres => {
                let mut tx = self.db.begin().await?;
                let cur: Option<Vec<u8>> =
                    sqlx::query_scalar(&format!("SELECT value FROM paperrust_cache WHERE cache_key = $1 AND {PG_LIVE} FOR UPDATE"))
                        .bind(&full)
                        .fetch_optional(&mut *tx)
                        .await?;
                let Some(cur) = cur else {
                    return Ok(None);
                };
                let n: i64 = std::str::from_utf8(&cur)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .ok_or_else(|| ApiError::internal("cache incr: value is not an integer"))?;
                let n = n.checked_add(delta).ok_or_else(|| ApiError::internal("cache incr: overflow"))?;
                sqlx::query("UPDATE paperrust_cache SET value = $2 WHERE cache_key = $1")
                    .bind(&full)
                    .bind(n.to_string().into_bytes())
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(Some(n))
            }
        }
    }

    /// Compare and set (atomic). `expected = None` = key must not exist.
    /// `timeout`: like [`set`](Self::set), but `Some(0)`/`None` = without expiry.
    pub async fn compare_and_set(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        new: &[u8],
        timeout: Option<u64>,
    ) -> Result<bool, ApiError> {
        let full = self.key(key);
        let timeout = timeout.filter(|t| *t > 0);
        match self.backend_for(&full).await? {
            Backend::Local => Ok(self.local.compare_and_set(&full, expected, new, timeout)),
            Backend::Postgres => {
                let mut tx = self.db.begin().await?;
                let row = sqlx::query(&format!("SELECT value, {PG_LIVE} AS live FROM paperrust_cache WHERE cache_key = $1 FOR UPDATE"))
                    .bind(&full)
                    .fetch_optional(&mut *tx)
                    .await?;
                let ok = match row {
                    // no row: insert; if a parallel inserter wins, it is a conflict
                    None if expected.is_none() => sqlx::query(&format!(
                        "INSERT INTO paperrust_cache (cache_key, value, expires) VALUES ($1, $2, {PG_EXPIRES}) \
                         ON CONFLICT (cache_key) DO NOTHING RETURNING 1"
                    ))
                    .bind(&full)
                    .bind(new)
                    .bind(pg_timeout(timeout))
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some(),
                    None => false,
                    Some(r) => {
                        let live: bool = r.try_get("live")?;
                        let cur: Vec<u8> = r.try_get("value")?;
                        let cur = live.then_some(cur.as_slice());
                        if cur == expected {
                            sqlx::query(&format!("UPDATE paperrust_cache SET value = $2, expires = {PG_EXPIRES} WHERE cache_key = $1"))
                                .bind(&full)
                                .bind(new)
                                .bind(pg_timeout(timeout))
                                .execute(&mut *tx)
                                .await?;
                            true
                        } else {
                            false
                        }
                    }
                };
                tx.commit().await?;
                if ok {
                    self.pg_maybe_purge().await;
                }
                Ok(ok)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let mut v: Vec<(&str, &str)> = vec![("PAPERLESS_SECRET_KEY", "test-only-0123456789abcdefghijklmnopqrstuvwxyzABCDEFG")];
        v.extend_from_slice(pairs);
        Config::from_env_map(crate::config::Env::from_pairs(v)).unwrap()
    }

    #[test]
    fn backend_selection() {
        assert_eq!(backend(&cfg(&[])), Backend::Local);
        assert_eq!(backend(&cfg(&[("PAPERLESS_DEBUG", "true")])), Backend::Local);
        assert_eq!(backend(&cfg(&[("PAPERRUST_CACHE", "postgres")])), Backend::Postgres);
        assert_eq!(backend(&cfg(&[("PAPERRUST_CACHE", " Local ")])), Backend::Local);
        // former Redis cache → shared cache in PostgreSQL (never Redis)
        assert_eq!(backend(&cfg(&[("PAPERLESS_CACHE_BACKEND", REDIS_CACHE_BACKEND)])), Backend::Postgres);
        assert_eq!(backend(&cfg(&[("PAPERLESS_CACHE_BACKEND", DATABASE_CACHE_BACKEND)])), Backend::Postgres);
        assert_eq!(backend(&cfg(&[("PAPERLESS_CACHE_BACKEND", LOCMEM_CACHE_BACKEND)])), Backend::Local);
        assert_eq!(backend(&cfg(&[("PAPERLESS_CACHE_BACKEND", "django.core.cache.backends.dummy.DummyCache")])), Backend::Local);
        // PAPERRUST_CACHE wins
        assert_eq!(backend(&cfg(&[("PAPERRUST_CACHE", "local"), ("PAPERLESS_CACHE_BACKEND", REDIS_CACHE_BACKEND)])), Backend::Local);
    }

    #[test]
    fn key_format() {
        assert_eq!(make_key("", "treenode_list"), ":1:treenode_list");
        assert_eq!(make_key("pl", "doc_3_suggest"), "pl:1:doc_3_suggest");
    }

    #[test]
    fn locmem_semantics() {
        let c = LocalCache::default();
        assert_eq!(c.get("a"), None);
        c.set("a", b"1", Some(60));
        assert_eq!(c.get("a").as_deref(), Some(&b"1"[..]));
        assert!(!c.add("a", b"2", Some(60)));
        assert!(c.add("b", b"2", None));
        assert_eq!(c.incr("a", 4), Some(Ok(5)));
        assert_eq!(c.get("a").as_deref(), Some(&b"5"[..]));
        assert_eq!(c.incr("zz", 1), None);
        assert!(c.touch("a", Some(120)));
        assert!(!c.touch("zz", Some(1)));
        assert!(c.delete("a"));
        assert!(!c.delete("a"));
        // timeout 0: expired immediately
        c.set("z", b"x", Some(0));
        assert_eq!(c.get("z"), None);
        assert!(c.add("z", b"y", Some(30)));
        // compare-and-set
        assert!(!c.compare_and_set("cas", Some(b"x"), b"1", Some(10)));
        assert!(c.compare_and_set("cas", None, b"1", Some(10)));
        assert!(!c.compare_and_set("cas", None, b"2", Some(10)));
        assert!(c.compare_and_set("cas", Some(b"1"), b"2", Some(10)));
        assert_eq!(c.get("cas").as_deref(), Some(&b"2"[..]));
    }

    #[test]
    fn locmem_cull_lru() {
        let c = LocalCache::new(6, 3);
        for i in 0..6 {
            c.set(&format!("k{i}"), b"v", None);
        }
        // k0 used last → stays when thinning out
        assert!(c.get("k0").is_some());
        c.set("k6", b"v", None);
        // 6 // 3 = 2 oldest (k1, k2) removed
        assert_eq!(c.len(), 5);
        assert!(c.get("k1").is_none() && c.get("k2").is_none());
        assert!(c.get("k0").is_some() && c.get("k6").is_some());
        // Overwriting an existing key does not thin out
        let d = LocalCache::new(2, 3);
        d.set("a", b"1", None);
        d.set("b", b"1", None);
        d.set("a", b"2", None);
        assert_eq!(d.len(), 2);
        // CULL_FREQUENCY 0 = clear everything
        let e = LocalCache::new(2, 0);
        e.set("a", b"1", None);
        e.set("b", b"1", None);
        e.set("c", b"1", None);
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn guarded_key_classification() {
        assert!(is_guarded_key("p:1:allauth.mfa.totp.used?user=1&code=123456"));
        assert!(is_guarded_key(":1:throttle_login_1.2.3.4"));
        assert!(is_guarded_key("x:y:1:allauth:rl:allauth.account:login_failed:abc:lock"));
        assert!(is_guarded_key("throttle_login_x"));
        assert!(!is_guarded_key("p:1:doc_1_metadata"));
        assert!(!is_guarded_key("p:1:treenode_list"));
        assert!(!is_guarded_key("throttle"));
    }

    /// M1: an eviction attack with many new keys (LRU area and throttle keys
    /// per `X-Forwarded-For`) deletes neither TOTP replay locks nor failed-attempt counters.
    #[test]
    fn eviction_attack_keeps_locks() {
        let c = LocalCache::default();
        let totp = "pfx:1:allauth.mfa.totp.used?user=7&code=123456";
        let failed = "pfx:1:allauth:rl:allauth.account:login_failed:deadbeef";
        assert!(c.add(totp, b"y", Some(30)));
        c.set(failed, b"hist", Some(300));
        assert!(c.compare_and_set("pfx:1:throttle_login_victim", None, b"h", Some(60)));
        // 10,000 foreign throttle keys and 10,000 normal entries
        for i in 0..10_000 {
            assert!(c.compare_and_set(&format!("pfx:1:throttle_login_10.0.{}.{}", i / 256, i % 256), None, b"h", Some(60)));
            c.set(&format!("pfx:1:doc_{i}_metadata"), b"m", Some(300));
        }
        assert!(!c.add(totp, b"y", Some(30)), "TOTP-Replay-Sperre verdrängt");
        assert_eq!(c.get(failed).as_deref(), Some(&b"hist"[..]));
        assert!(c.get("pfx:1:throttle_login_victim").is_some());
        // the LRU area stays LocMem-limited
        assert!(c.len() - c.guarded_len() <= LOCMEM_MAX_ENTRIES);
        assert_eq!(c.guarded_len(), 10_003);
    }

    /// Upper limit: with the protected area full it throttles (fail-closed), existing entries stay;
    /// expired entries are cleaned up and free space.
    #[test]
    fn guarded_area_full_fails_closed() {
        let c = LocalCache::with_guarded_max(300, 3, 3);
        assert!(c.add("throttle_a", b"1", Some(60)));
        assert!(c.add("throttle_b", b"1", Some(60)));
        assert!(c.add("allauth.mfa.totp.used?user=1&code=1", b"y", Some(1)));
        // full: new key → not set (TOTP counts as used, throttle applies)
        assert!(!c.add("allauth.mfa.totp.used?user=2&code=2", b"y", Some(30)));
        assert!(!c.compare_and_set("throttle_new", None, b"h", Some(60)));
        c.set("allauth:rl:x", b"h", Some(60));
        assert!(c.get("allauth:rl:x").is_none());
        // Existing entries may still be changed
        assert!(c.compare_and_set("throttle_a", Some(b"1"), b"2", Some(60)));
        assert_eq!(c.incr("throttle_b", 2), Some(Ok(3)));
        assert!(c.touch("throttle_b", Some(120)));
        // normal entries are unaffected by this
        c.set("doc_1_metadata", b"m", Some(60));
        assert!(c.get("doc_1_metadata").is_some());
        // Expiry frees space
        std::thread::sleep(Duration::from_millis(1100));
        assert!(c.add("allauth.mfa.totp.used?user=2&code=2", b"y", Some(30)));
        assert_eq!(c.guarded_len(), 3);
        // delete/clear
        assert!(c.delete("throttle_a"));
        assert!(!c.delete("throttle_a"));
        c.clear();
        assert!(c.is_empty());
    }

    /// Local backend never touches the database for normal keys (done immediately even with an unreachable
    /// address); protected keys only if they are meant to stay local (unit tests).
    #[tokio::test]
    async fn local_backend_never_connects() {
        let config = cfg(&[("PAPERLESS_REDIS", "redis://10.255.255.1:6379"), ("PAPERLESS_REDIS_PREFIX", "t")]);
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        let st = crate::AppState::with_pool(config, pool).unwrap();
        st.keep_guarded_keys_local();
        let t0 = Instant::now();
        let c = st.cache();
        assert_eq!(c.backend(), Backend::Local);
        c.set("doc_1_metadata", b"m", Some(300)).await.unwrap();
        assert_eq!(c.get("doc_1_metadata").await.unwrap().as_deref(), Some(&b"m"[..]));
        // PAPERLESS_REDIS_PREFIX has no effect
        assert_eq!(c.local().get(":1:doc_1_metadata").as_deref(), Some(&b"m"[..]));
        assert_eq!(c.delete_many(&["doc_1_metadata", "doc_1_suggest"]).await.unwrap(), 1);
        assert!(c.add("allauth.mfa.totp.used?user=1&code=1", b"y", Some(30)).await.unwrap());
        assert!(!c.add("allauth.mfa.totp.used?user=1&code=1", b"y", Some(30)).await.unwrap());
        assert!(t0.elapsed() < Duration::from_millis(500));
        // the local cache is one per AppState (all callers see the same entries)
        assert!(st.cache().get("allauth.mfa.totp.used?user=1&code=1").await.unwrap().is_some());
    }

    /// S4: with the local backend protected keys go to the shared table – so without a reachable
    /// database an error instead of a silent entry in process memory; normal keys
    /// stay local.
    #[tokio::test]
    async fn local_backend_guarded_keys_use_shared_table() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy("postgres://u:p@127.0.0.1:1/x")
            .unwrap();
        let st = crate::AppState::with_pool(cfg(&[]), pool).unwrap();
        let c = st.cache();
        assert_eq!(c.backend(), Backend::Local);
        c.set("doc_1_metadata", b"m", Some(300)).await.unwrap();
        assert!(c.add("allauth.mfa.totp.used?user=1&code=1", b"y", Some(30)).await.is_err());
        assert!(c.get("throttle_login_1.2.3.4").await.is_err());
        assert!(c.local().get(":1:allauth.mfa.totp.used?user=1&code=1").is_none());
        // mixed sets: error as soon as a protected key is included
        assert!(c.delete_many(&["doc_1_metadata", "throttle_x"]).await.is_err());
    }

    /// PostgreSQL backend: same semantics as the local cache. Own throwaway DB:
    /// `PL_TEST_DB=postgres://…/pl_cache_test cargo test -p pl-core -- --ignored pg_backend`.
    #[tokio::test]
    #[ignore = "braucht eine Wegwerf-PostgreSQL-DB (PL_TEST_DB)"]
    async fn pg_backend_roundtrip() {
        let url = std::env::var("PL_TEST_DB").expect("PL_TEST_DB");
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        for (_, sql) in migrations::ALL {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        sqlx::query("DELETE FROM paperrust_cache").execute(&pool).await.unwrap();
        let st = crate::AppState::with_pool(cfg(&[("PAPERRUST_CACHE", "postgres")]), pool.clone()).unwrap();
        let c = st.cache();
        assert_eq!(c.backend(), Backend::Postgres);
        assert!(c.is_shared());
        c.set("a", b"\x80\x05K\x01.", Some(30)).await.unwrap();
        let raw: Vec<u8> = sqlx::query_scalar("SELECT value FROM paperrust_cache WHERE cache_key = ':1:a'").fetch_one(&pool).await.unwrap();
        assert_eq!(raw, b"\x80\x05K\x01.");
        assert_eq!(c.get("a").await.unwrap().as_deref(), Some(&b"\x80\x05K\x01."[..]));
        assert!(!c.add("a", b"x", Some(30)).await.unwrap());
        assert!(c.add("n", b"5", Some(30)).await.unwrap());
        assert_eq!(c.incr("n", 2).await.unwrap(), Some(7));
        assert_eq!(c.incr("missing", 1).await.unwrap(), None);
        assert!(c.touch("n", Some(60)).await.unwrap());
        assert!(!c.touch("missing", Some(60)).await.unwrap());
        assert_eq!(c.get_many(&["a", "missing", "n"]).await.unwrap().len(), 2);
        assert!(c.compare_and_set("cas", None, b"1", Some(30)).await.unwrap());
        assert!(!c.compare_and_set("cas", None, b"1", Some(30)).await.unwrap());
        assert!(!c.compare_and_set("cas", Some(b"0"), b"2", Some(30)).await.unwrap());
        assert!(c.compare_and_set("cas", Some(b"1"), b"2", Some(30)).await.unwrap());
        assert_eq!(c.get("cas").await.unwrap().as_deref(), Some(&b"2"[..]));
        // timeout 0: delete
        c.set("a", b"x", Some(0)).await.unwrap();
        assert_eq!(c.get("a").await.unwrap(), None);
        assert!(c.add("z", b"y", Some(0)).await.unwrap());
        assert_eq!(c.get("z").await.unwrap(), None);
        // expired counts as missing (delete does not count it, CAS sees None)
        sqlx::query("INSERT INTO paperrust_cache VALUES (':1:old', 'v', now() - interval '1 second')").execute(&pool).await.unwrap();
        assert_eq!(c.get("old").await.unwrap(), None);
        assert!(!c.delete("old").await.unwrap());
        sqlx::query("INSERT INTO paperrust_cache VALUES (':1:old', 'v', now() - interval '1 second')").execute(&pool).await.unwrap();
        assert!(c.compare_and_set("old", None, b"n", None).await.unwrap());
        assert!(c.add("throttle_x", b"h", None).await.unwrap());
        assert_eq!(c.delete_many(&["n", "cas", "nope", "old", "throttle_x"]).await.unwrap(), 4);
        // a second state (other process) sees the same entries
        let st2 = crate::AppState::with_pool(cfg(&[("PAPERRUST_CACHE", "postgres")]), pool.clone()).unwrap();
        c.set("shared", b"1", None).await.unwrap();
        assert_eq!(st2.cache().get("shared").await.unwrap().as_deref(), Some(&b"1"[..]));
        sqlx::query("DROP TABLE paperrust_cache").execute(&pool).await.unwrap();
    }
}
