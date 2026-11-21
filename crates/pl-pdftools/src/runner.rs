//! Server side: starts `paperrust-pdftool` as a child process, sends a [`Job`] via stdin
//! and reads exactly one reply from stdout.
//!
//! Isolation (mandatory, docs/contract/pl-pdftools.md §2):
//! * own process – a crash, stack overflow or `abort` in the PdfCraft code only terminates the child;
//! * time limit – after `timeout` the child is terminated (`kill`, additionally `kill_on_drop`);
//! * memory limit – the child counts its heap itself (own allocator) and aborts above
//!   `memory_mb`; the server only notices a failed run;
//! * output limit – more than [`MAX_REPLY`] bytes on stdout count as a protocol error;
//! * no secrets – the child gets an emptied environment (only `PATH`/`SystemRoot`/`TEMP`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::job::{Job, Op, Reply};

/// Name of the executable (without `.exe`).
pub const TOOL_NAME: &str = "paperrust-pdftool";
/// At most this many bytes of reply.
pub const MAX_REPLY: usize = 4 * 1024 * 1024;
/// Default time limit per run.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Default memory limit of the child process.
pub const DEFAULT_MEMORY_MB: u64 = 1536;

/// Error of a run. `Display` is for logs; users only see [`RunError::user_message`].
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RunError {
    #[error("PDF tool not installed ({TOOL_NAME} not found)")]
    NotInstalled,
    #[error("PDF tool timed out after {0} s")]
    Timeout(u64),
    #[error("PDF tool ended abnormally: {0}")]
    Crashed(String),
    #[error("PDF tool protocol error: {0}")]
    Protocol(String),
    #[error("PDF tool could not be started: {0}")]
    Io(String),
    /// Domain error, reported by the child (text without paths).
    #[error("{0}")]
    Failed(String),
}

impl RunError {
    /// Text for API responses (no paths, no internals).
    pub fn user_message(&self) -> String {
        match self {
            RunError::NotInstalled => {
                "The PDF tools are not installed on this server (paperrust-pdftool is missing). Please ask the administrator to install it next to paperrust.".into()
            }
            RunError::Timeout(s) => format!("The PDF could not be processed within {s} seconds."),
            RunError::Crashed(_) | RunError::Protocol(_) | RunError::Io(_) => {
                "The PDF could not be processed safely (the PDF tool stopped). The file may be damaged or too large.".into()
            }
            RunError::Failed(m) => m.clone(),
        }
    }
}

/// Limits of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    pub memory_mb: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS), memory_mb: DEFAULT_MEMORY_MB }
    }
}

fn parse_u64(v: Option<String>) -> Option<u64> {
    v.and_then(|s| s.trim().parse::<u64>().ok())
}

impl Limits {
    /// `PAPERRUST_PDFTOOL_TIMEOUT` (seconds, ≥ 1) and `PAPERRUST_PDFTOOL_MEMORY_MB` (0 = no
    /// limit), live from the environment or UI.
    pub fn from_env() -> Self {
        let d = Limits::default();
        let timeout =
            parse_u64(pl_core::config::var("PAPERRUST_PDFTOOL_TIMEOUT")).filter(|s| *s >= 1).map(Duration::from_secs).unwrap_or(d.timeout);
        let memory_mb = parse_u64(pl_core::config::var("PAPERRUST_PDFTOOL_MEMORY_MB")).unwrap_or(d.memory_mb);
        Limits { timeout, memory_mb }
    }
}

fn exe_name() -> String {
    format!("{TOOL_NAME}{}", std::env::consts::EXE_SUFFIX)
}

/// Where the tool was searched for and whether it is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLocation {
    /// Gefunden.
    Found(PathBuf),
    /// `PAPERRUST_PDFTOOL_BIN` does not point to a file.
    ConfiguredMissing(PathBuf),
    /// Not next to the running executable (checked locations).
    NotFound(Vec<PathBuf>),
}

impl ToolLocation {
    pub fn path(&self) -> Option<&Path> {
        match self {
            ToolLocation::Found(p) => Some(p),
            _ => None,
        }
    }

    /// Hint for logs and operations (where it was searched, what to do).
    pub fn hint(&self) -> String {
        match self {
            ToolLocation::Found(p) => format!("{TOOL_NAME} found at {}", p.display()),
            ToolLocation::ConfiguredMissing(p) => {
                format!("PAPERRUST_PDFTOOL_BIN points to {}, which does not exist; fix the path or unset it", p.display())
            }
            ToolLocation::NotFound(c) => format!(
                "{TOOL_NAME} is not installed (looked in: {}); place it next to paperrust or set PAPERRUST_PDFTOOL_BIN",
                c.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

/// Search rule without environment (testable): `configured` = value of `PAPERRUST_PDFTOOL_BIN`, `exe` = running
/// executable. Without setting: next to `exe`, for `…/deps/` (cargo test runs) also one level up.
pub fn locate(configured: Option<&str>, exe: Option<&Path>) -> ToolLocation {
    if let Some(c) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let p = PathBuf::from(c);
        return if p.is_file() { ToolLocation::Found(p) } else { ToolLocation::ConfiguredMissing(p) };
    }
    let mut candidates = Vec::new();
    if let Some(dir) = exe.and_then(Path::parent) {
        candidates.push(dir.join(exe_name()));
        if dir.file_name().is_some_and(|n| n == "deps")
            && let Some(up) = dir.parent()
        {
            candidates.push(up.join(exe_name()));
        }
    }
    match candidates.iter().find(|p| p.is_file()) {
        Some(p) => ToolLocation::Found(p.clone()),
        None => ToolLocation::NotFound(candidates),
    }
}

/// Search the executable: `PAPERRUST_PDFTOOL_BIN`, otherwise next to the running executable.
pub fn tool_location() -> ToolLocation {
    let configured = pl_core::config::var("PAPERRUST_PDFTOOL_BIN");
    let exe = std::env::current_exe().ok();
    locate(configured.as_deref(), exe.as_deref())
}

/// Path of the tool, if installed.
pub fn tool_path() -> Option<PathBuf> {
    tool_location().path().map(Path::to_path_buf)
}

/// Is the tool installed?
pub fn available() -> bool {
    tool_path().is_some()
}

/// Run with the found tool and limits from the environment. If the tool is missing:
/// [`RunError::NotInstalled`] and a warning with the place where it was searched.
pub async fn run(op: Op) -> Result<Reply, RunError> {
    run_located(&tool_location(), op, Limits::from_env()).await
}

/// Like [`run`] with an explicit search result (testable).
pub async fn run_located(loc: &ToolLocation, op: Op, limits: Limits) -> Result<Reply, RunError> {
    match loc.path() {
        Some(tool) => run_with(tool, op, limits).await,
        None => {
            tracing::warn!(target: "paperrust.pdftools", "{}", loc.hint());
            Err(RunError::NotInstalled)
        }
    }
}

/// Run with an explicit tool and limits. Never returns `Reply::Failed` – that becomes
/// [`RunError::Failed`].
pub async fn run_with(tool: &Path, op: Op, limits: Limits) -> Result<Reply, RunError> {
    let job = Job { op, memory_mb: limits.memory_mb };
    let input = serde_json::to_vec(&job).map_err(|e| RunError::Protocol(e.to_string()))?;
    let mut cmd = tokio::process::Command::new(tool);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // no secrets to the child: empty the environment, keep only the bare minimum
    cmd.env_clear();
    for k in KEEP_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: no console window in the desktop app
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| RunError::Io(e.to_string()))?;
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err(RunError::Io("pipes not available".into()));
    };
    let work = async {
        // write and read at the same time (large replies would otherwise block the child)
        let write = async move {
            let r = stdin.write_all(&input).await;
            drop(stdin);
            r
        };
        let read_out = async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            loop {
                let n = stdout.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                if buf.len() + n > MAX_REPLY {
                    return Err(std::io::Error::other("reply too large"));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok::<_, std::io::Error>(buf)
        };
        let read_err = async move {
            // keep only the last bytes (debugging), never unbounded
            let mut tail = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&chunk[..n]);
                if tail.len() > 8192 {
                    let cut = tail.len() - 8192;
                    tail.drain(..cut);
                }
            }
            tail
        };
        let (w, out, err) = tokio::join!(write, read_out, read_err);
        let status = child.wait().await;
        (w, out, err, status)
    };
    let (w, out, err, status) = match tokio::time::timeout(limits.timeout, work).await {
        Ok(v) => v,
        Err(_) => {
            // Future dropped → terminate and reap the child (kill_on_drop as a second safeguard).
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            tracing::warn!(target: "paperrust.pdftools", "PDF tool timed out after {} s, killed", limits.timeout.as_secs());
            return Err(RunError::Timeout(limits.timeout.as_secs()));
        }
    };
    let status = status.map_err(|e| RunError::Io(e.to_string()))?;
    let err_text = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        tracing::warn!(target: "paperrust.pdftools", "PDF tool ended with {status}: {err_text}");
        return Err(RunError::Crashed(format!("{status}")));
    }
    if let Err(e) = w {
        return Err(RunError::Protocol(format!("writing job: {e}")));
    }
    let out = out.map_err(|e| RunError::Protocol(e.to_string()))?;
    let reply: Reply = serde_json::from_slice(&out).map_err(|e| RunError::Protocol(format!("bad reply: {e}")))?;
    match reply {
        Reply::Failed { error } => Err(RunError::Failed(error)),
        r => Ok(r),
    }
}

/// Environment variables the child may keep (no secrets).
const KEEP_ENV: &[&str] = &[
    "PATH",
    "SystemRoot",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "RUST_BACKTRACE",
    "WINDIR",
    "LOCALAPPDATA",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "HOME",
];

/// Run without tokio (for synchronous callers such as the parsers during ingestion, which run in
/// `spawn_blocking`) with the found tool and limits from the environment.
pub fn run_blocking(op: Op) -> Result<Reply, RunError> {
    let tool = tool_path().ok_or(RunError::NotInstalled)?;
    run_blocking_with(&tool, op, Limits::from_env())
}

/// Like [`run_with`], but with `std::process` and helper threads instead of tokio. Same isolation:
/// own process, emptied environment, time limit (child is terminated), output limit.
pub fn run_blocking_with(tool: &Path, op: Op, limits: Limits) -> Result<Reply, RunError> {
    use std::io::{Read, Write};
    let job = Job { op, memory_mb: limits.memory_mb };
    let input = serde_json::to_vec(&job).map_err(|e| RunError::Protocol(e.to_string()))?;
    let mut cmd = std::process::Command::new(tool);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.env_clear();
    for k in KEEP_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| RunError::Io(e.to_string()))?;
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RunError::Io("pipes not available".into()));
    };
    let writer = std::thread::spawn(move || {
        let r = stdin.write_all(&input);
        drop(stdin);
        r
    });
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = stdout.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            if buf.len() + n > MAX_REPLY {
                return Err(std::io::Error::other("reply too large"));
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Ok::<_, std::io::Error>(buf)
    });
    let err_reader = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..n]);
            if tail.len() > 8192 {
                let cut = tail.len() - 8192;
                tail.drain(..cut);
            }
        }
        tail
    });
    let deadline = std::time::Instant::now() + limits.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // pipes are closed now, the helper threads end by themselves
                let _ = writer.join();
                let _ = reader.join();
                let _ = err_reader.join();
                tracing::warn!(target: "paperrust.pdftools", "PDF tool timed out after {} s, killed", limits.timeout.as_secs());
                return Err(RunError::Timeout(limits.timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::Io(e.to_string()));
            }
        }
    };
    let w = writer.join().unwrap_or_else(|_| Err(std::io::Error::other("writer thread failed")));
    let out = reader.join().unwrap_or_else(|_| Err(std::io::Error::other("reader thread failed")));
    let err = err_reader.join().unwrap_or_default();
    let err_text = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        tracing::warn!(target: "paperrust.pdftools", "PDF tool ended with {status}: {err_text}");
        return Err(RunError::Crashed(format!("{status}")));
    }
    if let Err(e) = w {
        return Err(RunError::Protocol(format!("writing job: {e}")));
    }
    let out = out.map_err(|e| RunError::Protocol(e.to_string()))?;
    let reply: Reply = serde_json::from_slice(&out).map_err(|e| RunError::Protocol(format!("bad reply: {e}")))?;
    match reply {
        Reply::Failed { error } => Err(RunError::Failed(error)),
        r => Ok(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_have_no_internals() {
        let m = RunError::Crashed("exit code: 0xc00000fd".into()).user_message();
        assert!(!m.contains("0xc0"), "{m}");
        assert_eq!(RunError::Failed("page 1 is unreadable".into()).user_message(), "page 1 is unreadable");
        assert!(RunError::Timeout(5).user_message().contains('5'));
    }

    #[test]
    fn limits_default() {
        let l = Limits::default();
        assert_eq!(l.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        assert_eq!(parse_u64(Some(" 12 ".into())), Some(12));
        assert_eq!(parse_u64(Some("x".into())), None);
    }

    #[test]
    fn locate_reports_where_it_looked() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("deps").join("paperrust.exe");
        // nothing there: both places (next to exe, one level up) named
        let loc = locate(None, Some(&exe));
        let ToolLocation::NotFound(c) = &loc else { panic!("{loc:?}") };
        assert_eq!(c.len(), 2);
        assert!(loc.hint().contains("PAPERRUST_PDFTOOL_BIN") && loc.hint().contains("not installed"), "{}", loc.hint());
        // set, but missing
        let missing = t.path().join("nope.exe");
        let loc = locate(Some(missing.to_str().unwrap()), Some(&exe));
        assert_eq!(loc, ToolLocation::ConfiguredMissing(missing));
        assert!(loc.hint().contains("does not exist"));
        // found next to the executable; an empty setting does not count
        std::fs::create_dir_all(t.path().join("deps")).unwrap();
        let tool = t.path().join(exe_name());
        std::fs::write(&tool, b"x").unwrap();
        assert_eq!(locate(Some("  "), Some(&exe)), ToolLocation::Found(tool));
        assert_eq!(locate(None, None), ToolLocation::NotFound(vec![]));
    }

    #[tokio::test]
    async fn missing_tool_is_not_installed_with_clear_message() {
        let loc = ToolLocation::NotFound(vec![PathBuf::from("x/paperrust-pdftool")]);
        let e = run_located(&loc, Op::SelfTest { mode: crate::job::SelfTest::Panic }, Limits::default()).await.unwrap_err();
        assert_eq!(e, RunError::NotInstalled);
        let m = e.user_message();
        assert!(m.contains("not installed") && m.contains("paperrust-pdftool"), "{m}");
        assert!(!m.contains("x/"), "no paths for users: {m}");
    }

    #[test]
    fn missing_tool_is_io_error_blocking() {
        let r = run_blocking_with(
            Path::new("definitely-not-here-pdftool.exe"),
            Op::SelfTest { mode: crate::job::SelfTest::Panic },
            Limits::default(),
        );
        assert!(matches!(r, Err(RunError::Io(_))), "{r:?}");
    }

    #[tokio::test]
    async fn missing_tool_is_io_error() {
        let r =
            run_with(Path::new("definitely-not-here-pdftool.exe"), Op::SelfTest { mode: crate::job::SelfTest::Panic }, Limits::default())
                .await;
        assert!(matches!(r, Err(RunError::Io(_))), "{r:?}");
    }
}
