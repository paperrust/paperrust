//! `FileLock(settings.MEDIA_LOCK)` (filelock 4.x): exclusive lock on `MEDIA_ROOT/media.lock`,
//! compatible with the Python process (Unix `flock`, Windows `LockFileEx`) and the server
//! (`pl_consumer::medialock`, BEFUNDE B1).
//!
//! Deviation from upstream (`timeout=-1`, waits forever): same implementation as in the server – with
//! a timeout (`PAPERRUST_MEDIA_LOCK_TIMEOUT`, seconds, default 120, `0` = no limit) and a clear
//! error message, asynchronous (does not block a Tokio thread, e.g. `empty_trash` in the worker).
//!
//! Lock order as in the server: first lock document rows
//! ([`pl_consumer::medialock::lock_document_rows`]), then `MEDIA_LOCK`; under `MEDIA_LOCK` never wait for
//! a foreign row lock.

use std::path::Path;

pub struct MediaLock {
    inner: Option<pl_consumer::medialock::MediaLock>,
    path: std::path::PathBuf,
}

impl MediaLock {
    /// Waits at most the configured timeout (`ErrorKind::TimedOut` afterwards).
    pub async fn acquire(path: &Path) -> std::io::Result<MediaLock> {
        let inner = pl_consumer::medialock::MediaLock::acquire(path).await?;
        Ok(MediaLock { inner: Some(inner), path: path.to_path_buf() })
    }
}

impl Drop for MediaLock {
    fn drop(&mut self) {
        drop(self.inner.take());
        // filelock (WindowsFileLock) deletes the lock file on release, UnixFileLock does not
        if cfg!(windows) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held lock (other handle, e.g. server/Python): waiting ends with a timeout error,
    /// after release it succeeds.
    #[tokio::test]
    async fn times_out_while_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.lock");
        let held = pl_consumer::medialock::MediaLock::acquire_with(&path, None).await.unwrap();
        let t = std::time::Instant::now();
        let r = pl_consumer::medialock::MediaLock::acquire_with(&path, Some(std::time::Duration::from_millis(200))).await;
        assert_eq!(r.err().map(|e| e.kind()), Some(std::io::ErrorKind::TimedOut));
        assert!(t.elapsed() >= std::time::Duration::from_millis(150));
        drop(held);
        let l = MediaLock::acquire(&path).await.unwrap();
        drop(l);
    }
}
