//! Single-instance lock: exclusive file lock (`File::try_lock`, std ≥ 1.89; Windows
//! `LockFileEx`, Unix `flock`). The operating system releases the lock when the process ends – even after
//! a crash, so there are no "orphaned" locks.
//!
//! The running instance writes URL and PID to `instance.toml` (a separate file, because Windows also blocks
//! readers of a locked file). A second instance reads it and only opens the URL.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DesktopError, Result, io_err};

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    info_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub pid: u32,
    pub url: String,
}

/// Result of a lock attempt.
#[derive(Debug)]
pub enum Acquire {
    Acquired(InstanceLock),
    /// Another instance is running; `info` if it has already published its URL.
    Busy(Option<InstanceInfo>),
}

impl InstanceLock {
    pub fn acquire(lock_path: &Path, info_path: &Path) -> Result<Acquire> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(io_err(format!("Sperrdatei {} öffnen", lock_path.display())))?;
        match file.try_lock() {
            Ok(()) => Ok(Acquire::Acquired(InstanceLock { _file: file, info_path: info_path.to_path_buf() })),
            Err(TryLockError::WouldBlock) => Ok(Acquire::Busy(read_info(info_path))),
            Err(TryLockError::Error(e)) => Err(io_err(format!("Sperrdatei {} sperren", lock_path.display()))(e)),
        }
    }

    /// Publish the URL of the running instance.
    pub fn publish(&self, url: &str) -> Result<()> {
        let info = InstanceInfo { pid: std::process::id(), url: url.to_string() };
        let text = toml::to_string(&info).map_err(|e| DesktopError::Other(e.to_string()))?;
        // D-N4: writable only by the user (Unix 0600; Windows: ACL of the data directory).
        crate::config::write_private(&self.info_path, text.as_bytes()).map_err(io_err(format!("{} schreiben", self.info_path.display())))
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // The OS itself releases the lock together with the handle.
        let _ = std::fs::remove_file(&self.info_path);
    }
}

/// Ends as soon as `path` exists (stop request from outside, see
/// [`crate::paths::AppPaths::stop_request_file`]); then deletes the file. Polled every 500 ms –
/// no platform API, also works without a console (service, installer, Tauri).
pub async fn stop_requested(path: PathBuf) {
    loop {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Result of [`request_stop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    NotRunning,
    Stopped,
    TimedOut,
}

/// Asks a running instance for an orderly shutdown and waits until its lock is free.
pub async fn request_stop(lock_path: &Path, info_path: &Path, stop_path: &Path, timeout: std::time::Duration) -> Result<StopOutcome> {
    if let Acquire::Acquired(l) = InstanceLock::acquire(lock_path, info_path)? {
        drop(l);
        return Ok(StopOutcome::NotRunning);
    }
    std::fs::write(stop_path, b"stop").map_err(io_err(format!("{} schreiben", stop_path.display())))?;
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if let Acquire::Acquired(l) = InstanceLock::acquire(lock_path, info_path)? {
            drop(l);
            let _ = std::fs::remove_file(stop_path);
            return Ok(StopOutcome::Stopped);
        }
        if start.elapsed() > timeout {
            return Ok(StopOutcome::TimedOut);
        }
    }
}

/// Reads `instance.toml` of the running instance. The URL is only accepted if it has exactly the
/// form `http://127.0.0.1:<port>/` (D-N4: it goes to `rundll32 url.dll`/`open`/`xdg-open`,
/// which would also open programs and files).
pub fn read_info(path: &Path) -> Option<InstanceInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let info: InstanceInfo = toml::from_str(&text).ok()?;
    if is_local_app_url(&info.url) {
        Some(info)
    } else {
        tracing::warn!(file = %path.display(), "instance.toml enthält keine lokale PaperRust-URL – ignoriert");
        None
    }
}

/// `http://127.0.0.1:<port>` with an optional trailing `/`, port 1–65535, nothing else.
pub fn is_local_app_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://127.0.0.1:") else {
        return false;
    };
    let port = rest.strip_suffix('/').unwrap_or(rest);
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_busy_until_drop() {
        let dir = tempfile::tempdir().unwrap();
        let (l, i) = (dir.path().join("instance.lock"), dir.path().join("instance.toml"));
        let Acquire::Acquired(first) = InstanceLock::acquire(&l, &i).unwrap() else { panic!("erste Sperre") };
        match InstanceLock::acquire(&l, &i).unwrap() {
            Acquire::Busy(None) => {}
            other => panic!("erwartet Busy(None), war {other:?}"),
        }
        first.publish("http://127.0.0.1:58080/").unwrap();
        match InstanceLock::acquire(&l, &i).unwrap() {
            Acquire::Busy(Some(info)) => {
                assert_eq!(info.url, "http://127.0.0.1:58080/");
                assert_eq!(info.pid, std::process::id());
            }
            other => panic!("erwartet Busy(Some), war {other:?}"),
        }
        drop(first);
        assert!(!i.exists());
        assert!(matches!(InstanceLock::acquire(&l, &i).unwrap(), Acquire::Acquired(_)));
    }

    #[test]
    fn only_local_urls_from_instance_file() {
        assert!(is_local_app_url("http://127.0.0.1:58080/"));
        assert!(is_local_app_url("http://127.0.0.1:1"));
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            r"C:\Windows\System32\calc.exe",
            "http://127.0.0.1:58080/../x",
            "http://127.0.0.1:58080/@evil",
            "http://127.0.0.1.evil.com:80/",
            "https://127.0.0.1:58080/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:99999/",
            "http://127.0.0.1:/",
            "http://localhost:58080/",
        ] {
            assert!(!is_local_app_url(bad), "{bad}");
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("instance.toml");
        std::fs::write(&f, "pid = 1\nurl = \"file:///C:/x.exe\"\n").unwrap();
        assert_eq!(read_info(&f), None);
        std::fs::write(&f, "pid = 1\nurl = \"http://127.0.0.1:5/\"\n").unwrap();
        assert_eq!(read_info(&f).map(|i| i.url).as_deref(), Some("http://127.0.0.1:5/"));
    }

    #[tokio::test]
    async fn stop_request_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (l, i, s) = (dir.path().join("instance.lock"), dir.path().join("instance.toml"), dir.path().join("stop.request"));
        let t = std::time::Duration::from_secs(10);
        assert_eq!(request_stop(&l, &i, &s, t).await.unwrap(), StopOutcome::NotRunning);
        let Acquire::Acquired(held) = InstanceLock::acquire(&l, &i).unwrap() else { panic!() };
        let watcher = tokio::spawn({
            let s = s.clone();
            async move {
                stop_requested(s).await;
                drop(held);
            }
        });
        assert_eq!(request_stop(&l, &i, &s, t).await.unwrap(), StopOutcome::Stopped);
        watcher.await.unwrap();
        assert!(!s.exists());
    }
}
