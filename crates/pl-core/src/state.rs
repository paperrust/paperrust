//! Shared application state.
//!
//! * PostgreSQL pool (sqlx): `test_before_acquire(false)`, ping only for connections that
//!   were idle ≥ 30 s (corresponds to Django's `CONN_HEALTH_CHECKS=True`, but without a ping per request).
//!   The pool connects **lazily** (start without a DB possible, like Django).
//! * Django's `default` cache: [`AppState::cache`] ([`crate::cache`]) – process-local or in
//!   PostgreSQL (multi-process operation). Redis no longer exists.
//! * `reqwest::Client` without following redirects and without decompression.
//! * [`Services`]: services of higher crates plugged in after construction (task queue
//!   `Arc<dyn pl_queue::TaskQueue>`, `pl_queue::EventBus`). pl-queue depends on pl-core, hence
//!   typed via `TypeId` instead of a fixed field.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono_tz::Tz;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::{Connection, PgPool};

use crate::Config;
use crate::cache::{Backend as CacheBackend, DjangoCache, GuardTable, LocalCache};
use crate::config::{ConfigError, DbConfig, DbEngine};

/// Error while building the state.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unsupported database engine {0:?} (PaperRust phase 1 supports PostgreSQL only)")]
    UnsupportedEngine(DbEngine),
    #[error("http client: {0}")]
    Http(String),
}

/// Idle time after which a pool connection is pinged before being handed out.
pub const PING_IF_IDLE_FOR: Duration = Duration::from_secs(30);

/// Idle time after which a free pool connection is closed (main and secondary pools).
///
/// Every open connection is its own PostgreSQL process that keeps catalog/plan caches after work,
/// prepared statements (sqlx: up to 100 per connection) and sort/hash buffers – on the
/// Jetson together ~3× as much PostgreSQL RAM as paperless-ngx after ingestion (147 instead of 49 MiB,
/// benchmark 2026-11-18). Django closes its connection after every request (`CONN_MAX_AGE=0`).
/// Until 2026-11-18 it was 600 s; with 60 s the connections opened during ingestion are dropped after
/// one minute of idle time. Cost: after a pause of > 60 s one connection setup (a few ms).
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum age of a pool connection (sqlx default 30 min): sqlx hands out free connections in
/// order (FIFO). If a loop polls every second (queue, search index sync), every
/// connection left open after ingestion gets its turn every few seconds and never reaches
/// [`POOL_IDLE_TIMEOUT`]. With a 5 min maximum age every connection is closed on return after
/// 5 min at the latest; afterwards only as many new ones are created as are needed at the same time.
/// Cost under continuous load: one rebuild per connection every 5 min (a few ms).
pub const POOL_MAX_LIFETIME: Duration = Duration::from_secs(300);

/// Shared state for all handlers (`Clone` is cheap, everything sits behind an `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<AppStateInner>,
}

/// Content of [`AppState`]. Fields are publicly readable; it is only created via
/// [`AppState::new`] / [`AppState::from_env`] / [`AppState::with_pool`].
#[non_exhaustive]
pub struct AppStateInner {
    /// PostgreSQL-Pool.
    pub db: PgPool,
    /// Configuration.
    pub config: Config,
    /// `PAPERLESS_TIME_ZONE`.
    pub tz: Tz,
    /// HTTP client (outgoing requests).
    pub http: reqwest::Client,
    /// Backend of Django's `default` cache (`PAPERRUST_CACHE`).
    pub cache_backend: CacheBackend,
    /// Process-local cache (for [`CacheBackend::Local`]), one per state.
    pub local_cache: LocalCache,
    /// Protection keys with the local backend in `paperrust_cache` (S4): state of the table.
    pub guard_table: GuardTable,
    /// Plugged-in services (queue, event bus), see [`Services`].
    pub services: Services,
}

/// Services that only come into existence after the state is built and are defined in higher crates
/// (e.g. `Arc<dyn pl_queue::TaskQueue>`, `pl_queue::EventBus`). The key is the type; each type
/// at most once. Set at startup (pl-server), read in handlers:
///
/// ```ignore
/// let queue: Option<Arc<dyn pl_queue::TaskQueue>> = state.service();
/// ```
#[derive(Default)]
pub struct Services {
    map: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Services {
    /// Set a service (replaces an existing one of the same type).
    pub fn insert<T: Any + Send + Sync>(&self, value: T) {
        let mut m = match self.map.write() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        m.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// Read a service (copy; services are cheap to clone, e.g. `Arc<…>`).
    pub fn get<T: Any + Send + Sync + Clone>(&self) -> Option<T> {
        let m = match self.map.read() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        m.get(&TypeId::of::<T>()).and_then(|v| v.downcast_ref::<T>()).cloned()
    }
}

impl AppState {
    /// `Config::from_env()` + [`AppState::new`].
    pub async fn from_env() -> Result<Self, StateError> {
        Self::new(Config::from_env()?)
    }

    /// Builds the state with a lazy pool from the configuration.
    pub fn new(config: Config) -> Result<Self, StateError> {
        if config.db.engine != DbEngine::Postgresql {
            return Err(StateError::UnsupportedEngine(config.db.engine));
        }
        let pool = pool_options(&config).connect_lazy_with(pg_connect_options(&config.db));
        Self::with_pool(config, pool)
    }

    /// Builds the state with an existing pool (tests, own pool in the server).
    pub fn with_pool(config: Config, db: PgPool) -> Result<Self, StateError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| StateError::Http(e.to_string()))?;
        let cache_backend = crate::cache::backend(&config);
        Ok(Self {
            inner: Arc::new(AppStateInner {
                db,
                tz: config.tz,
                config,
                http,
                cache_backend,
                local_cache: LocalCache::default(),
                guard_table: GuardTable::default(),
                services: Services::default(),
            }),
        })
    }

    /// Database pool.
    pub fn db(&self) -> &PgPool {
        &self.inner.db
    }

    /// Configuration.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Time zone (`PAPERLESS_TIME_ZONE`).
    pub fn tz(&self) -> &Tz {
        &self.inner.tz
    }

    /// HTTP-Client.
    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    /// Django's `default` cache (process-local or in PostgreSQL, see [`crate::cache`]).
    pub fn cache(&self) -> DjangoCache<'_> {
        DjangoCache {
            backend: self.inner.cache_backend,
            db: &self.inner.db,
            local: &self.inner.local_cache,
            guard: &self.inner.guard_table,
        }
    }

    /// Keep protection keys (TOTP replay, `throttle_*`, `allauth:rl:*`) with the local backend in
    /// process memory instead of in `paperrust_cache` (unit tests without a database).
    pub fn keep_guarded_keys_local(&self) {
        self.inner.guard_table.set_local();
    }

    /// Plugged-in service of type `T` (e.g. `Arc<dyn pl_queue::TaskQueue>`), if set.
    pub fn service<T: Any + Send + Sync + Clone>(&self) -> Option<T> {
        self.inner.services.get::<T>()
    }

    /// Plug in a service (at startup, before the first request).
    pub fn set_service<T: Any + Send + Sync>(&self, value: T) {
        self.inner.services.insert(value);
    }
}

/// Connection options from `DbConfig` (host or Unix socket directory, SSL, timeout).
pub fn pg_connect_options(db: &DbConfig) -> PgConnectOptions {
    let mut o = PgConnectOptions::new_without_pgpass()
        .username(&db.user)
        .password(&db.password)
        .database(&db.name)
        .application_name(&db.application_name);
    if let Some(h) = &db.host {
        o = if h.starts_with('/') { o.socket(h) } else { o.host(h) };
    }
    if let Some(p) = db.port {
        o = o.port(p);
    }
    let mode = match db.sslmode.as_str() {
        "disable" => PgSslMode::Disable,
        "allow" => PgSslMode::Allow,
        "require" => PgSslMode::Require,
        "verify-ca" => PgSslMode::VerifyCa,
        "verify-full" => PgSslMode::VerifyFull,
        _ => PgSslMode::Prefer,
    };
    o = o.ssl_mode(mode);
    if let Some(c) = &db.sslrootcert {
        o = o.ssl_root_cert(c);
    }
    if let Some(c) = &db.sslcert {
        o = o.ssl_client_cert(c);
    }
    if let Some(k) = &db.sslkey {
        o = o.ssl_client_key(k);
    }
    o
}

/// Pool settings: `PAPERLESS_DB_POOLSIZE` or `PAPERRUST_DB_MAX_CONNECTIONS` (default 10),
/// `test_before_acquire(false)`, ping only after ≥ 30 s idle or after an observed
/// connection error ([`crate::dbconn`]).
pub fn pool_options(config: &Config) -> PgPoolOptions {
    let max = config
        .db
        .pool_max_size
        .or_else(|| config.env.int("PAPERRUST_DB_MAX_CONNECTIONS").ok().flatten().map(|v| v.clamp(1, 1000) as u32))
        .unwrap_or(10);
    let mut o = PgPoolOptions::new()
        .max_connections(max)
        .min_connections(0)
        .idle_timeout(Some(POOL_IDLE_TIMEOUT))
        .max_lifetime(Some(POOL_MAX_LIFETIME))
        .test_before_acquire(false)
        .before_acquire(|conn, meta| {
            Box::pin(async move {
                // ≥ 30 s idle or already idle when a connection error was noticed (DB restart)
                if crate::dbconn::needs_ping(meta.idle_for)
                    && let Err(e) = conn.ping().await
                {
                    crate::dbconn::note_error(&e);
                    return Err(e);
                }
                Ok(true)
            })
        });
    if let Some(t) = config.db.connect_timeout {
        o = o.acquire_timeout(Duration::from_secs(t.max(1)));
    }
    // PaperRust (FINDINGS B2): wait time for a free pool connection separately configurable
    // (seconds, takes precedence over PAPERLESS_DB_TIMEOUT; sqlx default 30 s).
    if let Some(t) = config.env.int("PAPERRUST_DB_ACQUIRE_TIMEOUT").ok().flatten().filter(|t| *t > 0) {
        o = o.acquire_timeout(Duration::from_secs(t.clamp(1, 3600) as u64));
    }
    o
}

#[cfg(test)]
mod service_tests {
    use super::*;

    #[test]
    fn pool_closes_idle_and_old_connections() {
        let env = crate::config::Env::from_pairs([("PAPERLESS_SECRET_KEY", "pool-test-secret-key-0123456789abcdef")]);
        let cfg = Config::from_env_map(env).unwrap();
        let o = pool_options(&cfg);
        assert_eq!(o.get_idle_timeout(), Some(POOL_IDLE_TIMEOUT));
        assert_eq!(o.get_max_lifetime(), Some(POOL_MAX_LIFETIME));
        assert_eq!(o.get_min_connections(), 0);
        // Idle timeout shorter than maximum age, both short enough for the idle time after ingestion.
        assert!(POOL_IDLE_TIMEOUT < POOL_MAX_LIFETIME && POOL_MAX_LIFETIME <= Duration::from_secs(300));
    }

    #[test]
    fn services_by_type() {
        let s = Services::default();
        assert_eq!(s.get::<Arc<String>>(), None);
        s.insert(Arc::new(String::from("q")));
        s.insert(7u32);
        assert_eq!(s.get::<Arc<String>>().as_deref().map(String::as_str), Some("q"));
        assert_eq!(s.get::<u32>(), Some(7));
        s.insert(8u32);
        assert_eq!(s.get::<u32>(), Some(8));
        let d: Arc<dyn Fn() -> i32 + Send + Sync> = Arc::new(|| 3);
        s.insert(d);
        assert_eq!(s.get::<Arc<dyn Fn() -> i32 + Send + Sync>>().map(|f| f()), Some(3));
    }
}
