//! `check [--fail-level LEVEL]` – system checks (B5).
//!
//! upstream: src/paperless/checks.py@v3.4.2 (`path_check`/`paths_check`, `binaries_check`,
//! `debug_mode_check`, `settings_values_check`, `audit_log_check`,
//! `check_v3_minimum_upgrade_version`, `check_deprecated_db_settings`,
//! `check_deprecated_v2_ocr_env_vars`, `check_default_language_available`),
//! src/documents/checks.py@v3.4.2 (`parser_check`, `filename_format_check`), output like
//! django/core/management/base.py (`BaseCommand.check`: Gruppen CRITICALS/ERRORS/WARNINGS/INFOS/
//! DEBUGS, `?: (id) msg\n\tHINT: hint`, "System check identified …", from `--fail-level` (default
//! ERROR) `SystemCheckError` on stderr with exit 1).
//!
//! Deviations (docs/contract/pl-cli.md): the binaries are the ones PaperRust needs (Tesseract,
//! PDFium) instead of `gs`/`pdftoppm`/`pdfinfo`; database errors are reported as errors instead of a
//! traceback; the remaining Django checks (models, `--deploy`) do not exist.

use std::path::{Path, PathBuf};

use crate::args::{self, flag, value};
use crate::{CliError, CliResult};

/// Django `checks.messages` levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 10,
    Info = 20,
    Warning = 30,
    Error = 40,
    Critical = 50,
}

impl Level {
    fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "CRITICAL" => Level::Critical,
            "ERROR" => Level::Error,
            "WARNING" => Level::Warning,
            "INFO" => Level::Info,
            "DEBUG" => Level::Debug,
            _ => return None,
        })
    }
    fn group(self) -> &'static str {
        match self {
            Level::Critical => "CRITICALS",
            Level::Error => "ERRORS",
            Level::Warning => "WARNINGS",
            Level::Info => "INFOS",
            Level::Debug => "DEBUGS",
        }
    }
}

/// `django.core.checks.CheckMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    pub msg: String,
    pub hint: Option<String>,
    pub id: Option<&'static str>,
}

impl Issue {
    fn new(level: Level, msg: impl Into<String>) -> Self {
        Self { level, msg: msg.into(), hint: None, id: None }
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    fn id(mut self, id: &'static str) -> Self {
        self.id = Some(id);
        self
    }
}

impl std::fmt::Display for Issue {
    /// `CheckMessage.__str__` (obj `None` → `?`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "?: ")?;
        if let Some(id) = self.id {
            write!(f, "({id}) ")?;
        }
        write!(f, "{}", self.msg)?;
        if let Some(h) = &self.hint {
            write!(f, "\n\tHINT: {h}")?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------
// paperless/checks.py
// ------------------------------------------------------------------------------------------

/// `path_check(var, directory)`.
pub fn path_check(var: &str, directory: &Path) -> Vec<Issue> {
    if directory.as_os_str().is_empty() {
        return Vec::new();
    }
    if !directory.is_dir() {
        return vec![Issue::new(Level::Error, format!("{var} is set but doesn't exist."))
            .hint(format!("Create a directory at {}", directory.display()))];
    }
    let test_file = directory.join(format!("__paperless_write_test_{}__", std::process::id()));
    let mut out = Vec::new();
    if std::fs::File::create(&test_file).is_err() {
        let mode = file_mode(directory);
        out.push(Issue::new(Level::Error, format!("{var} is not writeable")).hint(format!(
            "Set the permissions of {} to be writeable by the user running the Paperless services",
            format_args!("\n{mode}   {}\n", directory.display())
        )));
    }
    if test_file.is_file() {
        let _ = std::fs::remove_file(&test_file);
    }
    out
}

/// `stat.filemode(st_mode)` (Unix only; otherwise `?`).
fn file_mode(p: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(m) = std::fs::metadata(p) else { return "?".into() };
        let mode = m.permissions().mode();
        let mut s = String::from(if m.is_dir() { "d" } else { "-" });
        for shift in [6u32, 3, 0] {
            let b = (mode >> shift) & 7;
            s.push(if b & 4 != 0 { 'r' } else { '-' });
            s.push(if b & 2 != 0 { 'w' } else { '-' });
            s.push(if b & 1 != 0 { 'x' } else { '-' });
        }
        s
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        "?".into()
    }
}

/// `paths_check`.
pub fn paths_check(cfg: &pl_core::Config) -> Vec<Issue> {
    let mut v = path_check("PAPERLESS_DATA_DIR", &cfg.data_dir);
    if let Some(t) = &cfg.empty_trash_dir {
        v.extend(path_check("PAPERLESS_EMPTY_TRASH_DIR", t));
    }
    v.extend(path_check("PAPERLESS_MEDIA_ROOT", &cfg.media_root));
    v.extend(path_check("PAPERLESS_CONSUMPTION_DIR", &cfg.consumption_dir));
    v
}

/// `shutil.which(name)` (PATH, on Windows with PATHEXT).
pub fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD".into()).split(';').map(str::to_string).collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let c = dir.join(format!("{name}{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// `binaries_check` – PaperRust binaries: Tesseract (`PAPERRUST_TESSERACT_PATH` or PATH) and
/// PDFium (`PAPERRUST_PDFIUM_PATH` or default search). Level WARNING like upstream.
pub fn binaries_check(tesseract: Option<PathBuf>, pdfium_ok: bool) -> Vec<Issue> {
    let hint = "Either it's not in your ${PATH} or it's not installed.";
    let mut v = Vec::new();
    if tesseract.is_none() {
        v.push(Issue::new(Level::Warning, "PaperRust can't find tesseract. Without it, consumption is impossible.").hint(hint));
    }
    if !pdfium_ok {
        v.push(
            Issue::new(Level::Warning, "PaperRust can't find PDFium. Without it, PDF pages cannot be rendered for OCR and thumbnails.")
                .hint("Install the PDFium library or set PAPERRUST_PDFIUM_PATH."),
        );
    }
    v
}

/// `debug_mode_check`.
pub fn debug_mode_check(debug: bool) -> Vec<Issue> {
    if !debug {
        return Vec::new();
    }
    vec![Issue::new(
        Level::Warning,
        "DEBUG mode is enabled. Disable Debug mode. This is a serious security issue, since it puts security overrides in place which are meant to be only used during development. This also means that paperless will tell anyone various debugging information when something goes wrong.",
    )]
}

/// `settings_values_check` (values like `settings.py` with default values).
pub fn settings_values_check(get: &dyn Fn(&str) -> Option<String>, time_zone: &str, email_cert: Option<&Path>) -> Vec<Issue> {
    let mut v = Vec::new();
    let output_type = get("PAPERLESS_OCR_OUTPUT_TYPE").unwrap_or_else(|| "pdfa".into());
    if !["pdfa", "pdf", "pdfa-1", "pdfa-2", "pdfa-3"].contains(&output_type.as_str()) {
        v.push(Issue::new(Level::Error, format!("OCR output type \"{output_type}\" is not valid")));
    }
    let mode_raw = get("PAPERLESS_OCR_MODE").unwrap_or_default();
    let mode = if mode_raw.is_empty() || mode_raw == "skip" || mode_raw == "skip_noarchive" { "auto".to_string() } else { mode_raw };
    if !["auto", "force", "redo", "off"].contains(&mode.as_str()) {
        v.push(Issue::new(Level::Error, format!("OCR output mode \"{mode}\" is not valid")));
    }
    let generation = get("PAPERLESS_ARCHIVE_FILE_GENERATION").filter(|s| !s.is_empty()).unwrap_or_else(|| "auto".into());
    if !["auto", "always", "never"].contains(&generation.as_str()) {
        v.push(Issue::new(Level::Error, format!("PAPERLESS_ARCHIVE_FILE_GENERATION setting \"{generation}\" is not valid")));
    }
    let clean = get("PAPERLESS_OCR_CLEAN").unwrap_or_else(|| "clean".into());
    if !["clean", "clean-final", "none"].contains(&clean.as_str()) {
        v.push(Issue::new(Level::Error, format!("OCR clean mode \"{clean}\" is not valid")));
    }
    if time_zone.parse::<chrono_tz::Tz>().is_err() {
        v.push(Issue::new(Level::Error, format!("Timezone \"{time_zone}\" is not a valid timezone")));
    }
    if let Some(c) = email_cert
        && !c.is_file()
    {
        v.push(Issue::new(Level::Error, format!("Email cert {} is not a file", c.display())));
    }
    v
}

/// `audit_log_check`.
pub fn audit_log_check(table_exists: bool, audit_enabled: bool) -> Vec<Issue> {
    if table_exists && !audit_enabled {
        return vec![Issue::new(Level::Warning, "auditlog table was found but audit log is disabled.")];
    }
    Vec::new()
}

/// `check_v3_minimum_upgrade_version` – `applied` = applied `documents` migrations
/// (`None`: no table `django_migrations`).
pub fn v3_minimum_upgrade_check(applied: Option<&[String]>) -> Vec<Issue> {
    let Some(applied) = applied else { return Vec::new() };
    if applied.is_empty()
        || applied.iter().any(|m| m == "0001_squashed" || m == "0002_squashed" || m == "1075_workflowaction_order")
    {
        return Vec::new();
    }
    let mut sorted = applied.to_vec();
    sorted.sort();
    let last = sorted.last().cloned().unwrap_or_else(|| "(none)".into());
    tracing::error!(
        target: "paperless.checks",
        "V3 upgrade check failed: last applied documents migration is '{last}'. Expected '1075_workflowaction_order' (v2.20.15). Ensure you have upgraded to v2.20.15 and run 'manage.py migrate' before upgrading to v3."
    );
    vec![Issue::new(Level::Error, "Cannot upgrade to Paperless-ngx v3 from this version.")
        .hint("Upgrading to v3 can only be performed from v2.20.15. Please upgrade to v2.20.15, run migrations, then upgrade to v3. See https://docs.paperless-ngx.com/setup/#upgrading for details.")
        .id("paperless.E002")]
}

/// `check_deprecated_db_settings` (process environment only, `os.getenv`).
pub fn deprecated_db_settings_check(getenv: &dyn Fn(&str) -> Option<String>) -> Vec<Issue> {
    ["PAPERLESS_DB_TIMEOUT", "PAPERLESS_DB_POOLSIZE", "PAPERLESS_DBSSLMODE", "PAPERLESS_DBSSLROOTCERT", "PAPERLESS_DBSSLCERT", "PAPERLESS_DBSSLKEY"]
        .iter()
        .filter(|n| getenv(n).is_some_and(|v| !v.is_empty()))
        .map(|n| {
            Issue::new(Level::Warning, format!("Deprecated environment variable: {n}"))
                .hint(format!(
                    "{n} is deprecated. Set the equivalent option via PAPERLESS_DB_OPTIONS instead. See https://docs.paperless-ngx.com/migration-v3/ for the full reference."
                ))
                .id("paperless.W001")
        })
        .collect()
}

/// `check_deprecated_v2_ocr_env_vars`.
pub fn deprecated_v2_ocr_check(getenv: &dyn Fn(&str) -> Option<String>) -> Vec<Issue> {
    let mut v = Vec::new();
    if getenv("PAPERLESS_OCR_SKIP_ARCHIVE_FILE").is_some_and(|s| !s.is_empty()) {
        v.push(
            Issue::new(
                Level::Warning,
                "PAPERLESS_OCR_SKIP_ARCHIVE_FILE is set but has no effect. Use PAPERLESS_ARCHIVE_FILE_GENERATION=never/always/auto instead.",
            )
            .id("paperless.W002"),
        );
    }
    let mode = getenv("PAPERLESS_OCR_MODE").unwrap_or_default();
    if mode == "skip" || mode == "skip_noarchive" {
        v.push(
            Issue::new(
                Level::Warning,
                format!(
                    "PAPERLESS_OCR_MODE='{mode}' is not a valid value. Use PAPERLESS_OCR_MODE=auto (and PAPERLESS_ARCHIVE_FILE_GENERATION=never if you used skip_noarchive) instead."
                ),
            )
            .id("paperless.W003"),
        );
    }
    v
}

/// `check_default_language_available`; `installed` = `tesseract --list-langs` (None: no
/// Tesseract, then `binaries_check` reports it).
pub fn default_language_check(ocr_language: &str, installed: Option<&[String]>) -> Vec<Issue> {
    if ocr_language.is_empty() {
        return vec![Issue::new(
            Level::Warning,
            "No OCR language has been specified with PAPERLESS_OCR_LANGUAGE. This means that tesseract will fallback to english.",
        )];
    }
    let Some(installed) = installed else { return Vec::new() };
    ocr_language
        .split('+')
        .map(str::trim)
        .filter(|l| !installed.iter().any(|i| i == l))
        .map(|l| {
            Issue::new(
                Level::Error,
                format!("The selected ocr language {l} is not installed. Paperless cannot OCR your documents without it. Please fix PAPERLESS_OCR_LANGUAGE."),
            )
        })
        .collect()
}

/// `get_tesseract_langs()`.
fn tesseract_langs(program: &Path) -> Option<Vec<String>> {
    let out = std::process::Command::new(program).arg("--list-langs").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.trim().split('\n').skip(1).map(|l| l.trim().to_string()).collect())
}

// ------------------------------------------------------------------------------------------
// documents/checks.py
// ------------------------------------------------------------------------------------------

/// `filename_format_check`.
pub fn filename_format_check(filename_format: Option<&str>) -> Vec<Issue> {
    let Some(f) = filename_format.filter(|f| !f.is_empty()) else { return Vec::new() };
    let converted = pl_templating::convert_format_str_to_template_format(f);
    if converted != f {
        return vec![Issue::new(
            Level::Warning,
            format!("Filename format {f} is using the old style, please update to use double curly brackets"),
        )
        .hint(converted)];
    }
    Vec::new()
}

// ------------------------------------------------------------------------------------------
// Output (BaseCommand.check)
// ------------------------------------------------------------------------------------------

/// `(text, serious?)`: `serious` = at least one message ≥ `fail_level` → `SystemCheckError`.
pub fn render(issues: &[Issue], fail_level: Level) -> (String, bool) {
    let header = if issues.is_empty() { String::new() } else { "System check identified some issues:\n".to_string() };
    let mut body = String::new();
    for level in [Level::Critical, Level::Error, Level::Warning, Level::Info, Level::Debug] {
        let mut formatted: Vec<String> = issues.iter().filter(|i| i.level == level).map(|i| i.to_string()).collect();
        if formatted.is_empty() {
            continue;
        }
        formatted.sort();
        body.push_str(&format!("\n{}:\n{}\n", level.group(), formatted.join("\n")));
    }
    let n = issues.len();
    let count = match n {
        0 => "no issues".to_string(),
        1 => "1 issue".to_string(),
        n => format!("{n} issues"),
    };
    let mut footer = String::new();
    if n > 0 {
        footer.push('\n');
    }
    footer.push_str(&format!("System check identified {count} (0 silenced)."));
    let serious = issues.iter().any(|i| i.level >= fail_level);
    if serious {
        (format!("SystemCheckError: {header}{body}{footer}"), true)
    } else {
        (format!("{header}{body}{footer}"), false)
    }
}

const OPTS: &[args::Opt] = &[
    value(&["--fail-level"], "fail_level"),
    value(&["--database"], "database"),
    flag(&["--deploy"], "deploy"),
    value(&["-t", "--tag"], "tag"),
];

/// All checks (order like the registration in Django; the output is sorted anyway).
pub async fn collect() -> Vec<Issue> {
    let mut v = Vec::new();
    let cfg = match pl_core::config::Config::from_env() {
        Ok(c) => c,
        Err(e) => return vec![Issue::new(Level::Critical, format!("Configuration error: {e}"))],
    };
    v.extend(paths_check(&cfg));
    let tess = std::env::var_os("PAPERRUST_TESSERACT_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| which("tesseract"));
    v.extend(binaries_check(tess.clone(), pl_ocr::render::PdfiumRenderer::from_env().is_ok()));
    v.extend(debug_mode_check(cfg.debug));
    let get = |k: &str| pl_core::config::Env::from_process().get(k).map(str::to_string);
    v.extend(settings_values_check(&get, &cfg.time_zone, cfg.email_certificate_file.as_deref()));
    match crate::ctx::load_config() {
        Err(e) => v.push(Issue::new(Level::Error, e.to_string())),
        Ok(c) => match crate::ctx::connect_pool(&c).await {
            Err(e) => v.push(Issue::new(Level::Error, e.to_string())),
            Ok(pool) => {
                let audit: Option<bool> = sqlx::query_scalar("SELECT to_regclass('auditlog_logentry') IS NOT NULL").fetch_one(&pool).await.ok();
                v.extend(audit_log_check(audit.unwrap_or(false), cfg.audit_log_enabled));
                let has_mig: bool = sqlx::query_scalar("SELECT to_regclass('django_migrations') IS NOT NULL").fetch_one(&pool).await.unwrap_or(false);
                if has_mig {
                    // DatabaseError → [] like upstream
                    if let Ok(applied) = sqlx::query_scalar::<_, String>("SELECT name FROM django_migrations WHERE app = 'documents'").fetch_all(&pool).await {
                        v.extend(v3_minimum_upgrade_check(Some(&applied)));
                    }
                }
            }
        },
    }
    let getenv = |k: &str| std::env::var(k).ok();
    v.extend(deprecated_db_settings_check(&getenv));
    v.extend(deprecated_v2_ocr_check(&getenv));
    let lang = get("PAPERLESS_OCR_LANGUAGE").unwrap_or_else(|| "eng".into());
    let installed = tess.as_deref().and_then(tesseract_langs);
    v.extend(default_language_check(&lang, installed.as_deref()));
    v.extend(filename_format_check(cfg.filename_format.as_deref()));
    v
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, usize::MAX)?;
    let fail_level = match a.value("fail_level") {
        None => Level::Error,
        Some(s) => Level::parse(s).ok_or_else(|| {
            CliError::Usage(format!("argument --fail-level: invalid choice: '{s}' (choose from 'CRITICAL', 'ERROR', 'WARNING', 'INFO', 'DEBUG')"))
        })?,
    };
    let issues = collect().await;
    let (msg, serious) = render(&issues, fail_level);
    if serious {
        return Err(CliError::Raw(msg));
    }
    if issues.is_empty() {
        println!("{msg}");
    } else {
        eprintln!("{msg}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn django_output_format() {
        let issues = vec![
            Issue::new(Level::Warning, "b warn").hint("do x"),
            Issue::new(Level::Error, "an error"),
            Issue::new(Level::Warning, "a warn").id("paperless.W001"),
        ];
        let (msg, serious) = render(&issues, Level::Error);
        assert!(serious);
        assert_eq!(
            msg,
            "SystemCheckError: System check identified some issues:\n\nERRORS:\n?: an error\n\nWARNINGS:\n?: (paperless.W001) a warn\n?: b warn\n\tHINT: do x\n\nSystem check identified 3 issues (0 silenced)."
        );
        let (msg, serious) = render(&issues[..1], Level::Error);
        assert!(!serious);
        assert!(msg.ends_with("System check identified 1 issue (0 silenced)."));
        assert_eq!(render(&[], Level::Error).0, "System check identified no issues (0 silenced).");
        assert!(render(&issues[..1], Level::Warning).1);
    }

    #[test]
    fn paths() {
        let t = tempfile::tempdir().unwrap();
        assert!(path_check("PAPERLESS_DATA_DIR", t.path()).is_empty());
        let missing = t.path().join("nope");
        let v = path_check("PAPERLESS_DATA_DIR", &missing);
        assert_eq!(v[0].msg, "PAPERLESS_DATA_DIR is set but doesn't exist.");
        assert_eq!(v[0].hint.as_deref(), Some(format!("Create a directory at {}", missing.display()).as_str()));
        // no test file is left behind
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn settings_values() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert!(settings_values_check(&env(&[]), "UTC", None).is_empty());
        let v = settings_values_check(
            &env(&[("PAPERLESS_OCR_OUTPUT_TYPE", "tiff"), ("PAPERLESS_OCR_MODE", "skip"), ("PAPERLESS_OCR_CLEAN", "x"), ("PAPERLESS_ARCHIVE_FILE_GENERATION", "sometimes")]),
            "Mars/Base",
            Some(Path::new("/definitely/not/here.pem")),
        );
        let msgs: Vec<&str> = v.iter().map(|i| i.msg.as_str()).collect();
        assert_eq!(
            msgs,
            vec![
                "OCR output type \"tiff\" is not valid",
                "PAPERLESS_ARCHIVE_FILE_GENERATION setting \"sometimes\" is not valid",
                "OCR clean mode \"x\" is not valid",
                "Timezone \"Mars/Base\" is not a valid timezone",
                "Email cert /definitely/not/here.pem is not a file",
            ]
        );
    }

    #[test]
    fn deprecated_and_upgrade_and_language() {
        let env = |k: &str| match k {
            "PAPERLESS_DB_POOLSIZE" => Some("4".to_string()),
            "PAPERLESS_OCR_MODE" => Some("skip_noarchive".to_string()),
            _ => None,
        };
        let w = deprecated_db_settings_check(&env);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].id, Some("paperless.W001"));
        let w = deprecated_v2_ocr_check(&env);
        assert_eq!(w[0].id, Some("paperless.W003"));
        assert!(w[0].msg.starts_with("PAPERLESS_OCR_MODE='skip_noarchive' is not a valid value."));
        assert!(v3_minimum_upgrade_check(None).is_empty());
        assert!(v3_minimum_upgrade_check(Some(&["0001_squashed".to_string()])).is_empty());
        assert_eq!(v3_minimum_upgrade_check(Some(&["1074_x".to_string()]))[0].id, Some("paperless.E002"));
        let langs = vec!["eng".to_string(), "deu".to_string()];
        assert!(default_language_check("deu+eng", Some(&langs)).is_empty());
        assert_eq!(default_language_check("deu+fra", Some(&langs)).len(), 1);
        assert_eq!(default_language_check("", None)[0].level, Level::Warning);
        assert!(audit_log_check(true, true).is_empty());
        assert_eq!(audit_log_check(true, false).len(), 1);
    }

    #[test]
    fn filename_format_old_style() {
        assert!(filename_format_check(Some("{{ title }}")).is_empty());
        let w = filename_format_check(Some("{title}"));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].hint.as_deref(), Some("{{ title }}"));
    }
}
