// upstream: filelock 4.x (`FileLock(settings.MEDIA_LOCK)`, without time limit), src/documents/consumer.py@v3.4.2
//           and src/documents/signals/handlers.py@v3.4.2 (update_filename_and_move_files: move files under
//           MEDIA_LOCK, then `Document.objects.filter(pk=…).update(…)`).
//
//! `MEDIA_LOCK` – shared implementation for pl-consumer and pl-documents-write (FINDINGS B1).
//!
//! **Fixed lock order** (the same everywhere, otherwise deadlock that PostgreSQL does not detect):
//!
//! 1. first the database row locks (document rows, in a transaction or a
//!    savepoint – [`lock_document_rows`]),
//! 2. **then** `MEDIA_LOCK`,
//! 3. while `MEDIA_LOCK` is held, only write rows that this transaction has already
//!    locked (never wait for a foreign row lock).
//!
//! Whoever holds `MEDIA_LOCK` therefore never waits for the database; whoever waits for `MEDIA_LOCK` may
//! hold rows. Previously `update_filename_and_move_files` first took `MEDIA_LOCK` and then wrote
//! the document row, while ingesting a new version held the row of the root document
//! and waited for `MEDIA_LOCK` → both waited forever.
//!
//! Additionally: an in-process queue per lock file (own tasks do not poll the file lock at a
//! 50 ms interval and take their turn in order) and a **time limit**
//! (`PAPERRUST_MEDIA_LOCK_TIMEOUT`, seconds, default 120, `0` = no limit like upstream) with a clear
//! error message instead of waiting endlessly, in case something does hang (e.g. another process such as
//! `document_exporter` holds the lock for a very long time).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sqlx::PgConnection;

/// Default time limit for waiting for `MEDIA_LOCK`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Polling interval of the file lock (filelock default).
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// From this wait time on, a warning in the log (once per wait).
const WARN_AFTER: Duration = Duration::from_secs(10);

/// `PAPERRUST_MEDIA_LOCK_TIMEOUT` (seconds; `0` or negative = no limit). `None` = no limit.
pub fn timeout() -> Option<Duration> {
    parse_timeout(pl_core::config::var("PAPERRUST_MEDIA_LOCK_TIMEOUT").as_deref())
}

fn parse_timeout(v: Option<&str>) -> Option<Duration> {
    match v.map(str::trim).filter(|v| !v.is_empty()).map(str::parse::<f64>) {
        Some(Ok(s)) if s.is_finite() && s <= 0.0 => None,
        Some(Ok(s)) if s.is_finite() => Some(Duration::from_secs_f64(s.min(86_400.0))),
        _ => Some(DEFAULT_TIMEOUT),
    }
}

/// In-process queue per lock file.
fn local_queue(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static QUEUES: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let map = QUEUES.get_or_init(Default::default);
    let mut g = match map.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    g.entry(path.to_path_buf()).or_default().clone()
}

fn timed_out(path: &Path, limit: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "timed out after {} s waiting for the media lock {} (another task or process is moving files; \
             PAPERRUST_MEDIA_LOCK_TIMEOUT)",
            limit.as_secs(),
            path.display()
        ),
    )
}

/// Held `MEDIA_LOCK` (file lock like filelock + in-process queue). Released on
/// drop. Not reentrant (like filelock on Windows: a second handle of the same task
/// waits for itself) – never acquire again within the held section.
pub struct MediaLock {
    _file: std::fs::File,
    _local: tokio::sync::OwnedMutexGuard<()>,
}

impl MediaLock {
    /// With the configured time limit ([`timeout`]).
    pub async fn acquire(path: &Path) -> std::io::Result<Self> {
        Self::acquire_with(path, timeout()).await
    }

    /// With an own time limit (`None` = none). Error `ErrorKind::TimedOut` with an understandable
    /// message if the lock is not freed in time.
    pub async fn acquire_with(path: &Path, limit: Option<Duration>) -> std::io::Result<Self> {
        let start = tokio::time::Instant::now();
        let deadline = limit.map(|l| start + l);
        let queue = local_queue(path);
        let local = match deadline {
            Some(d) => match tokio::time::timeout_at(d, queue.lock_owned()).await {
                Ok(g) => g,
                Err(_) => return Err(timed_out(path, limit.unwrap_or_default())),
            },
            None => queue.lock_owned().await,
        };
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut warned = false;
        loop {
            match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path) {
                Ok(f) => match f.try_lock() {
                    Ok(()) => return Ok(Self { _file: f, _local: local }),
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(std::fs::TryLockError::Error(e)) => return Err(e),
                },
                // Windows: file is just being deleted by filelock (Python)
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
                Err(e) => return Err(e),
            }
            let now = tokio::time::Instant::now();
            if let Some(d) = deadline
                && now >= d
            {
                return Err(timed_out(path, limit.unwrap_or_default()));
            }
            if !warned && now.duration_since(start) >= WARN_AFTER {
                warned = true;
                tracing::warn!(
                    target: "paperless.handlers",
                    "Still waiting for the media lock {} after {} s (held by another process?)",
                    path.display(),
                    WARN_AFTER.as_secs()
                );
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Table for `SELECT … FOR UPDATE` on documents: `paperrust_document` as soon as data model stage 1
/// is applied (then `documents_document` is a view), otherwise the Django table.
pub async fn document_lock_table(conn: &mut PgConnection) -> Result<&'static str, sqlx::Error> {
    let native: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relname = 'paperrust_document' AND c.relkind = 'r')",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(if native { "paperrust_document" } else { "documents_document" })
}

/// Step 1 of the lock order: lock the document rows `ids` (e.g. document + root document) in
/// id order with `FOR UPDATE` – **before** `MEDIA_LOCK`. Must run in a transaction or
/// a savepoint (otherwise the lock ends immediately). `FOR UPDATE` (not `NO KEY`):
/// the later `UPDATE … filename` changes a unique column and would otherwise need a stronger lock
/// under `MEDIA_LOCK`.
pub async fn lock_document_rows(conn: &mut PgConnection, ids: &[i64]) -> Result<(), sqlx::Error> {
    let mut ids: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let table = document_lock_table(conn).await?;
    sqlx::query(&format!("SELECT id FROM {table} WHERE id = ANY($1::int4[]) ORDER BY id FOR UPDATE"))
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_setting() {
        assert_eq!(parse_timeout(None), Some(DEFAULT_TIMEOUT));
        assert_eq!(parse_timeout(Some("")), Some(DEFAULT_TIMEOUT));
        assert_eq!(parse_timeout(Some("x")), Some(DEFAULT_TIMEOUT));
        assert_eq!(parse_timeout(Some("0")), None);
        assert_eq!(parse_timeout(Some("-1")), None);
        assert_eq!(parse_timeout(Some("2.5")), Some(Duration::from_millis(2500)));
    }

    /// A second handle waits until the first releases; without a release the wait ends with an
    /// understandable timeout error instead of hanging forever.
    #[tokio::test]
    async fn waits_and_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.lock");
        let first = MediaLock::acquire_with(&path, Some(Duration::from_secs(5))).await.unwrap();
        let t = std::time::Instant::now();
        let second = MediaLock::acquire_with(&path, Some(Duration::from_millis(300))).await;
        let err = second.err().unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("media lock"), "{err}");
        assert!(t.elapsed() >= Duration::from_millis(250));
        let p2 = path.clone();
        let waiter = tokio::spawn(async move { MediaLock::acquire_with(&p2, Some(Duration::from_secs(5))).await.is_ok() });
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(first);
        assert!(waiter.await.unwrap());
    }

    /// The file lock also applies across file handles (another process, e.g. pl-cli/Python).
    #[tokio::test]
    async fn file_lock_held_elsewhere_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.lock");
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).unwrap();
        f.lock().unwrap();
        let r = MediaLock::acquire_with(&path, Some(Duration::from_millis(200))).await;
        assert_eq!(r.err().map(|e| e.kind()), Some(std::io::ErrorKind::TimedOut));
        f.unlock().unwrap();
        assert!(MediaLock::acquire_with(&path, Some(Duration::from_secs(2))).await.is_ok());
    }
}
