// upstream: src/documents/search/_backend.py@v3.4.2 (WriteBatch.__enter__: filelock.FileLock
//           on INDEX_DIR/.tantivy.lock, _LOCK_* constants, retry with jittered backoff),
//           filelock 4.2.6 (UnixFileLock: fcntl.flock(LOCK_EX|LOCK_NB), lock file kept;
//           WindowsFileLock: LockFileEx(EXCLUSIVE|FAIL_IMMEDIATELY) on byte 0, poll 0.05 s)
//
// Cross-process "single writer" lock shared with the Python worker. Rust uses
// `std::fs::File::try_lock` which maps to the same OS primitive: `flock(LOCK_EX|LOCK_NB)`
// on Unix and `LockFileEx(EXCLUSIVE|FAIL_IMMEDIATELY)` over the whole file on Windows
// (overlaps filelock's byte 0 -> mutual exclusion both ways). On top of this, tantivy's own
// `.tantivy-writer.lock` (taken by `Index::writer`) is held for the batch, as in tantivy-py.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{IndexerError, Result};

pub const LOCK_FILE: &str = ".tantivy.lock";
/// `_LOCK_TIMEOUT_SECONDS` (single add_or_update/remove).
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// `batch_update(lock_timeout=30.0)` default.
pub const BATCH_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// `_LOCK_RETRY_ATTEMPTS`.
pub const LOCK_RETRY_ATTEMPTS: u32 = 4;
const LOCK_BACKOFF_BASE: f64 = 1.0;
const LOCK_BACKOFF_CAP: f64 = 10.0;
/// filelock default `poll_interval`.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Held `.tantivy.lock`; released on drop.
#[derive(Debug)]
pub struct IndexFileLock {
    file: Option<File>,
    path: PathBuf,
}

impl IndexFileLock {
    /// One non-blocking attempt. `Ok(None)` = held elsewhere (or transient open
    /// contention, e.g. a Windows name pending deletion after a Python release).
    pub fn try_acquire(index_dir: &Path) -> Result<Option<Self>> {
        let path = index_dir.join(LOCK_FILE);
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file: Some(file), path })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// `FileLock.acquire(timeout)`: poll until acquired or the timeout passes.
    pub async fn acquire(index_dir: &Path, timeout: Duration) -> Result<Option<Self>> {
        let start = Instant::now();
        loop {
            if let Some(lock) = Self::try_acquire(index_dir)? {
                return Ok(Some(lock));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `WriteBatch.__enter__` lock loop: 4 attempts of `timeout` each with
    /// `uniform(0, min(10, 1 * 2**attempt))` seconds of sleep in between, then
    /// `SearchIndexLockError`.
    pub async fn acquire_with_retries(index_dir: &Path, timeout: Duration) -> Result<Self> {
        for attempt in 0..LOCK_RETRY_ATTEMPTS {
            if let Some(lock) = Self::acquire(index_dir, timeout).await? {
                return Ok(lock);
            }
            if attempt == LOCK_RETRY_ATTEMPTS - 1 {
                break;
            }
            let upper = LOCK_BACKOFF_CAP.min(LOCK_BACKOFF_BASE * 2f64.powi(attempt as i32));
            let sleep_s = rand::random::<f64>() * upper;
            tracing::debug!(
                "Index lock contention; retrying in {sleep_s:.2}s (attempt {}/{})",
                attempt + 1,
                LOCK_RETRY_ATTEMPTS
            );
            tokio::time::sleep(Duration::from_secs_f64(sleep_s)).await;
        }
        Err(IndexerError::LockExhausted {
            attempts: LOCK_RETRY_ATTEMPTS,
            timeout_secs: timeout.as_secs_f64(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IndexFileLock {
    fn drop(&mut self) {
        if let Some(f) = self.file.take() {
            // Closing the handle releases the lock as well; unlock explicitly first.
            let _ = f.unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exclusive_within_process() {
        let dir = tempfile::tempdir().unwrap();
        let a = IndexFileLock::try_acquire(dir.path()).unwrap();
        assert!(a.is_some());
        // a second handle on the same file must not get the lock
        assert!(IndexFileLock::try_acquire(dir.path()).unwrap().is_none());
        assert!(
            IndexFileLock::acquire(dir.path(), Duration::from_millis(120))
                .await
                .unwrap()
                .is_none()
        );
        drop(a);
        assert!(IndexFileLock::try_acquire(dir.path()).unwrap().is_some());
        assert!(dir.path().join(LOCK_FILE).exists());
    }
}
