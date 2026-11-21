//! Start checks against the database, before anything else touches it (test log 2026-11-16):
//!
//! * **D-087 – database not reachable:** one connection attempt after the other until a deadline
//!   (`PAPERLESS_DB_TIMEOUT`, else [`DEFAULT_WAIT`]); then abort with
//!   „Datenbank nicht erreichbar (host:port, DB name)“ instead of the pool's
//!   „pool timed out“ after ~33 s. The message never contains the password.
//! * **A-181/B-312 – schema not up to date:** abort with „Datenbank ist nicht auf dem neuesten
//!   Stand – zuerst `paperrust manage migrate` ausführen“ when v3.4.2 or a PaperRust migration is
//!   missing ([`pl_cli::migrate::startup_check`], same logic as `manage migrate --check`).
//!
//! Why abort instead of migrating automatically: the Docker entrypoint and the desktop supervisor
//! run `paperrust manage migrate` before every start anyway; the data model stages rebuild tables
//! and need a backup first (D-073), which a server start should not do behind the operator's back
//! (upstream: bare-metal installs run `manage.py migrate` by hand too, docs/setup.md@v3.4.2).

use std::time::{Duration, Instant};

use pl_core::config::DbConfig;
use sqlx::{ConnectOptions as _, PgConnection};

/// Wait for the database at most this long when `PAPERLESS_DB_TIMEOUT` is not set.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);
/// Pause between two connection attempts.
const RETRY_PAUSE: Duration = Duration::from_millis(500);

/// `host:port` (or the socket directory) and database name – without user and password.
pub fn describe(db: &DbConfig) -> String {
    let host = db.host.as_deref().unwrap_or("localhost");
    let port = db.port.unwrap_or(5432);
    let addr = if host.starts_with('/') { format!("{host} (socket, port {port})") } else { format!("{host}:{port}") };
    format!("{addr}, DB {}", db.name)
}

/// The message for an unreachable database (`last` = the last connection error).
pub fn unreachable_message(db: &DbConfig, waited: Duration, last: &str) -> String {
    let mut last = last.to_string();
    // libpq-style errors never contain the password; be defensive anyway.
    if !db.password.is_empty() {
        last = last.replace(&db.password, "***");
    }
    format!(
        "Datenbank nicht erreichbar / database not reachable: {} (waited {} s): {last}. \
         Check PAPERRUST_DBHOST/PAPERRUST_DBPORT and that PostgreSQL is running.",
        describe(db),
        waited.as_secs()
    )
}

/// How long to wait for the database: `PAPERLESS_DB_TIMEOUT` (seconds, ≥ 1), else [`DEFAULT_WAIT`].
pub fn wait_limit(db: &DbConfig) -> Duration {
    db.connect_timeout.map(|t| Duration::from_secs(t.max(1))).unwrap_or(DEFAULT_WAIT)
}

/// Open one connection, retrying until `limit` is over (connection refused, DNS, timeouts).
/// Authentication and other server-side errors abort at once (retrying does not help).
pub async fn connect(db: &DbConfig, limit: Duration) -> Result<PgConnection, String> {
    let opts = pl_core::state::pg_connect_options(db);
    let start = Instant::now();
    loop {
        let left = limit.saturating_sub(start.elapsed());
        let err = match tokio::time::timeout(left.max(Duration::from_millis(100)), opts.connect()).await {
            Ok(Ok(c)) => return Ok(c),
            Ok(Err(sqlx::Error::Database(e))) => {
                // reachable, but refused (password, missing database …): say so right away
                return Err(format!("database {} refused the connection: {}", describe(db), e.message()));
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "no answer (timeout)".to_string(),
        };
        if start.elapsed() + RETRY_PAUSE >= limit {
            return Err(unreachable_message(db, start.elapsed(), &err));
        }
        tokio::time::sleep(RETRY_PAUSE).await;
    }
}

/// Both start checks; called by `paperrust serve` before migrations, worker and HTTP server.
pub async fn check(db: &DbConfig) -> Result<(), String> {
    let mut conn = connect(db, wait_limit(db)).await?;
    pl_cli::migrate::startup_check(&mut conn).await.map_err(|e| e.to_string())
}

/// tests/chaos C1: as long as the database is considered unreachable ([`pl_core::dbconn::is_down`]),
/// the server answers immediately: the API (`/api/…`, `/rs/api/…`) with 503 + JSON message + `Retry-After`,
/// pages with a short 503 page (de/en) that reloads every 5 s – instead of letting every request wait until the
/// pool timeout (30 s) and end with 500. Static files stay reachable.
pub async fn unavailable_middleware(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    if pl_core::dbconn::is_down() {
        let path = req.uri().path();
        if is_db_path(path) {
            return pl_core::ApiError::db_unavailable().into_response();
        }
        if !is_static_path(path) {
            return unavailable_page();
        }
    }
    // tests/chaos c1 (overall check 2026-11-18): a reading API request that started shortly before the outage
    // waited in `acquire` until the DB came back (measured 17.9 s), although the DB had long been marked
    // "gone". Reading requests (GET/HEAD, without side effects) therefore end with 503 as soon as
    // the DB is considered gone and they have already been running for `IN_FLIGHT_LIMIT`; if one runs longer without the DB
    // being considered gone, `watch_health` checks once (`dbconn::suspect`). Writing requests keep running (an
    // abort after a successful COMMIT would report a success as an error).
    if is_db_path(req.uri().path()) && matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        return abort_when_down(next.run(req), pl_core::dbconn::is_down, IN_FLIGHT_LIMIT)
            .await
            .unwrap_or_else(|| pl_core::ApiError::db_unavailable().into_response());
    }
    next.run(req).await
}

/// Maximum run time of a reading request while the DB is considered unreachable.
const IN_FLIGHT_LIMIT: std::time::Duration = pl_core::dbconn::RETRY_LIMIT;

/// Let `fut` run to completion – unless `is_down()` reports the outage and `fut` has already run for `limit`;
/// then `None` (the handler is discarded).
async fn abort_when_down<T>(
    fut: impl std::future::Future<Output = T>,
    is_down: impl Fn() -> bool,
    limit: std::time::Duration,
) -> Option<T> {
    let start = std::time::Instant::now();
    let mut suspected = false;
    tokio::pin!(fut);
    loop {
        tokio::select! {
            r = &mut fut => return Some(r),
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                if start.elapsed() >= limit {
                    if is_down() {
                        return None;
                    }
                    // nobody has noticed the outage yet (all are waiting in `acquire`): have it checked
                    if !suspected {
                        pl_core::dbconn::suspect();
                        suspected = true;
                    }
                }
            }
        }
    }
}

fn is_db_path(p: &str) -> bool {
    p.starts_with("/api/") || p.starts_with("/rs/api/")
}

/// Files that are served without a database (UI building blocks, icons).
fn is_static_path(p: &str) -> bool {
    const EXT: &[&str] =
        &[".js", ".css", ".png", ".svg", ".ico", ".woff", ".woff2", ".webmanifest", ".map", ".json", ".webp", ".br", ".gz"];
    p.starts_with("/static/") || p.starts_with("/rs/app/assets/") || EXT.iter().any(|e| p.ends_with(e))
}

const UNAVAILABLE_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"5\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>PaperRust</title><style>body{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;color:#222}</style></head><body><h1>Datenbank vorübergehend nicht erreichbar</h1><p>PaperRust wartet auf die Datenbank. Diese Seite lädt sich alle 5 Sekunden neu, sobald die Datenbank wieder da ist, geht es automatisch weiter. Es gehen keine Dokumente verloren.</p><h1 lang=\"en\">Database temporarily not reachable</h1><p lang=\"en\">PaperRust is waiting for the database. This page reloads every 5 seconds and continues automatically once the database is back. No documents are lost.</p></body></html>";

fn unavailable_page() -> axum::response::Response {
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse as _;
    let mut r = (StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE_HTML).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static(pl_core::dbconn::DB_UNAVAILABLE_RETRY_AFTER));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[cfg(test)]
mod unavailable_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// tests/chaos c1: a hanging read request ends after the outage at the latest after the deadline (+250 ms).
    #[tokio::test]
    async fn stuck_read_ends_when_db_down() {
        let down = Arc::new(AtomicBool::new(false));
        let d = down.clone();
        let flip = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            d.store(true, Ordering::Relaxed);
        });
        let start = std::time::Instant::now();
        let r = abort_when_down(tokio::time::sleep(Duration::from_secs(30)), || down.load(Ordering::Relaxed), Duration::from_secs(1)).await;
        assert!(r.is_none());
        assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());
        flip.await.unwrap();
        // DB reachable: nothing is aborted
        let r = abort_when_down(
            async {
                tokio::time::sleep(Duration::from_millis(600)).await;
                7
            },
            || false,
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(r, Some(7));
        // fast response despite outage unchanged
        assert_eq!(abort_when_down(async { 1 }, || true, Duration::ZERO).await, Some(1));
    }

    #[test]
    fn only_api_paths() {
        assert!(is_db_path("/api/documents/"));
        assert!(is_db_path("/rs/api/v2/documents"));
        assert!(!is_db_path("/rs/app/"));
        assert!(!is_db_path("/static/base.css"));
        assert!(is_static_path("/static/base.css"));
        assert!(is_static_path("/rs/app/assets/index-abc.js"));
        assert!(!is_static_path("/rs/app/"));
        assert!(!is_static_path("/accounts/login/"));
        assert!(UNAVAILABLE_HTML.contains("Datenbank") && UNAVAILABLE_HTML.contains("refresh"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(port: u16) -> DbConfig {
        let env = pl_core::config::Env::from_pairs([
            ("PAPERLESS_DBENGINE", "postgresql".to_string()),
            ("PAPERLESS_DBHOST", "127.0.0.1".to_string()),
            ("PAPERLESS_DBPORT", port.to_string()),
            ("PAPERLESS_DBPASS", "geheim-123".to_string()),
            ("PAPERLESS_SECRET_KEY", "test-only-secret-key-for-dbcheck".to_string()),
        ]);
        pl_core::config::Config::from_env_map(env).expect("config").db
    }

    #[test]
    fn description_has_host_port_but_no_password() {
        let d = db(5433);
        assert_eq!(describe(&d), "127.0.0.1:5433, DB paperless");
        let m = unreachable_message(&d, Duration::from_secs(10), "boom geheim-123");
        assert!(m.contains("Datenbank nicht erreichbar") && m.contains("127.0.0.1:5433"));
        assert!(!m.contains("geheim-123"));
        assert_eq!(wait_limit(&d), DEFAULT_WAIT);
    }

    /// D-087: a closed port fails within the limit (not after ~33 s) and names host:port.
    #[tokio::test]
    async fn unreachable_fails_fast_with_address() {
        let d = db(1);
        let t0 = Instant::now();
        let Err(e) = connect(&d, Duration::from_secs(2)).await else { panic!("must fail") };
        assert!(t0.elapsed() < Duration::from_secs(6), "took {:?}", t0.elapsed());
        assert!(e.contains("Datenbank nicht erreichbar") && e.contains("127.0.0.1:1"), "{e}");
        assert!(!e.contains("geheim-123"));
    }
}
