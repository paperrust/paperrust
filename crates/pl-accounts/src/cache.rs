//! Django's `default` cache: rate limiting, locks, session cache.
//!
//! upstream: django/core/cache/backends/base.py@5.2 (`default_key_func`: `<KEY_PREFIX>:1:<key>`),
//! Pickle encoding like `RedisSerializer` (protocol 5). Key format and encoding as in
//! `pl_auth_api::django_cache`; stored via `pl_core::cache` (process-local or shared in
//! PostgreSQL, `PAPERRUST_CACHE`).
//!
//! Gap S4: the allauth rate limits and the login throttle (`allauth:rl:*`) as well as the
//! TOTP replay lock (`allauth.mfa.totp.used*`) are **always** kept in the shared table
//! `paperrust_cache`, even with the process-local default cache – `pl_core::cache` does this
//! for all protected keys (`pl_core::cache::is_guarded_key`), also for `/api/token/`.

use pl_core::AppState;

pub use pl_auth_api::django_cache::{make_key, pickle_float_list, unpickle_float_list};

/// `pickle.dumps(True, 5)`.
pub const PICKLED_TRUE: &[u8] = b"\x80\x05\x88.";

/// Access to the Django cache (thin wrapper around [`AppState::cache`]).
#[derive(Clone)]
pub struct Cache {
    state: AppState,
}

/// Cache error (e.g. database unreachable with the shared cache → 500 as in Django).
#[derive(Debug)]
pub struct CacheError(pub String);

fn err(e: pl_core::ApiError) -> CacheError {
    CacheError(format!("{e:?}"))
}

impl Cache {
    /// Wrapper around `state.cache()` (backend from `PAPERRUST_CACHE`).
    pub fn new(state: &AppState) -> Self {
        Self { state: state.clone() }
    }

    /// Protected keys in process memory instead of the shared table (unit tests without DB).
    pub fn backend_only(state: &AppState) -> Self {
        state.keep_guarded_keys_local();
        Self { state: state.clone() }
    }

    /// `cache.get(key)` (raw bytes).
    pub async fn get(&self, k: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.state.cache().get(k).await.map_err(err)
    }

    /// `cache.set(key, value, timeout)`.
    pub async fn set(&self, k: &str, v: &[u8], timeout: u64) -> Result<(), CacheError> {
        self.state.cache().set(k, v, Some(timeout.max(1))).await.map_err(err)
    }

    /// `cache.add(key, value, timeout)` → newly set?
    pub async fn add(&self, k: &str, v: &[u8], timeout: u64) -> Result<bool, CacheError> {
        self.state.cache().add(k, v, Some(timeout.max(1))).await.map_err(err)
    }

    /// `cache.delete(key)`.
    pub async fn delete(&self, k: &str) -> Result<(), CacheError> {
        self.state.cache().delete(k).await.map(|_| ()).map_err(err)
    }
}
