//! Log files under `<home>/logs`:
//!
//! | File | Content |
//! |---|---|
//! | `desktop.log` | supervisor (this program), additionally on stderr |
//! | `postgresql.log` | server log of PostgreSQL (`pg_ctl -l`) |
//! | `server.log` | stdout+stderr of `paperrust serve --worker` (until 2026-11-16: `paperrust.log`) |
//! | `manage.log` | output of the `paperrust manage …` runs (migrate, createsuperuser, export) |
//! | `paperrust.log`, `mail.log` | logs of the UI (menu "Protokolle"), format and rotation as upstream (`PAPERRUST_LOGGING_DIR`, pl-server `logfile`); an older `paperless.log` stays readable |
//!
//! Rotation: `x.log` → `x.log.1` → … → `x.log.<keep>` (the oldest is dropped).
//!
//! Size limit [`MAX_BYTES`] per file (D2):
//! - `desktop.log` and `server.log` go through [`RotatingFile`] and rotate **during operation**
//!   as soon as they exceed the limit (the server's output goes through a pipe via
//!   the supervisor for this, see `server.rs`). At most `(KEEP + 1) × MAX_BYTES` per log.
//! - `desktop.log`/`server.log` additionally on every start.
//! - `manage.log`, `postgresql.log`, `pg_ctl.log` are written directly by the children; they are rotated before every
//!   run or PostgreSQL start if they are larger than the limit
//!   ([`rotate_if_larger`]). PostgreSQL only logs warnings (`log_min_messages = warning`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

pub const KEEP: usize = 5;
/// Size limit per log file (10 MiB); can be overridden with `PAPERRUST_DESKTOP_LOG_MAX_MB`.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Effective size limit (environment or [`MAX_BYTES`]).
pub fn max_bytes() -> u64 {
    std::env::var("PAPERRUST_DESKTOP_LOG_MAX_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&mb| mb > 0)
        .map_or(MAX_BYTES, |mb| mb * 1024 * 1024)
}

/// Rotates `path` if the file is larger than `max`.
pub fn rotate_if_larger(path: &Path, max: u64, keep: usize) -> io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > max => rotate(path, keep).map(|()| true),
        _ => Ok(false),
    }
}

/// Log file with size rotation during operation. Rotation only happens between two `write`s,
/// so a line/block always ends up entirely in one file.
pub struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    len: u64,
    max: u64,
    keep: usize,
}

impl RotatingFile {
    pub fn open(path: &Path, max: u64, keep: usize) -> io::Result<Self> {
        let file = open_append(path)?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            len,
            max,
            keep,
        })
    }

    fn rotate_now(&mut self) -> io::Result<()> {
        // Close first (Windows: renaming an open file is not allowed everywhere).
        self.file = None;
        rotate(&self.path, self.keep)?;
        self.file = Some(open_append(&self.path)?);
        self.len = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len + buf.len() as u64 > self.max {
            // Rotation failed (e.g. file locked by an editor): keep appending.
            if self.rotate_now().is_err() && self.file.is_none() {
                self.file = Some(open_append(&self.path)?);
            }
        }
        let f = match &mut self.file {
            Some(f) => f,
            None => self.file.insert(open_append(&self.path)?),
        };
        f.write_all(buf)?;
        self.len += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// Copies an output stream (stdout/stderr of a child) into a shared [`RotatingFile`].
pub async fn pump<R>(mut from: R, to: std::sync::Arc<Mutex<RotatingFile>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut f = to.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = f.write_all(&buf[..n]);
            }
        }
    }
    let mut f = to.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = f.flush();
}

/// Rotates `path` (if present) and keeps at most `keep` old files.
pub fn rotate(path: &Path, keep: usize) -> io::Result<()> {
    if keep == 0 {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let numbered = |n: usize| -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    };
    let _ = std::fs::remove_file(numbered(keep));
    for n in (1..keep).rev() {
        let from = numbered(n);
        if from.exists() {
            std::fs::rename(&from, numbered(n + 1))?;
        }
    }
    if path.exists() {
        std::fs::rename(path, numbered(1))?;
    }
    Ok(())
}

/// Open log file for appending; newly created on Unix with `0600` (M4: logs contain paths,
/// user names, document titles). Windows: ACL inherited from the data directory.
pub fn open_append(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Writes to stderr and to the file.
struct Tee {
    file: RotatingFile,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.file.flush()
    }
}

/// tracing → `desktop.log` + stderr. Filter from `PAPERRUST_DESKTOP_LOG` (default `info`).
pub fn init(log_dir: &Path) -> io::Result<PathBuf> {
    let path = log_dir.join("desktop.log");
    rotate(&path, KEEP)?;
    let file = RotatingFile::open(&path, max_bytes(), KEEP)?;
    let filter = std::env::var("PAPERRUST_DESKTOP_LOG")
        .ok()
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(Mutex::new(Tee { file }))
        .try_init();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_n() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.log");
        for i in 0..7 {
            std::fs::write(&p, format!("run {i}")).unwrap();
            rotate(&p, 3).unwrap();
        }
        assert!(!p.exists());
        let read =
            |n: usize| std::fs::read_to_string(dir.path().join(format!("x.log.{n}"))).unwrap();
        assert_eq!(read(1), "run 6");
        assert_eq!(read(2), "run 5");
        assert_eq!(read(3), "run 4");
        assert!(!dir.path().join("x.log.4").exists());
        // without an existing file: no error
        rotate(&dir.path().join("none.log"), 3).unwrap();
        std::fs::write(&p, "a").unwrap();
        rotate(&p, 0).unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn rotating_file_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.log");
        let mut f = RotatingFile::open(&p, 100, 2).unwrap();
        for i in 0..30 {
            writeln!(f, "zeile {i:03} ........").unwrap(); // 20 Bytes
        }
        f.flush().unwrap();
        drop(f);
        let len = |n: &str| std::fs::metadata(dir.path().join(n)).unwrap().len();
        assert!(len("r.log") <= 100, "{}", len("r.log"));
        assert!(len("r.log.1") <= 100 && len("r.log.2") <= 100);
        assert!(!dir.path().join("r.log.3").exists());
        let last = std::fs::read_to_string(&p).unwrap();
        assert!(last.ends_with("zeile 029 ........\n"), "{last}");
        assert!(!rotate_if_larger(&p, 1000, 2).unwrap());
        assert!(rotate_if_larger(&p, 10, 2).unwrap());
        assert!(!p.exists());
    }
}
