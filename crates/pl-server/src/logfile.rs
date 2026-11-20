//! Log files for the Logs view (`/api/logs/`): `paperrust.log` (upstream `paperless.log`), `mail.log` in
//! `PAPERLESS_LOGGING_DIR`, written like upstream's `LOGGING` dict-config.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`LOGGING`, `LOGROTATE_MAX_SIZE`,
//! `LOGROTATE_MAX_BACKUPS`), src/paperless/logging.py (`ConsumeTaskFormatter`).
//!
//! * Format `[{asctime}] [{levelname}] [{name}] {message}`; `asctime` = `2026-11-09 14:03:07,123`
//!   in `PAPERLESS_TIME_ZONE` (Django sets the process time zone from `TIME_ZONE`). The tracing
//!   *target* is the logger name (the ports log with `target: "paperless.consumer"` etc.); event
//!   fields other than the message are appended as ` key=value`. In the file the logger prefix
//!   `paperless` is shown as `paperrust` (`paperrust.consumer`, `paperrust_mail`; [`display_logger`]),
//!   the call sites keep the upstream names.
//! * Routing like upstream's `loggers`: `paperless`, `paperless_ai`, `_granian`, `granian.access`
//!   (and their children) at DEBUG and `ocrmypdf` at INFO → `paperrust.log`; `paperless_mail` →
//!   `mail.log`. Upstream's `celery.log` (loggers `celery`, `kombu`) does not exist in PaperRust
//!   (no Celery); an old `celery.log` taken over from paperless-ngx stays readable in the Logs
//!   view (pl-system). Other targets (Rust module paths, the access log `paperrust::access`) only
//!   reach the console as before.
//! * Rotation like `RotatingFileHandler`: before a record whose bytes would reach `maxBytes`, the
//!   file moves to `.1` (`.1` → `.2` … up to `backupCount`); `maxBytes` or `backupCount` 0 = never.
//!   Each record is appended with a fresh open (cheap at log volume) so that several processes
//!   (server + `--worker-only`) append to the current file instead of a
//!   rotated one; concurrent rotations by two processes may lose the order of a few lines (upstream
//!   uses a lock file for that).
//! * `paperrust.log` and `mail.log` are created at start (upstream's handlers open their files at
//!   configuration time).
//! * An older `paperless.log` (PaperRust before 2026-11-17, paperless-ngx) is left as it is and
//!   stays readable in the Logs view (pl-system `ALLOWED_LOG_FILES`).
//! * `PAPERRUST_LOG_FILES=0` switches the files off.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Upstream default `PAPERLESS_LOGROTATE_MAX_SIZE` (1 MiB).
pub const DEFAULT_MAX_SIZE: u64 = 1024 * 1024;
/// Upstream default `PAPERLESS_LOGROTATE_MAX_BACKUPS`.
pub const DEFAULT_MAX_BACKUPS: u32 = 20;

/// The upstream handlers PaperRust writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFile {
    /// `paperrust.log` (upstream handler `paperless`, file `paperless.log`).
    Main,
    Mail,
}

impl LogFile {
    pub fn file_name(self) -> &'static str {
        match self {
            LogFile::Main => "paperrust.log",
            LogFile::Mail => "mail.log",
        }
    }
}

fn is_logger_or_child(target: &str, logger: &str) -> bool {
    target == logger
        || (target.starts_with(logger) && target.as_bytes().get(logger.len()) == Some(&b'.'))
}

/// Which file a record of logger `target` at `level` goes to (upstream `LOGGING["loggers"]`).
pub fn route(target: &str, level: &Level) -> Option<LogFile> {
    for logger in ["paperless", "paperless_ai", "_granian", "granian.access"] {
        if is_logger_or_child(target, logger) {
            return Some(LogFile::Main);
        }
    }
    if is_logger_or_child(target, "ocrmypdf") {
        // level INFO: DEBUG/TRACE are dropped (tracing orders TRACE > DEBUG > INFO …)
        return (*level <= Level::INFO).then_some(LogFile::Main);
    }
    if is_logger_or_child(target, "paperless_mail") {
        return Some(LogFile::Mail);
    }
    None
}

/// Python `levelname`.
pub fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARNING",
        Level::INFO => "INFO",
        // Python has no TRACE; the logger level DEBUG lets it through as DEBUG.
        Level::DEBUG | Level::TRACE => "DEBUG",
    }
}

/// Settings of the file handlers.
#[derive(Debug, Clone)]
pub struct LogFileConfig {
    pub dir: PathBuf,
    pub max_bytes: u64,
    pub backups: u32,
    pub time_zone: Option<chrono_tz::Tz>,
}

impl LogFileConfig {
    /// From the pl-core configuration (`logging_dir`, `time_zone`) and
    /// `PAPERLESS_LOGROTATE_MAX_SIZE` / `PAPERLESS_LOGROTATE_MAX_BACKUPS` (invalid → default).
    pub fn from_core(core: &pl_core::Config) -> Self {
        let max_bytes = core.env.int("PAPERLESS_LOGROTATE_MAX_SIZE").ok().flatten();
        let backups = core
            .env
            .int("PAPERLESS_LOGROTATE_MAX_BACKUPS")
            .ok()
            .flatten();
        LogFileConfig {
            dir: core.logging_dir.clone(),
            max_bytes: max_bytes.map_or(DEFAULT_MAX_SIZE, |v| u64::try_from(v).unwrap_or(0)),
            backups: backups.map_or(DEFAULT_MAX_BACKUPS, |v| u32::try_from(v).unwrap_or(0)),
            time_zone: core.time_zone.parse().ok(),
        }
    }
}

/// One rotating file (`RotatingFileHandler`).
#[derive(Debug)]
struct Rotating {
    path: PathBuf,
    max_bytes: u64,
    backups: u32,
}

impl Rotating {
    fn backup(&self, n: u32) -> PathBuf {
        let mut s = self.path.clone().into_os_string();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    }

    /// `shouldRollover` + `doRollover` + `emit`.
    fn write(&self, line: &[u8]) -> std::io::Result<()> {
        if self.max_bytes > 0 && self.backups > 0 {
            let size = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
            if size > 0 && size + line.len() as u64 >= self.max_bytes {
                self.rollover();
            }
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line)
    }

    fn rollover(&self) {
        for i in (1..self.backups).rev() {
            let src = self.backup(i);
            if src.exists() {
                let dst = self.backup(i + 1);
                let _ = fs::remove_file(&dst);
                let _ = fs::rename(&src, &dst);
            }
        }
        let first = self.backup(1);
        let _ = fs::remove_file(&first);
        let _ = fs::rename(&self.path, &first);
    }
}

/// The tracing layer writing the upstream log files.
#[derive(Debug)]
pub struct LogFilesLayer {
    main: Rotating,
    mail: Rotating,
    time_zone: Option<chrono_tz::Tz>,
    /// Serialises writers of this process (rotation + append).
    lock: Mutex<()>,
}

impl LogFilesLayer {
    /// Create `dir` (upstream: `LOGGING_DIR.mkdir(parents=True, exist_ok=True)`) and the files
    /// `paperrust.log` and `mail.log`.
    pub fn new(cfg: &LogFileConfig) -> std::io::Result<Self> {
        fs::create_dir_all(&cfg.dir)?;
        let file = |f: LogFile| Rotating {
            path: cfg.dir.join(f.file_name()),
            max_bytes: cfg.max_bytes,
            backups: cfg.backups,
        };
        let layer = LogFilesLayer {
            main: file(LogFile::Main),
            mail: file(LogFile::Mail),
            time_zone: cfg.time_zone,
            lock: Mutex::new(()),
        };
        for f in [&layer.main, &layer.mail] {
            OpenOptions::new().create(true).append(true).open(&f.path)?;
        }
        Ok(layer)
    }

    fn file(&self, f: LogFile) -> &Rotating {
        match f {
            LogFile::Main => &self.main,
            LogFile::Mail => &self.mail,
        }
    }

    fn asctime(&self) -> String {
        const FMT: &str = "%Y-%m-%d %H:%M:%S,%3f";
        let now = chrono::Utc::now();
        match self.time_zone {
            Some(tz) => now.with_timezone(&tz).format(FMT).to_string(),
            None => now.with_timezone(&chrono::Local).format(FMT).to_string(),
        }
    }
}

/// Logger name as shown in the files: the upstream prefix `paperless` (`paperless.consumer`,
/// `paperless_mail`, `paperless_ai`) becomes `paperrust`; other names stay.
pub fn display_logger(target: &str) -> std::borrow::Cow<'_, str> {
    match target.strip_prefix("paperless") {
        Some(rest) if rest.is_empty() || rest.starts_with(['.', '_']) => format!("paperrust{rest}").into(),
        _ => target.into(),
    }
}

/// Console format like `tracing_subscriber`'s default (`<RFC 3339 UTC>  LEVEL span{…}: target:
/// message key=value`) but with the logger shown via [`display_logger`] (`paperrust.consumer`
/// instead of `paperless.consumer`). ANSI colours only when the writer supports them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut w: tracing_subscriber::fmt::format::Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        use tracing_subscriber::fmt::FormatFields as _;
        let meta = event.metadata();
        let ansi = w.has_ansi_escapes();
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ");
        let level = match *meta.level() {
            Level::TRACE => ("\x1b[35m", "TRACE"),
            Level::DEBUG => ("\x1b[34m", "DEBUG"),
            Level::INFO => ("\x1b[32m", " INFO"),
            Level::WARN => ("\x1b[33m", " WARN"),
            Level::ERROR => ("\x1b[31m", "ERROR"),
        };
        if ansi {
            write!(w, "\x1b[2m{ts}\x1b[0m {}{}\x1b[0m ", level.0, level.1)?;
        } else {
            write!(w, "{ts} {} ", level.1)?;
        }
        if let Some(scope) = ctx.event_scope() {
            let mut any = false;
            for span in scope.from_root() {
                if ansi {
                    write!(w, "\x1b[1m{}\x1b[0m", span.name())?;
                } else {
                    write!(w, "{}", span.name())?;
                }
                let ext = span.extensions();
                if let Some(f) =
                    ext.get::<tracing_subscriber::fmt::FormattedFields<N>>()
                    && !f.is_empty()
                {
                    write!(w, "{{{f}}}")?;
                }
                write!(w, ":")?;
                any = true;
            }
            if any {
                write!(w, " ")?;
            }
        }
        let target = display_logger(meta.target());
        if ansi {
            write!(w, "\x1b[2m{target}:\x1b[0m ")?;
        } else {
            write!(w, "{target}: ")?;
        }
        ctx.format_fields(w.by_ref(), event)?;
        writeln!(w)
    }
}

/// Formats one record: `[asctime] [LEVEL] [name] message key=value…\n`.
pub fn format_record(
    asctime: &str,
    level: &Level,
    name: &str,
    message: &str,
    fields: &str,
) -> String {
    format!(
        "[{asctime}] [{}] [{name}] {message}{fields}\n",
        level_name(level)
    )
}

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.rest, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else if !field.name().starts_with("log.") {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for LogFilesLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let Some(file) = route(meta.target(), meta.level()) else {
            return;
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let line = format_record(
            &self.asctime(),
            meta.level(),
            &display_logger(meta.target()),
            &fields.message,
            &fields.rest,
        );
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = self.file(file).write(line.as_bytes()) {
            // Never log from inside the logger (recursion); stderr like Python's handleError.
            eprintln!(
                "--- Logging error --- {}: {e}",
                self.file(file).path.display()
            );
        }
    }
}

/// Per-layer filter for [`LogFilesLayer`] (a plain `Layer::enabled` would also hide the records
/// from the console layer).
pub fn filter() -> tracing_subscriber::filter::FilterFn<impl Fn(&tracing::Metadata<'_>) -> bool> {
    tracing_subscriber::filter::filter_fn(|m| route(m.target(), m.level()).is_some())
}

/// The layer for this process, or `None` when switched off (`PAPERRUST_LOG_FILES=0`), the
/// configuration does not load (the caller reports that error itself) or the directory cannot
/// be created (warned on stderr).
pub fn from_env() -> Option<LogFilesLayer> {
    if pl_core::config::var("PAPERRUST_LOG_FILES").and_then(|v| pl_core::config::str_to_bool(&v))
        == Some(false)
    {
        return None;
    }
    let core = pl_core::Config::from_env().ok()?;
    let cfg = LogFileConfig::from_core(&core);
    match LogFilesLayer::new(&cfg) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!(
                "paperrust: log files in {} disabled: {e}",
                cfg.dir.display()
            );
            None
        }
    }
}

/// Lines of `dir/<name>` (test helper and diagnostics).
pub fn read_lines(dir: &Path, name: &str) -> Vec<String> {
    fs::read_to_string(dir.join(name))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_like_upstream_loggers() {
        assert_eq!(
            route("paperless.consumer", &Level::DEBUG),
            Some(LogFile::Main)
        );
        assert_eq!(route("paperless", &Level::TRACE), Some(LogFile::Main));
        assert_eq!(
            route("paperless.mail.tasks", &Level::INFO),
            Some(LogFile::Main)
        );
        assert_eq!(route("paperless_mail", &Level::DEBUG), Some(LogFile::Mail));
        assert_eq!(
            route("paperless_mail.mail", &Level::INFO),
            Some(LogFile::Mail)
        );
        assert_eq!(
            route("paperless_ai.client", &Level::INFO),
            Some(LogFile::Main)
        );
        assert_eq!(route("ocrmypdf", &Level::INFO), Some(LogFile::Main));
        assert_eq!(route("ocrmypdf._exec", &Level::DEBUG), None);
        assert_eq!(route("celery.worker", &Level::INFO), None);
        assert_eq!(route("paperlessx", &Level::INFO), None);
        assert_eq!(route("pl_consumer::run", &Level::ERROR), None);
        assert_eq!(route("paperrust::access", &Level::INFO), None);
    }

    #[test]
    fn rotation_like_rotating_file_handler() {
        let dir = tempfile::tempdir().unwrap();
        let r = Rotating {
            path: dir.path().join("paperrust.log"),
            max_bytes: 20,
            backups: 2,
        };
        for i in 0..5 {
            r.write(format!("line {i} 123456\n").as_bytes()).unwrap(); // 14 bytes each
        }
        assert_eq!(read_lines(dir.path(), "paperrust.log"), ["line 4 123456"]);
        assert_eq!(read_lines(dir.path(), "paperrust.log.1"), ["line 3 123456"]);
        assert_eq!(read_lines(dir.path(), "paperrust.log.2"), ["line 2 123456"]);
        assert!(!dir.path().join("paperrust.log.3").exists());
        // backupCount 0: never rotates
        let r = Rotating {
            path: dir.path().join("mail.log"),
            max_bytes: 20,
            backups: 0,
        };
        for _ in 0..3 {
            r.write(b"0123456789abcdef\n").unwrap();
        }
        assert_eq!(read_lines(dir.path(), "mail.log").len(), 3);
    }

    #[test]
    fn logger_prefix_shown_as_paperrust() {
        assert_eq!(display_logger("paperless.consumer"), "paperrust.consumer");
        assert_eq!(display_logger("paperless"), "paperrust");
        assert_eq!(display_logger("paperless_mail"), "paperrust_mail");
        assert_eq!(display_logger("paperless_ai.client"), "paperrust_ai.client");
        assert_eq!(display_logger("paperlessx"), "paperlessx");
        assert_eq!(display_logger("ocrmypdf"), "ocrmypdf");
    }

    #[test]
    fn format_matches_verbose_formatter() {
        assert_eq!(
            format_record(
                "2026-11-09 10:00:00,123",
                &Level::WARN,
                "paperless.consumer",
                "hi",
                " doc=1"
            ),
            "[2026-11-09 10:00:00,123] [WARNING] [paperless.consumer] hi doc=1\n"
        );
    }
}
