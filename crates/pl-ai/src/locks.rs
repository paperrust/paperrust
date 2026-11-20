//! Locks of the LLM index – compatible with upstream `paperless_ai/indexing.py` (v3.4.2) and
//! `filelock` 4.2.6, so that Django workers and PaperRust exclude each other:
//!
//! * `LLM_INDEX_LOCK` = `llm_index/index.lock` – `filelock.FileLock`: exclusive, writer against
//!   writer. Windows: `LockFileEx` on byte 0; Unix: `flock(LOCK_EX)`. Rust: `File::lock()`
//!   (Windows locks the whole range from 0 – overlaps byte 0, so mutually effective).
//! * `LLM_INDEX_RWLOCK` = `llm_index/llmindex.rwlock.db` – `filelock.ReadWriteLock`: an
//!   SQLite file with `PRAGMA journal_mode=MEMORY`; readers `BEGIN TRANSACTION; SELECT name FROM
//!   sqlite_master LIMIT 1;` (SHARED), compaction `BEGIN EXCLUSIVE TRANSACTION;`, release
//!   `ROLLBACK` + close. Exclusive timeout `LLM_INDEX_COMPACTION_LOCK_TIMEOUT` = 30 s.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

/// `LLM_INDEX_COMPACTION_LOCK_TIMEOUT`.
pub const COMPACTION_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

pub fn index_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("llm_index")
}
pub fn lock_path(index_dir: &Path) -> PathBuf {
    index_dir.join("index.lock")
}
pub fn rwlock_path(index_dir: &Path) -> PathBuf {
    index_dir.join("llmindex.rwlock.db")
}

/// Held write lock (`FileLock(LLM_INDEX_LOCK)`); released on drop.
pub struct WriteLock {
    file: File,
}

impl WriteLock {
    /// Blocking (upstream without timeout).
    pub fn acquire(index_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(index_dir)?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_path(index_dir))?;
        file.lock()?;
        Ok(Self { file })
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Error with the RW lock.
#[derive(Debug, thiserror::Error)]
pub enum RwError {
    #[error("timeout")]
    Timeout,
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Held read or exclusive lock on `llmindex.rwlock.db`.
pub struct RwGuard {
    conn: Option<Connection>,
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

fn acquire(index_dir: &Path, write: bool, timeout: Option<Duration>) -> Result<RwGuard, RwError> {
    std::fs::create_dir_all(index_dir)?;
    let conn = Connection::open(rwlock_path(index_dir))?;
    let start = std::time::Instant::now();
    loop {
        let left = match timeout {
            Some(t) => t.saturating_sub(start.elapsed()),
            None => Duration::from_millis(100),
        };
        let ms = left.as_millis().clamp(1, 100) as u64;
        let stmts = if write {
            format!("PRAGMA busy_timeout={ms}; PRAGMA journal_mode=MEMORY; BEGIN EXCLUSIVE TRANSACTION;")
        } else {
            format!("PRAGMA busy_timeout={ms}; PRAGMA journal_mode=MEMORY; BEGIN TRANSACTION; SELECT name FROM sqlite_master LIMIT 1;")
        };
        match conn.execute_batch(&stmts) {
            Ok(()) => return Ok(RwGuard { conn: Some(conn) }),
            Err(e) if is_busy(&e) => {
                if !conn.is_autocommit() {
                    let _ = conn.execute_batch("ROLLBACK");
                }
                if timeout.is_some_and(|t| start.elapsed() >= t) {
                    return Err(RwError::Timeout);
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

impl RwGuard {
    /// `read_store()`: shared, without timeout.
    pub fn read(index_dir: &Path) -> Result<Self, RwError> {
        acquire(index_dir, false, None)
    }
    /// `_exclude_readers()`: exclusive, timeout 30 s → [`RwError::Timeout`] (upstream: skip).
    pub fn exclusive(index_dir: &Path, timeout: Duration) -> Result<Self, RwError> {
        acquire(index_dir, true, Some(timeout))
    }
}

impl Drop for RwGuard {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            if !c.is_autocommit() {
                let _ = c.execute_batch("ROLLBACK");
            }
            let _ = c.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_share_writer_excludes() {
        let d = std::env::temp_dir().join(format!("plai-locks-{}", uuid::Uuid::new_v4()));
        let r1 = RwGuard::read(&d).unwrap();
        let r2 = RwGuard::read(&d).unwrap();
        assert!(matches!(RwGuard::exclusive(&d, Duration::from_millis(300)), Err(RwError::Timeout)));
        drop(r1);
        drop(r2);
        let w = RwGuard::exclusive(&d, Duration::from_secs(2)).unwrap();
        drop(w);
        let l = WriteLock::acquire(&d).unwrap();
        drop(l);
        let _ = std::fs::remove_dir_all(&d);
    }
}
