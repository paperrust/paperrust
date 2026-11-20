//! Pool connections after a database restart (INTEGRATION-TODO "discard pool connections").
//!
//! Django (`CONN_MAX_AGE = 0`, `CONN_HEALTH_CHECKS = True`, `src/paperless/settings/custom.py`)
//! opens a new connection per request and therefore knows no dead connections after a
//! PostgreSQL restart. The sqlx pool, by contrast, keeps connections open; a connection terminated
//! by the server during idle time (restart, `pg_terminate_backend`) only yields an error on the next
//! use (`57P01 terminating connection due to administrator command` or I/O).
//!
//! Approach, without slowing down the normal path (no ping per `acquire`):
//!
//! 1. `before_acquire` pings as before after ≥ [`PING_IF_IDLE_FOR`](crate::state::PING_IF_IDLE_FOR)
//!    idle time – and additionally every connection that was already idle when a
//!    connection error was observed somewhere ([`needs_ping`]). A failed ping discards the
//!    connection, sqlx then fetches another one or builds a new one. Normal case: one more
//!    `AtomicU64` load.
//! 2. Connection errors are reported centrally in `From<sqlx::Error> for ApiError`
//!    ([`note_error`]); sqlx itself discards the affected connection on return (ping
//!    in `return_to_pool`).
//! 3. [`retry_stale_once`]: for the first query of a request (e.g. user/token lookup)
//!    retry once if it failed on a dead connection – then even the
//!    first request after the restart sees no error.

use std::future::Future;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Reference point for [`LAST_FAILURE_MS`].
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Milliseconds since [`epoch`] + 1 at the last observed connection error; 0 = never.
static LAST_FAILURE_MS: AtomicU64 = AtomicU64::new(0);

/// Safety margin: even connections that became idle up to this long **after** the error
/// are pinged once (the error and the return of other connections overlap).
const FAILURE_MARGIN: Duration = Duration::from_secs(1);

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// Error that indicates a dead/unusable connection (not: SQL/constraint error).
///
/// SQLSTATE class `08` (connection exception), `57P01` admin_shutdown, `57P02` crash_shutdown,
/// `57P03` cannot_connect_now; additionally I/O, TLS and protocol errors.
pub fn is_connection_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::Protocol(_) => true,
        sqlx::Error::Database(d) => d
            .code()
            .is_some_and(|c| c.starts_with("08") || matches!(c.as_ref(), "57P01" | "57P02" | "57P03")),
        _ => false,
    }
}

/// Reports a database error; connection errors mark all idle connections as
/// needing a check and the database as "unreachable" ([`is_down`], tests/chaos C1).
pub fn note_error(e: &sqlx::Error) {
    if is_connection_error(e) {
        LAST_FAILURE_MS.store(now_ms() + 1, Ordering::Relaxed);
    }
    if is_unavailable(e) {
        mark_down();
    }
}

// ---------------------------------------------------------------------------------------------
// Database unreachable (tests/chaos C1)
//
// If PostgreSQL is gone (restart, crash, network), sqlx waits at every `acquire` up to the
// timeout (default 30 s) and keeps trying to connect during that time. Without protection,
// **every** request hung for 30 s and ended with "Server Error (500)". Instead:
// * Connection errors/pool timeouts mark the DB as "gone" ([`note_error`]).
// * As long as it is "gone", the API responds immediately with 503 and an understandable message
//   (middleware in pl-server, [`DB_UNAVAILABLE_DETAIL`]); errors from handlers also become 503.
// * [`watch_health`] then checks every second with its own connection whether the DB is back,
//   and clears the mark – recovery needs no manual work.
// ---------------------------------------------------------------------------------------------

/// `detail` of the 503 response while the database is unreachable.
pub const DB_UNAVAILABLE_DETAIL: &str = "The database is temporarily not reachable. Please try again in a moment.";
/// `Retry-After` (seconds) of the 503 response.
pub const DB_UNAVAILABLE_RETRY_AFTER: &str = "5";

/// Milliseconds since [`epoch`] + 1 since the DB is considered unreachable; 0 = reachable.
static DOWN_SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// Error after which the database is considered unreachable: connection errors, pool time
/// exceeded (no connection obtainable), pool closed.
pub fn is_unavailable(e: &sqlx::Error) -> bool {
    is_connection_error(e) || matches!(e, sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed)
}

/// Mark the DB as unreachable (idempotent).
pub fn mark_down() {
    if DOWN_SINCE_MS.compare_exchange(0, now_ms() + 1, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        tracing::warn!("database not reachable; the API answers 503 until it is back");
    }
}

/// Clear the mark (DB answers again).
pub fn mark_up() {
    let since = DOWN_SINCE_MS.swap(0, Ordering::Relaxed);
    if since != 0 {
        tracing::info!(down_for_secs = now_ms().saturating_sub(since - 1) / 1000, "database reachable again");
    }
}

/// Suspicion of an outage that no request has yet seen as an error (they wait in
/// `acquire` while the pool tries to reconnect in the background). [`watch_health`] then checks
/// once with its own connection and marks the DB as gone if necessary.
static SUSPECT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A request has been running suspiciously long (tests/chaos c1, overall check 2026-11-18: the first request
/// after a DB crash waited until the DB returned). Costs only an atomic store.
pub fn suspect() {
    SUSPECT.store(true, Ordering::Relaxed);
}

/// Is the database currently considered unreachable? (one atomic load)
pub fn is_down() -> bool {
    DOWN_SINCE_MS.load(Ordering::Relaxed) != 0
}

/// Result of a reachability check: if the server answers at all (even with an error
/// such as "too many clients"), it is reachable.
fn probe_says_up<T>(r: &Result<T, sqlx::Error>) -> bool {
    match r {
        Ok(_) => true,
        Err(e) => !is_unavailable(e),
    }
}

/// Background task: as long as the DB is marked "gone", try its own connection every second
/// (outside the pool, max. 3 s); if it succeeds, clear the mark. In normal operation costs
/// only one atomic load per second.
pub async fn watch_health(opts: sqlx::postgres::PgConnectOptions) {
    use sqlx::{ConnectOptions as _, Connection as _};
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let down = is_down();
        // Suspicion (long-running request, [`suspect`]) without a mark: check once
        if !down && !SUSPECT.swap(false, Ordering::Relaxed) {
            continue;
        }
        let r = match tokio::time::timeout(Duration::from_secs(3), opts.connect()).await {
            Ok(r) => r,
            Err(_) => Err(sqlx::Error::PoolTimedOut),
        };
        if !down {
            if probe_says_up(&r) {
                if let Ok(c) = r {
                    let _ = c.close().await;
                }
            } else {
                mark_down();
            }
            continue;
        }
        if probe_says_up(&r) {
            if let Ok(c) = r {
                let _ = c.close().await;
            }
            // Check connections from before the outage on next use
            LAST_FAILURE_MS.store(now_ms() + 1, Ordering::Relaxed);
            mark_up();
        }
    }
}

/// Whether a connection with this idle time must be pinged before being handed out.
pub fn needs_ping(idle_for: Duration) -> bool {
    if idle_for >= crate::state::PING_IF_IDLE_FOR {
        return true;
    }
    let f = LAST_FAILURE_MS.load(Ordering::Relaxed);
    if f == 0 {
        return false;
    }
    stale_since_failure(f - 1, now_ms(), idle_for)
}

/// Was the connection (since `now - idle_for`) already idle when the error (`failure_ms`) occurred
/// (with [`FAILURE_MARGIN`])?
fn stale_since_failure(failure_ms: u64, now_ms: u64, idle_for: Duration) -> bool {
    let idle_since = now_ms.saturating_sub(idle_for.as_millis() as u64);
    idle_since <= failure_ms + FAILURE_MARGIN.as_millis() as u64
}

/// Maximum wait time for the retry in [`retry_stale_once`].
pub const RETRY_LIMIT: Duration = Duration::from_secs(3);

/// Runs `f` and repeats it **once** if it failed with a connection error
/// (the dead connection is then discarded, all other idle ones are pinged).
///
/// Use only for queries without side effects or outside a transaction.
pub async fn retry_stale_once<T, F, Fut>(mut f: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    match f().await {
        Err(e) if is_connection_error(&e) => {
            note_error(&e);
            tracing::warn!(error = %e, "database connection lost; retrying once");
            // tests/chaos C1: if the DB is really gone (not just an old connection dead), the
            // second attempt would wait until the pool timeout (30 s). A new connection to a
            // running DB takes milliseconds → wait at most RETRY_LIMIT, then 503.
            match tokio::time::timeout(RETRY_LIMIT, f()).await {
                Ok(r) => r,
                Err(_) => {
                    mark_down();
                    Err(sqlx::Error::PoolTimedOut)
                }
            }
        }
        r => r,
    }
}

/// Own small pool with the same connection options as `pool` (FINDINGS B2): for work that
/// must not starve on the general pool (heartbeat/lease/claim of the task queue,
/// sync sequencer, long poll) or must not drain it. Connections are only created
/// on demand (`connect_lazy`); `max` limits only this secondary pool.
pub fn side_pool(pool: &sqlx::PgPool, max: u32) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(max.max(1))
        .min_connections(0)
        .idle_timeout(Some(crate::state::POOL_IDLE_TIMEOUT))
        .max_lifetime(Some(crate::state::POOL_MAX_LIFETIME))
        .acquire_timeout(pool.options().get_acquire_timeout())
        .test_before_acquire(false)
        .before_acquire(|conn, meta| {
            Box::pin(async move {
                if needs_ping(meta.idle_for)
                    && let Err(e) = sqlx::Connection::ping(conn).await
                {
                    note_error(&e);
                    return Err(e);
                }
                Ok(true)
            })
        })
        .connect_lazy_with((*pool.connect_options()).clone())
}

/// `LISTEN` connection **outside** the general pool (FINDINGS B2): sqlx's
/// `PgListener::connect_with(pool)` keeps a pool connection permanently; with four listeners
/// only 6 of 10 connections remained for requests and tasks. Here each listener gets its
/// own single-connection pool (like `PgListener::connect(url)`, but with the options of the
/// main pool: host/socket, SSL, password), via which it also reconnects.
pub async fn listener(pool: &sqlx::PgPool) -> Result<sqlx::postgres::PgListener, sqlx::Error> {
    let own = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .max_lifetime(None)
        .idle_timeout(None)
        .acquire_timeout(pool.options().get_acquire_timeout())
        .connect_lazy_with((*pool.connect_options()).clone());
    sqlx::postgres::PgListener::connect_with(&own).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/chaos c1: without a reported error (all requests wait in `acquire`) only the
    /// suspicion of a long-running request marks the DB as gone – here against a closed port.
    #[tokio::test]
    async fn suspect_marks_unreachable_db_down() {
        let opts = sqlx::postgres::PgConnectOptions::new().host("127.0.0.1").port(1).username("x");
        mark_up();
        let watcher = tokio::spawn(watch_health(opts));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!is_down(), "no probe without suspicion");
        suspect();
        let t0 = std::time::Instant::now();
        while !is_down() && t0.elapsed() < Duration::from_secs(8) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let marked = is_down();
        watcher.abort();
        mark_up();
        assert!(marked, "unreachable database not marked down after suspicion");
    }

    #[test]
    fn classification() {
        assert!(is_connection_error(&sqlx::Error::Io(std::io::Error::other("x"))));
        assert!(is_connection_error(&sqlx::Error::Protocol("x".into())));
        assert!(!is_connection_error(&sqlx::Error::RowNotFound));
        assert!(!is_connection_error(&sqlx::Error::PoolTimedOut));
        assert!(is_unavailable(&sqlx::Error::PoolTimedOut));
        assert!(is_unavailable(&sqlx::Error::Io(std::io::Error::other("x"))));
        assert!(!is_unavailable(&sqlx::Error::RowNotFound));
    }

    #[test]
    fn down_flag_and_probe() {
        // reachable as soon as the server answers at all
        assert!(!probe_says_up::<()>(&Err(sqlx::Error::Io(std::io::Error::other("refused")))));
        assert!(!probe_says_up::<()>(&Err(sqlx::Error::PoolTimedOut)));
        assert!(probe_says_up::<()>(&Err(sqlx::Error::Configuration("x".into()))));
        mark_down();
        assert!(is_down());
        mark_up();
        assert!(!is_down());
        note_error(&sqlx::Error::RowNotFound);
        assert!(!is_down());
        note_error(&sqlx::Error::PoolTimedOut);
        assert!(is_down());
        mark_up();
    }

    #[tokio::test]
    async fn side_pool_closes_idle_and_old_connections() {
        let main = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        let side = side_pool(&main, 3);
        assert_eq!(side.options().get_idle_timeout(), Some(crate::state::POOL_IDLE_TIMEOUT));
        assert_eq!(side.options().get_max_lifetime(), Some(crate::state::POOL_MAX_LIFETIME));
        assert_eq!(side.options().get_max_connections(), 3);
    }

    #[test]
    fn staleness_window() {
        let ms = Duration::from_millis;
        // went idle at 5000, failure at 9000 → stale
        assert!(stale_since_failure(9000, 10_000, ms(5000)));
        // went idle at 9500, failure at 9000 → within margin → ping once
        assert!(stale_since_failure(9000, 10_000, ms(500)));
        // went idle at 10_500 (> failure + margin) → fresh
        assert!(!stale_since_failure(9000, 11_000, ms(500)));
        assert!(needs_ping(crate::state::PING_IF_IDLE_FOR));
    }
}

/// DB tests (`cargo test -p pl-core -j 1 -- --ignored dbconn`) against the local reference PostgreSQL
/// (environment from `reference/env.sh`); only `SELECT 1` and `pg_terminate_backend` on **own**
/// connections – nothing is written.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::Config;
    use crate::state::{pg_connect_options, pool_options};
    use sqlx::{Connection, PgPool};

    fn pool(cfg: &Config, max: u32) -> PgPool {
        pool_options(cfg).max_connections(max).connect_lazy_with(pg_connect_options(&cfg.db))
    }

    async fn pids(pool: &PgPool, n: usize) -> Vec<i32> {
        let mut held = vec![];
        for _ in 0..n {
            held.push(pool.acquire().await.unwrap());
        }
        let mut out = vec![];
        for c in held.iter_mut() {
            out.push(sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()").fetch_one(&mut **c).await.unwrap());
        }
        out
    }

    async fn kill(cfg: &Config, pids: &[i32]) {
        let mut c = sqlx::PgConnection::connect_with(&pg_connect_options(&cfg.db)).await.unwrap();
        for p in pids {
            let ok: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)").bind(p).fetch_one(&mut c).await.unwrap();
            assert!(ok);
        }
        c.close().await.ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    #[ignore = "braucht die lokale Referenz-PostgreSQL (reference/env.sh)"]
    async fn dbconn_recovers_after_backend_termination() {
        let cfg = Config::from_env().unwrap();
        let pool = pool(&cfg, 4);
        let p = pids(&pool, 4).await;
        // give sqlx time to return the connections (release pings asynchronously)
        tokio::time::sleep(Duration::from_millis(200)).await;
        kill(&cfg, &p).await;
        // the first query hits a dead connection; retry_stale_once recovers it
        let v: i32 = retry_stale_once(|| sqlx::query_scalar("SELECT 1").fetch_one(&pool)).await.unwrap();
        assert_eq!(v, 1);
        // all other stale connections are now pinged and replaced: no further errors
        for _ in 0..8 {
            let held = pids(&pool, 4).await;
            assert!(held.iter().all(|x| !p.contains(x)));
        }
    }

    /// Measurement: normal path (no error) against `test_before_acquire(true)`.
    #[tokio::test]
    #[ignore = "Messung; braucht die lokale Referenz-PostgreSQL (reference/env.sh)"]
    async fn dbconn_measure_acquire_overhead() {
        let cfg = Config::from_env().unwrap();
        let fast = pool(&cfg, 2);
        let tested = pool_options(&cfg).max_connections(2).test_before_acquire(true).connect_lazy_with(pg_connect_options(&cfg.db));
        // previous behaviour (ping only after ≥ 30 s idle) as baseline
        let old = pool_options(&cfg)
            .max_connections(2)
            .before_acquire(|conn, meta| {
                Box::pin(async move {
                    if meta.idle_for >= crate::state::PING_IF_IDLE_FOR {
                        conn.ping().await?;
                    }
                    Ok(true)
                })
            })
            .connect_lazy_with(pg_connect_options(&cfg.db));
        let n = 3000;
        for p in [&old, &fast, &tested] {
            for _ in 0..200 {
                sqlx::query("SELECT 1").execute(p).await.unwrap();
            }
        }
        let mut res = vec![];
        for round in 0..3 {
            for (name, p) in [("baseline_before", &old), ("pool_options", &fast), ("test_before_acquire", &tested)] {
                let t = Instant::now();
                for _ in 0..n {
                    sqlx::query("SELECT 1").execute(p).await.unwrap();
                }
                let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
                res.push(format!("round {round} {name}: {us:.1} µs/query"));
            }
        }
        println!("{}", res.join("\n"));
    }
}
