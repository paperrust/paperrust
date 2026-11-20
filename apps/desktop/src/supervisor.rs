//! Flow (docs/DESKTOP.md "Start-Ablauf"):
//!
//! 1. Directories, single-instance lock (second instance → open the first one's URL, exit)
//! 2. Load/create `desktop.toml`, check the installation layout
//! 3. PostgreSQL: choose port, `initdb` if needed, start (health: `pg_isready`), `createdb` if needed
//! 4. First start with an empty database: choice ([`AdminPrompt::choose_setup`]) – start fresh,
//!    import a paperless-ngx export, or adopt an existing database (`paperrust-migrate-db`,
//!    **before** `migrate`, the target must be empty)
//! 5. `paperrust manage migrate` (on **every** start – idempotent, covers updates)
//! 6. Import: `manage document_importer <folder|ZIP>` (after `migrate`, as upstream)
//! 7. No superuser → local owner without a dialog (operating system user name, random
//!    password) → `manage createsuperuser --no-input`; owner for automatic login in
//!    `desktop.toml` (`[setup] owner_user`), for existing installations the first active superuser
//! 8. Start `paperrust serve --worker`, health `GET /__rs/health`, publish the URL
//! 9. wait for the shell's quit request, backup request ([`Control`]) or unexpected end
//!    of the server (up to 3 restarts)
//! 10. Shutdown: wait for a running backup, server (stdin EOF/SIGINT, grace period), then
//!     `pg_ctl stop -m fast`
//!
//! PostgreSQL is stopped again on **every** error path after step 3.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::config::DesktopConfig;
use crate::layout::InstallLayout;
use crate::lock::{Acquire, InstanceLock};
use crate::paths::AppPaths;
use crate::pg::PgCluster;
use crate::ports::pick_port;
use crate::server::{ServerSpec, base_url, server_env};
use crate::{DesktopError, Result};

/// Credentials for the first superuser (only from the environment now, managed installations).
#[derive(Clone)]
pub struct AdminCredentials {
    pub username: String,
    pub email: String,
    pub password: String,
}

/// Choice in the first-start form (docs/DESKTOP.md section 8).
#[derive(Clone)]
pub enum SetupChoice {
    /// Start fresh. `None` (default): create a local owner without a dialog (name from the
    /// operating system, random password). `Some`: these credentials (`PAPERRUST_DESKTOP_ADMIN_*`).
    Fresh(Option<AdminCredentials>),
    /// Export from paperless-ngx (`document_exporter`, folder or ZIP) → `manage document_importer`.
    ImportExport { source: PathBuf },
    /// Adopt an existing paperless-ngx database (2.20 or 3.x; SQLite/MariaDB/MySQL/PostgreSQL)
    /// (`paperrust-migrate-db`), optionally copy the old media folder (`MEDIA_ROOT`).
    MigrateDb {
        source_url: String,
        media_dir: Option<PathBuf>,
    },
}

impl SetupChoice {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fresh(_) => "fresh",
            Self::ImportExport { .. } => "import",
            Self::MigrateDb { .. } => "migrate-db",
        }
    }
}

/// What the first-start form may offer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetupOptions {
    /// `paperrust-migrate-db` is included.
    pub can_migrate_db: bool,
}

/// First-start choice of the shell (console, Tauri form). `None` = aborted.
pub trait AdminPrompt {
    /// First start with an empty database: start fresh or adopt data. Default: start
    /// fresh with an automatically created owner.
    fn choose_setup(&self, _options: &SetupOptions) -> Option<SetupChoice> {
        Some(SetupChoice::Fresh(None))
    }
}

/// First start without a dialog (scripts, tests, managed installations):
/// `PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}`.
pub fn admin_from_env(get: impl Fn(&str) -> Option<String>) -> Option<AdminCredentials> {
    Some(AdminCredentials {
        username: get("PAPERRUST_DESKTOP_ADMIN_USER")?,
        email: get("PAPERRUST_DESKTOP_ADMIN_EMAIL").unwrap_or_default(),
        password: get("PAPERRUST_DESKTOP_ADMIN_PASSWORD")?,
    })
}

/// Like [`admin_from_env`], plus `PAPERRUST_DESKTOP_IMPORT=<source>` (is detected, see
/// [`detect_setup`]: export folder/ZIP, data folder with SQLite or database URL) or
/// `PAPERRUST_DESKTOP_IMPORT_DB=<URL>` (+ optionally `PAPERRUST_DESKTOP_IMPORT_MEDIA=<folder>`).
pub fn setup_from_env(get: impl Fn(&str) -> Option<String>) -> Option<SetupChoice> {
    let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
    if let Some(p) = get("PAPERRUST_DESKTOP_IMPORT") {
        let media = get("PAPERRUST_DESKTOP_IMPORT_MEDIA").map(PathBuf::from);
        // Not recognized → treated as an export; `validate_setup` then reports the reason.
        return Some(
            detect_setup(&p, media).unwrap_or(SetupChoice::ImportExport {
                source: PathBuf::from(p.trim()),
            }),
        );
    }
    if let Some(u) = get("PAPERRUST_DESKTOP_IMPORT_DB") {
        return Some(SetupChoice::MigrateDb {
            source_url: u,
            media_dir: get("PAPERRUST_DESKTOP_IMPORT_MEDIA").map(PathBuf::from),
        });
    }
    admin_from_env(get).map(|c| SetupChoice::Fresh(Some(c)))
}

/// Switching with one click (package Z1): detects the path from **one** input – same rules
/// as `paperrust-migrate-db detect` (crates/pl-dbmigrate/src/detect.rs):
/// folder with `manifest.json` or ZIP → export import; data folder with `db.sqlite3`,
/// paperless folder with `data/db.sqlite3` or the SQLite file → database migration (the media folder
/// next to it is found); `sqlite:`/`postgres://`/`mysql://`/`mariadb://` → database migration.
/// The migration state (2.20, 3.0–3.4.2 or known newer) is checked by `paperrust-migrate-db` during the migration.
pub fn detect_setup(
    input: &str,
    media: Option<PathBuf>,
) -> std::result::Result<SetupChoice, String> {
    let input = input.trim().trim_matches('"');
    if input.is_empty() {
        return Err("Bitte Export, Datenordner oder Datenbank-URL angeben.".into());
    }
    let lower = input.to_ascii_lowercase();
    if [
        "sqlite:",
        "postgres://",
        "postgresql://",
        "mysql://",
        "mariadb://",
    ]
    .iter()
    .any(|s| lower.starts_with(s))
    {
        let media =
            media.or_else(|| sqlite_path(input).and_then(|p| p.parent().and_then(guess_media)));
        return Ok(SetupChoice::MigrateDb {
            source_url: input.to_string(),
            media_dir: media,
        });
    }
    let p = PathBuf::from(input);
    let is_zip = |p: &Path| {
        let mut b = [0u8; 4];
        std::fs::File::open(p)
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
            .is_ok()
            && b == *b"PK\x03\x04"
    };
    let is_sqlite = |p: &Path| {
        let mut b = [0u8; 16];
        std::fs::File::open(p)
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
            .is_ok()
            && &b == b"SQLite format 3\0"
    };
    let db = |file: &Path, media: Option<PathBuf>| SetupChoice::MigrateDb {
        source_url: sqlite_url(file),
        media_dir: media.or_else(|| file.parent().and_then(guess_media)),
    };
    if p.is_file() {
        if is_zip(&p) {
            return Ok(SetupChoice::ImportExport { source: p });
        }
        if is_sqlite(&p) {
            return Ok(db(&p, media));
        }
        return Err(format!(
            "{} ist weder ein ZIP-Export noch eine SQLite-Datenbank.",
            p.display()
        ));
    }
    if p.is_dir() {
        if p.join("manifest.json").is_file() {
            return Ok(SetupChoice::ImportExport { source: p });
        }
        for f in [p.join("db.sqlite3"), p.join("data").join("db.sqlite3")] {
            if f.is_file() {
                return Ok(db(&f, media));
            }
        }
        return Err(format!(
            "{} enthält weder manifest.json (Export) noch db.sqlite3 bzw. data/db.sqlite3.",
            p.display()
        ));
    }
    Err(format!("{} nicht gefunden.", p.display()))
}

/// Media folder next to a data folder (`<root>/data` + `<root>/media`).
fn guess_media(data_dir: &Path) -> Option<PathBuf> {
    [
        Some(data_dir.join("media")),
        data_dir.parent().map(|p| p.join("media")),
    ]
    .into_iter()
    .flatten()
    .find(|m| m.join("documents").is_dir())
}

fn sqlite_url(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("sqlite://{s}")
    } else {
        format!("sqlite:///{s}")
    }
}

/// Validation of the dialog inputs (Django rules roughly: user name `[\w.@+-]`, ≤ 150 characters;
/// password ≥ 8 characters – pl-cli does not check more strictly, upstream with --no-input neither).
pub fn validate_admin(c: &AdminCredentials) -> std::result::Result<(), &'static str> {
    if c.username.is_empty()
        || c.username.chars().count() > 150
        || !c
            .username
            .chars()
            .all(|ch| ch.is_alphanumeric() || "_.@+-".contains(ch))
    {
        return Err("Benutzername: 1–150 Zeichen, Buchstaben, Ziffern und _ . @ + -");
    }
    if c.password.chars().count() < 8 {
        return Err("Passwort: mindestens 8 Zeichen");
    }
    if !c.email.is_empty() && !c.email.contains('@') {
        return Err("E-Mail-Adresse ungültig");
    }
    Ok(())
}

/// Path of a SQLite source (`sqlite:///C:/x/db.sqlite3`, `sqlite:///home/x/db.sqlite3`).
fn sqlite_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("sqlite://")?;
    let rest = rest.split('?').next().unwrap_or(rest);
    // `sqlite:///C:/…` → `C:/…`; `sqlite:////abs` or `sqlite:///abs` (Unix) → `/abs`
    let p = match rest.strip_prefix('/') {
        Some(r) if r.len() > 1 && r.as_bytes().get(1) == Some(&b':') => r,
        Some(_) => rest,
        None => rest,
    };
    Some(PathBuf::from(p))
}

/// Validation of the first-start choice before submitting (form) or before execution.
pub fn validate_setup(c: &SetupChoice, o: &SetupOptions) -> std::result::Result<(), String> {
    match c {
        SetupChoice::Fresh(Some(a)) => validate_admin(a).map_err(str::to_string),
        SetupChoice::Fresh(None) => Ok(()),
        SetupChoice::ImportExport { source } => {
            if source.is_dir() {
                if source.join("manifest.json").is_file() {
                    Ok(())
                } else {
                    Err(format!(
                        "{} enthält keine manifest.json – ist das ein Export von paperless-ngx (document_exporter)?",
                        source.display()
                    ))
                }
            } else if source.is_file() {
                if source
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
                {
                    Ok(())
                } else {
                    Err("Bitte einen Export-Ordner oder eine ZIP-Datei angeben.".into())
                }
            } else {
                Err(format!("{} nicht gefunden.", source.display()))
            }
        }
        SetupChoice::MigrateDb {
            source_url,
            media_dir,
        } => {
            if !o.can_migrate_db {
                return Err("Dieses Paket enthält paperrust-migrate-db nicht.".into());
            }
            let ok_scheme = [
                "sqlite://",
                "mysql://",
                "mariadb://",
                "postgres://",
                "postgresql://",
            ]
            .iter()
            .any(|s| source_url.starts_with(s));
            if !ok_scheme {
                return Err(
                    "Quelle: sqlite:///C:/pfad/db.sqlite3, mysql://…, mariadb://… oder postgres://…"
                        .into(),
                );
            }
            if let Some(p) = sqlite_path(source_url)
                && !p.is_file()
            {
                return Err(format!("SQLite-Datei {} nicht gefunden.", p.display()));
            }
            match media_dir {
                Some(m) if !m.is_dir() => {
                    Err(format!("Medienordner {} nicht gefunden.", m.display()))
                }
                _ => Ok(()),
            }
        }
    }
}

/// Result of a backup during operation (tray "Sicherung jetzt").
pub type BackupReply = tokio::sync::oneshot::Sender<Result<PathBuf>>;

/// Control of a running instance by the shell.
#[derive(Default)]
pub struct Control {
    backups: Option<tokio::sync::mpsc::UnboundedReceiver<BackupReply>>,
}

/// Sender for backup requests + the matching [`Control`] for [`Supervisor::run_with_control`].
pub fn control_channel() -> (tokio::sync::mpsc::UnboundedSender<BackupReply>, Control) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (tx, Control { backups: Some(rx) })
}

impl Control {
    /// Next backup request; never if there is no channel (or after it has ended).
    async fn next_backup(&mut self) -> BackupReply {
        if let Some(rx) = &mut self.backups {
            if let Some(r) = rx.recv().await {
                return r;
            }
            self.backups = None;
        }
        std::future::pending().await
    }
}

/// Events for the shell (status line, tray tooltip, splash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Stage(&'static str),
    Ready { url: String },
    /// Reconciliation after "Switch from paperless-ngx": running (reported before `Ready`) or finished.
    SwitchCheck(SwitchCheck),
    Restarting { attempt: u32 },
    BackupStarted,
    Stopping,
}

/// Result of the reconciliation after switching (first start "import export" / "adopt
/// database"): `paperrust-migrate-db verify` writes `logs/umstieg-bericht.{json,html,txt}`; the
/// shell shows the verdict and a short summary and, on request, opens the full report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchCheck {
    /// `RUNNING` (reconciliation still running) | `OK` | `WARN` | `FAIL`
    pub verdict: String,
    /// Short summary in plain words (field `summary` of the JSON report), otherwise a note
    pub summary: Vec<String>,
    /// full report (HTML), if written
    pub report: Option<PathBuf>,
}

impl SwitchCheck {
    pub fn running() -> Self {
        Self {
            verdict: "RUNNING".into(),
            summary: vec!["Der Abgleich zwischen paperless-ngx und PaperRust läuft …".into()],
            report: None,
        }
    }

    /// Read from `<logs>/umstieg-bericht.json`; `exit` = exit code of `verify` (0 OK, 3 WARN, otherwise FAIL)
    /// as a fallback if the report is missing or unreadable.
    pub fn from_report(logs: &Path, exit: Option<i32>) -> Self {
        let by_exit = match exit {
            Some(0) => "OK",
            Some(3) => "WARN",
            _ => "FAIL",
        };
        let html = logs.join("umstieg-bericht.html");
        let report = html.is_file().then_some(html);
        let json: Option<serde_json::Value> = std::fs::read_to_string(logs.join("umstieg-bericht.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let Some(j) = json else {
            return Self {
                verdict: by_exit.into(),
                summary: vec!["Der Prüfbericht konnte nicht gelesen werden – Details in logs/migrate-db.log.".into()],
                report,
            };
        };
        let verdict = j["verdict"].as_str().filter(|v| matches!(*v, "OK" | "WARN" | "FAIL")).unwrap_or(by_exit).to_string();
        let mut summary: Vec<String> = j["summary"]
            .as_array()
            .map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        // the first findings (WARN/FAIL) in addition, at most three
        if let Some(f) = j["findings"].as_array() {
            for x in f.iter().filter(|x| matches!(x["level"].as_str(), Some("WARN" | "FAIL"))).take(3) {
                if let Some(m) = x["message"].as_str() {
                    summary.push(format!("{}: {m}", x["level"].as_str().unwrap_or("")));
                }
            }
        }
        Self { verdict, summary, report }
    }
}

pub struct Supervisor {
    pub paths: AppPaths,
    pub layout: InstallLayout,
    pub cfg: DesktopConfig,
    /// `desktop.toml` was newly created at this start (first start).
    pub config_created: bool,
    /// Notes from [`Self::open`] (produced before the log start), e.g. a warning about the storage location;
    /// the shell logs/shows them.
    pub notices: Vec<String>,
    /// Login key of this run (automatic login, [`crate::autologin`]); never log.
    login_token: String,
    _lock: InstanceLock,
}

/// Result of [`Supervisor::open`].
pub enum Opened {
    Ready(Box<Supervisor>),
    /// An instance is already running; its URL (if known).
    AlreadyRunning(Option<String>),
}

const MIGRATE_TIMEOUT: Duration = Duration::from_secs(1800);
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(120);
const SERVER_STOP_GRACE: Duration = Duration::from_secs(20);
const MAX_RESTARTS: u32 = 3;
/// Exit code "restart wanted" of the server (`pl_settings::restart::EXIT_CODE`, docs/SETTINGS.md
/// E9): restart immediately, does not count towards [`MAX_RESTARTS`].
const RESTART_EXIT_CODE: i32 = 75;
const IMPORT_TIMEOUT: Duration = Duration::from_secs(24 * 3600);
/// Quitting during a backup: it is waited for this long, then aborted.
const BACKUP_SHUTDOWN_WAIT: Duration = Duration::from_secs(300);
/// Do not repeat invalid first-start inputs (e.g. from the environment) endlessly.
const MAX_SETUP_ATTEMPTS: u32 = 5;

/// Everything a backup needs – independent of the `Supervisor`, so it can run as its own task
/// alongside the running server.
#[derive(Clone)]
struct BackupJob {
    spec: ServerSpec,
    pg: PgCluster,
    db: String,
    dir: PathBuf,
}

impl BackupJob {
    /// `pg_dump -Fc` → `paperrust.pgdump` + `document_exporter` → `export/` (upstream-compatible,
    /// also importable into paperless-ngx). Also runs with the server running (pg_dump is consistent
    /// thanks to MVCC; the exporter reads DB + files).
    async fn run(&self) -> Result<PathBuf> {
        crate::perms::create_private_dir(&self.dir)
            .map_err(crate::io_err(format!("{} anlegen", self.dir.display())))?;
        self.pg
            .dump(&self.db, &self.dir.join("paperrust.pgdump"))
            .await?;
        let export = self.dir.join("export");
        crate::perms::create_private_dir(&export).map_err(crate::io_err("export anlegen"))?;
        let export_s = export.display().to_string();
        let st = self
            .spec
            .manage(
                &["document_exporter", &export_s],
                &[],
                Duration::from_secs(6 * 3600),
            )
            .await?;
        if !st.success() {
            return Err(DesktopError::Server(format!(
                "document_exporter fehlgeschlagen ({st}); Details in logs/manage.log"
            )));
        }
        tracing::info!(dir = %self.dir.display(), "Sicherung fertig");
        Ok(self.dir.clone())
    }
}

/// Copy a folder recursively; existing target files are kept (no overwriting). Symlinks
/// are skipped. Returns: number of copied files.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let ty = e.file_type()?;
        let dst = to.join(e.file_name());
        if ty.is_dir() {
            n += copy_tree(&e.path(), &dst)?;
        } else if ty.is_file() && !dst.exists() {
            std::fs::copy(e.path(), &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

impl Supervisor {
    /// Steps 1–2.
    pub fn open(paths: AppPaths, layout: InstallLayout) -> Result<Opened> {
        paths.ensure_dirs()?;
        let lock = match InstanceLock::acquire(&paths.lock_file(), &paths.instance_file())? {
            Acquire::Acquired(l) => l,
            Acquire::Busy(info) => return Ok(Opened::AlreadyRunning(info.map(|i| i.url))),
        };
        // An old stop request (instance was already gone) must not end the new run immediately.
        let _ = std::fs::remove_file(paths.stop_request_file());
        // Old backup request of an ended instance: do not execute at the next start.
        let _ = std::fs::remove_file(paths.backup_request_file());
        // D-N2: do not leave behind the password file of a crashed initdb.
        let _ = std::fs::remove_file(paths.run.join(crate::pg::INITDB_PWFILE));
        let missing = layout.missing(|p| p.exists());
        if !missing.is_empty() {
            return Err(DesktopError::Other(format!(
                "Installation unvollständig, es fehlt: {}",
                missing.join(", ")
            )));
        }
        let (mut cfg, created) = DesktopConfig::load_or_create(&paths.config_file)?;
        let mut notices = Vec::new();
        // M4: check location, make the data directory accessible only to the user, once.
        if let Some(w) =
            crate::perms::location_warning(crate::paths::Os::current(), &paths.home, |k| {
                std::env::var(k).ok()
            })
        {
            notices.push(w);
        }
        let mut dirty = false;
        if !cfg.setup.home_protected {
            match crate::perms::protect_home(&paths.home) {
                Ok(()) => {
                    cfg.setup.home_protected = true;
                    dirty = true;
                }
                Err(e) => notices.push(format!(
                    "Rechte für {} ließen sich nicht einschränken: {e}",
                    paths.home.display()
                )),
            }
        }
        // D-N3: configuration from before the application role → generate the password now.
        if cfg.database.app_password.is_empty() {
            cfg.database.app_password = crate::config::random_secret(32);
            dirty = true;
        }
        if dirty {
            cfg.save(&paths.config_file)?;
        }
        Ok(Opened::Ready(Box::new(Self {
            paths,
            layout,
            cfg,
            config_created: created,
            notices,
            login_token: crate::autologin::generate_token(),
            _lock: lock,
        })))
    }

    /// What the first-start form can offer.
    pub fn setup_options(&self) -> SetupOptions {
        SetupOptions {
            can_migrate_db: self.layout.migrate_db_bin.is_some(),
        }
    }

    fn save(&self) -> Result<()> {
        self.cfg.save(&self.paths.config_file)
    }

    /// One-time move of `time_zone`/`ocr_language` into the database (docs/SETTINGS.md §2.6):
    /// `manage settings set --if-unset` (time zone → `paperrust_setting`, OCR language → AC column
    /// `language`), afterwards [`server_env`] no longer sets them as environment. Errors are not
    /// fatal (the next start tries again; until then they apply as before as environment).
    async fn migrate_settings(&mut self, pg: &PgCluster) {
        if self.cfg.setup.settings_migrated {
            return;
        }
        let mut spec = self.spec(pg.port, self.cfg.server.port);
        // Without the two variables – otherwise the keys would be locked as "from environment".
        for k in ["PAPERLESS_TIME_ZONE", "PAPERLESS_OCR_LANGUAGE"] {
            spec.env.remove(k);
        }
        let pairs = [
            ("PAPERLESS_TIME_ZONE", self.cfg.server.time_zone.clone()),
            ("PAPERLESS_OCR_LANGUAGE", self.cfg.server.ocr_language.clone()),
        ];
        for (k, v) in &pairs {
            match spec.manage(&["settings", "set", k, v, "--if-unset"], &[], Duration::from_secs(120)).await {
                Ok(st) if st.success() => {}
                Ok(st) => {
                    tracing::warn!(key = k, status = %st, "Einstellung nicht in die Datenbank übernommen; Details in logs/manage.log");
                    return;
                }
                Err(e) => {
                    tracing::warn!(key = k, error = %e, "Einstellung nicht in die Datenbank übernommen");
                    return;
                }
            }
        }
        self.cfg.setup.settings_migrated = true;
        match self.save() {
            Ok(()) => tracing::info!("Zeitzone und OCR-Sprache in die Datenbank übernommen (jetzt in der Oberfläche änderbar)"),
            Err(e) => tracing::warn!(error = %e, "desktop.toml nicht gespeichert"),
        }
    }

    /// Apply a change request from the UI (WP7). Returns: restart server?
    fn apply_settings_request(&mut self, req: std::result::Result<crate::settings_ctl::Request, String>) -> bool {
        use crate::settings_ctl::{Outcome, write_outcome, write_values};
        let req = match req {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Einstellungs-Anfrage verworfen");
                return false;
            }
        };
        let mut cfg = self.cfg.clone();
        let outcome = match crate::settings_ctl::apply(&mut cfg, &req.changes) {
            Ok(restart) => {
                let old = std::mem::replace(&mut self.cfg, cfg);
                match self.save() {
                    Ok(()) => {
                        tracing::info!(keys = ?req.changes.keys().collect::<Vec<_>>(), restart, "Desktop-Einstellungen geändert");
                        Outcome { id: req.id, ok: true, error: None, restart }
                    }
                    Err(e) => {
                        self.cfg = old;
                        Outcome { id: req.id, ok: false, error: Some(format!("desktop.toml: {e}")), restart: false }
                    }
                }
            }
            Err(e) => Outcome { id: req.id, ok: false, error: Some(e), restart: false },
        };
        write_values(&self.paths.run, &self.cfg);
        write_outcome(&self.paths.run, &outcome);
        outcome.ok && outcome.restart
    }

    fn cluster(&self, port: u16) -> PgCluster {
        PgCluster::new(
            self.layout.pg_bin.clone(),
            self.paths.pgdata.clone(),
            self.paths.logs.clone(),
            self.paths.run.clone(),
            port,
            self.cfg.database.user.clone(),
            self.cfg.database.password.clone(),
            self.cfg.database.locale_provider.clone(),
        )
    }

    /// Step 3: get PostgreSQL ready (incl. first-start parts initdb/createdb).
    pub async fn start_database(&mut self) -> Result<PgCluster> {
        let mut last_err = None;
        // If our cluster is still running (supervisor crashed), it itself occupies the preferred port: stop
        // it first, then choose a port – otherwise the port changes after every crash (smoke test D1).
        if self.cluster(self.cfg.database.port).is_initialized() {
            let orphan = self.cluster(self.cfg.database.port);
            if orphan.status().await? == crate::pg::PgStatus::Running {
                tracing::warn!("PostgreSQL lief noch (vorheriger Absturz?) – wird zuerst beendet");
                orphan.stop().await?;
            }
        }
        // Two attempts: another process can strike between the port check and the bind.
        for attempt in 0..2 {
            let preferred = (attempt == 0).then_some(self.cfg.database.port);
            let port = pick_port(preferred, &[self.cfg.server.port])
                .map_err(crate::io_err("Port für PostgreSQL wählen"))?;
            if port != self.cfg.database.port {
                tracing::info!(
                    old = self.cfg.database.port,
                    new = port,
                    "PostgreSQL-Port gewechselt"
                );
                self.cfg.database.port = port;
                self.save()?;
            }
            let pg = self.cluster(port);
            if !pg.is_initialized() {
                tracing::info!(pgdata = %self.paths.pgdata.display(), "Erst-Start: initdb");
                pg.initdb().await?;
            }
            if !self.cfg.setup.cluster_initialized {
                self.cfg.setup.cluster_initialized = true;
                self.save()?;
            }
            match pg.start().await {
                Ok(()) => {
                    if !self.cfg.setup.database_created {
                        let r = pg.ensure_database(&self.cfg.database.name).await;
                        if let Err(e) = r {
                            let _ = pg.stop().await;
                            return Err(e);
                        }
                        self.cfg.setup.database_created = true;
                        self.save()?;
                    }
                    // D-N3: server/manage run as the application role without superuser rights.
                    let db = &self.cfg.database;
                    if let Err(e) = pg
                        .ensure_app_role(&db.name, &db.app_user, &db.app_password)
                        .await
                    {
                        let _ = pg.stop().await;
                        return Err(e);
                    }
                    return Ok(pg);
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "PostgreSQL-Start fehlgeschlagen");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| DesktopError::Postgres("Start fehlgeschlagen".into())))
    }

    fn spec(&self, db_port: u16, app_port: u16) -> ServerSpec {
        ServerSpec {
            program: self.layout.server_bin.clone(),
            env: server_env(&self.cfg, &self.paths, &self.layout, db_port, app_port),
            cwd: self.paths.run.clone(),
            log_dir: self.paths.logs.clone(),
        }
    }

    fn backup_job(&self, pg: &PgCluster, dir: PathBuf) -> BackupJob {
        BackupJob {
            spec: self.spec(pg.port, self.cfg.server.port),
            pg: pg.clone(),
            db: self.cfg.database.name.clone(),
            dir,
        }
    }

    /// Step 4.
    pub async fn migrate(&self, pg: &PgCluster) -> Result<()> {
        let st = self
            .spec(pg.port, self.cfg.server.port)
            .manage(&["migrate"], &[], MIGRATE_TIMEOUT)
            .await?;
        if st.success() {
            Ok(())
        } else {
            Err(DesktopError::Server(format!(
                "manage migrate fehlgeschlagen ({st}); Details in logs/manage.log"
            )))
        }
    }

    /// First-start state of the database: (tables in schema `public`, "data" = real users
    /// or documents). `AnonymousUser` (django-guardian, created by `migrate`) does not count.
    async fn database_content(&self, pg: &PgCluster) -> Result<(i64, i64)> {
        let db = &self.cfg.database.name;
        let tables = pg
            .query_scalar(
                db,
                "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public'",
            )
            .await?
            .parse::<i64>()
            .unwrap_or(0);
        if tables == 0 {
            return Ok((0, 0));
        }
        let data = pg
            .query_scalar(
                db,
                "SELECT (SELECT count(*) FROM auth_user WHERE username <> 'AnonymousUser') \
                 + (SELECT count(*) FROM documents_document)",
            )
            .await
            // Schema incomplete (aborted migration): treat as "data present" –
            // never delete anything.
            .map(|s| s.parse::<i64>().unwrap_or(1))
            .unwrap_or(1);
        Ok((tables, data))
    }

    /// Step 3c: first-start choice, only for a database without users/documents.
    async fn first_start_choice(
        &mut self,
        pg: &PgCluster,
        prompt: &dyn AdminPrompt,
    ) -> Result<Option<SetupChoice>> {
        if self.cfg.setup.superuser_created {
            return Ok(None);
        }
        let (tables, data) = self.database_content(pg).await?;
        if data > 0 {
            return Ok(None);
        }
        let opts = self.setup_options();
        let mut attempts = 0;
        let choice = loop {
            attempts += 1;
            let c = prompt.choose_setup(&opts).ok_or_else(|| {
                DesktopError::Other("Einrichtung abgebrochen: kein Administrator angelegt".into())
            })?;
            match validate_setup(&c, &opts) {
                Ok(()) => break c,
                Err(msg) if attempts >= MAX_SETUP_ATTEMPTS => {
                    return Err(DesktopError::Other(format!("Einrichtung: {msg}")));
                }
                Err(msg) => tracing::warn!("{msg}"),
            }
        };
        tracing::info!(kind = choice.kind(), "Erst-Start-Wahl");
        if matches!(choice, SetupChoice::MigrateDb { .. }) && tables > 0 {
            // An earlier first start has already run `migrate` but created nothing
            // (checked: no users, no documents). paperrust-migrate-db requires an empty
            // target → recreate the schema (as cluster superuser, owner application role).
            let db = &self.cfg.database;
            tracing::warn!(
                tables,
                "leeres, schon migriertes Schema wird für den Datenbank-Umzug neu angelegt"
            );
            pg.run_script(
                &db.name,
                &format!(
                    "DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION \"{}\";",
                    db.app_user
                ),
            )
            .await?;
        }
        Ok(Some(choice))
    }

    /// Database migration with `paperrust-migrate-db` into the empty desktop DB (as the application role,
    /// so that all tables belong to it). Source and target URL only via the environment (passwords
    /// not in the process list). Output to `logs/migrate-db.log`, report
    /// `logs/migrate-db-report.json`.
    /// `media`: old media folder – needed so that when converting a paperless-ngx 2.x database the
    /// SHA-256 checksums are computed from the files (as in paperless-ngx 3).
    async fn migrate_db(&self, pg: &PgCluster, source_url: &str, media: Option<&Path>) -> Result<()> {
        let bin = self.layout.migrate_db_bin.as_ref().ok_or_else(|| {
            DesktopError::Other("paperrust-migrate-db fehlt in diesem Paket".into())
        })?;
        let db = &self.cfg.database;
        let target = format!(
            "postgres://{}:{}@127.0.0.1:{}/{}?sslmode=disable",
            db.app_user, db.app_password, pg.port, db.name
        );
        let log_path = self.paths.logs.join("migrate-db.log");
        let _ = crate::logs::rotate(&log_path, crate::logs::KEEP);
        let log = crate::logs::open_append(&log_path).map_err(crate::io_err("migrate-db.log"))?;
        let err = log.try_clone().map_err(crate::io_err("migrate-db.log"))?;
        let report = self.paths.logs.join("migrate-db-report.json");
        let mut c = tokio::process::Command::new(bin);
        crate::pg::scrub_env(&mut c);
        crate::pg::detach_console(&mut c);
        c.arg("--report").arg(&report).arg("--quiet");
        if let Some(m) = media {
            c.arg("--media").arg(m);
        }
        c
            .env("PAPERRUST_MIGRATE_SOURCE", source_url)
            .env("PAPERRUST_MIGRATE_TARGET", &target)
            .env("NO_COLOR", "1")
            .current_dir(&self.paths.run)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true);
        let mut child = c
            .spawn()
            .map_err(crate::io_err(format!("{} starten", bin.display())))?;
        let st = tokio::time::timeout(IMPORT_TIMEOUT, child.wait())
            .await
            .map_err(|_| DesktopError::Server("paperrust-migrate-db: Zeitüberschreitung".into()))?
            .map_err(crate::io_err("paperrust-migrate-db"))?;
        if st.success() {
            tracing::info!(report = %report.display(), "Datenbank übernommen");
            Ok(())
        } else {
            Err(DesktopError::Server(format!(
                "paperrust-migrate-db fehlgeschlagen ({st}); Details in logs/migrate-db.log"
            )))
        }
    }

    /// Step 4b: `manage document_importer <folder|ZIP>`.
    async fn import_export(&self, pg: &PgCluster, source: &Path) -> Result<()> {
        let src = source.display().to_string();
        let st = self
            .spec(pg.port, self.cfg.server.port)
            .manage(&["document_importer", &src], &[], IMPORT_TIMEOUT)
            .await?;
        if st.success() {
            tracing::info!(source = %src, "Export importiert");
            Ok(())
        } else {
            Err(DesktopError::Server(format!(
                "document_importer fehlgeschlagen ({st}); Details in logs/manage.log"
            )))
        }
    }

    /// Step 5: superuser and owner. No superuser (new installation or import without
    /// superuser) → `preset` (`PAPERRUST_DESKTOP_ADMIN_*`) or local owner without a dialog
    /// (name from the operating system, random password that is stored nowhere). Then
    /// [`Self::ensure_owner`] – on every start, also for existing installations.
    pub async fn ensure_superuser(
        &mut self,
        pg: &PgCluster,
        preset: Option<AdminCredentials>,
    ) -> Result<()> {
        if !self.cfg.setup.superuser_created {
            let n = pg
                .query_scalar(
                    &self.cfg.database.name,
                    "SELECT count(*) FROM auth_user WHERE is_superuser AND is_active",
                )
                .await?;
            if n.parse::<i64>().unwrap_or(0) == 0 {
                let generated = preset.is_none();
                let creds = match preset {
                    Some(c) => {
                        validate_admin(&c)
                            .map_err(|m| DesktopError::Other(format!("Einrichtung: {m}")))?;
                        c
                    }
                    None => AdminCredentials {
                        username: self.free_owner_name(pg).await?,
                        email: String::new(),
                        password: crate::autologin::random_password(),
                    },
                };
                let st = self
                    .spec(pg.port, self.cfg.server.port)
                    .manage(
                        &[
                            "createsuperuser",
                            "--no-input",
                            "--username",
                            &creds.username,
                            "--email",
                            &creds.email,
                        ],
                        &[("DJANGO_SUPERUSER_PASSWORD", &creds.password)],
                        Duration::from_secs(120),
                    )
                    .await?;
                if !st.success() {
                    return Err(DesktopError::Server(format!(
                        "createsuperuser fehlgeschlagen ({st}); Details in logs/manage.log"
                    )));
                }
                if generated {
                    tracing::info!(user = %creds.username, "lokaler Eigentümer angelegt (automatische Anmeldung, kein Passwort-Dialog)");
                } else {
                    tracing::info!(user = %creds.username, "Administrator angelegt");
                }
                self.cfg.setup.owner_user = creds.username;
            }
            self.cfg.setup.superuser_created = true;
            self.save()?;
        }
        self.ensure_owner(pg).await;
        Ok(())
    }

    /// Free user name for the owner: operating system name, if taken (also in a different
    /// upper/lower case) with a counter (`jane2`, `jane3` …).
    async fn free_owner_name(&self, pg: &PgCluster) -> Result<String> {
        let base = crate::autologin::owner_name_from_os(|k| std::env::var(k).ok());
        for i in 1..=50u32 {
            let name = crate::autologin::numbered_name(&base, i);
            let taken = pg
                .query_scalar(
                    &self.cfg.database.name,
                    &format!(
                        "SELECT count(*) FROM auth_user WHERE lower(username) = lower({})",
                        crate::autologin::sql_literal(&name)
                    ),
                )
                .await?;
            if taken.parse::<i64>().unwrap_or(1) == 0 {
                return Ok(name);
            }
        }
        Err(DesktopError::Other(
            "kein freier Benutzername für den lokalen Eigentümer gefunden".into(),
        ))
    }

    /// Check the owner for automatic login: active → stays; missing (existing installation,
    /// import) or deleted/deactivated → first active superuser (smallest id), remembered in
    /// `desktop.toml`. Not fatal: without an owner only automatic login stays off.
    async fn ensure_owner(&mut self, pg: &PgCluster) {
        let current = self.cfg.setup.owner_user.clone();
        let found = match pg
            .query_scalar(
                &self.cfg.database.name,
                &crate::autologin::owner_query(&current),
            )
            .await
        {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "Eigentümer nicht prüfbar – automatische Anmeldung bleibt aus");
                self.cfg.setup.owner_user.clear();
                return;
            }
        };
        let decision = crate::autologin::owner_decision(&current, &found);
        match decision {
            crate::autologin::OwnerDecision::Keep => {}
            crate::autologin::OwnerDecision::None => {
                tracing::warn!("kein aktiver Superuser – automatische Anmeldung bleibt aus");
                self.cfg.setup.owner_user.clear();
            }
            crate::autologin::OwnerDecision::Adopt(name) => {
                tracing::info!(old = %current, new = %name, "Eigentümer für die automatische Anmeldung übernommen");
                self.cfg.setup.owner_user = name;
                if let Err(e) = self.save() {
                    tracing::warn!(error = %e, "Eigentümer nicht in desktop.toml gespeichert");
                }
            }
        }
    }

    /// Environment for `paperrust serve`: like [`Self::spec`], plus login key and owner
    /// (only with an owner; `manage` calls get neither).
    fn serve_spec(&self, db_port: u16, app_port: u16) -> ServerSpec {
        let mut s = self.spec(db_port, app_port);
        crate::autologin::add_server_env(&mut s.env, &self.login_token, &self.cfg.setup.owner_user);
        s
    }

    /// Write the key file for shell/browser (or remove it if there is no owner).
    fn publish_login_token(&self) {
        if self.cfg.setup.owner_user.is_empty() {
            crate::autologin::remove_token(&self.paths.run);
            return;
        }
        if let Err(e) = crate::autologin::write_token(&self.paths.run, &self.login_token) {
            tracing::warn!(error = %e, "Anmeldeschlüssel nicht geschrieben – automatische Anmeldung aus");
        }
    }

    /// Step 5b: full-text index. If `data/index` is missing, it is created and built from the DB with
    /// `document_index reindex` (first start: empty, done immediately; import of a
    /// DB without index: all documents). Upstream creates the directory in the Docker init
    /// (`init-folders`, `init-search-index`); without the directory pl-indexer discards every
    /// index update ("index directory does not exist", smoke test D1).
    pub async fn ensure_index(&self, pg: &PgCluster) -> Result<()> {
        let dir = self.paths.index_dir();
        if dir.is_dir() {
            return Ok(());
        }
        crate::perms::create_private_dir(&dir)
            .map_err(crate::io_err(format!("{} anlegen", dir.display())))?;
        let st = self
            .spec(pg.port, self.cfg.server.port)
            .manage(
                &["document_index", "reindex"],
                &[],
                Duration::from_secs(6 * 3600),
            )
            .await;
        match st {
            Ok(st) if st.success() => {
                tracing::info!(dir = %dir.display(), "Volltext-Index aufgebaut");
                Ok(())
            }
            other => {
                // Directory gone again → the next start tries again.
                let _ = std::fs::remove_dir_all(&dir);
                let detail = match other {
                    Ok(st) => st.to_string(),
                    Err(e) => e.to_string(),
                };
                Err(DesktopError::Server(format!(
                    "document_index reindex fehlgeschlagen ({detail}); Details in logs/manage.log"
                )))
            }
        }
    }

    /// Complete run (steps 3–8) without a control channel. `on_event` informs the shell;
    /// `shutdown` ends when the user wants to quit (Ctrl+C, tray "Beenden", closing the window).
    pub async fn run(
        self,
        prompt: &dyn AdminPrompt,
        on_event: &(dyn Fn(Event) + Sync),
        shutdown: impl Future<Output = ()>,
    ) -> Result<()> {
        self.run_with_control(prompt, on_event, shutdown, Control::default())
            .await
    }

    /// Like [`Self::run`], additionally with backup requests of the shell ([`control_channel`]).
    pub async fn run_with_control(
        mut self,
        prompt: &dyn AdminPrompt,
        on_event: &(dyn Fn(Event) + Sync),
        shutdown: impl Future<Output = ()>,
        control: Control,
    ) -> Result<()> {
        on_event(Event::Stage("Datenbank starten"));
        let pg = self.start_database().await?;
        let result = self
            .run_with_db(&pg, prompt, on_event, shutdown, control)
            .await;
        on_event(Event::Stopping);
        crate::autologin::remove_token(&self.paths.run);
        let stopped = pg.stop().await;
        result.and(stopped)
    }

    /// Steps 3c–5b (first start, migration, import, superuser, index).
    async fn prepare(
        &mut self,
        pg: &PgCluster,
        prompt: &dyn AdminPrompt,
        on_event: &(dyn Fn(Event) + Sync),
    ) -> Result<Option<SetupChoice>> {
        let choice = self.first_start_choice(pg, prompt).await?;
        if let Some(SetupChoice::MigrateDb {
            source_url,
            media_dir,
        }) = &choice
        {
            on_event(Event::Stage("Bestehende Datenbank übernehmen"));
            self.migrate_db(pg, source_url, media_dir.as_deref()).await?;
            if let Some(m) = media_dir {
                on_event(Event::Stage("Dokumente kopieren"));
                let (from, to) = (m.clone(), self.paths.media.clone());
                let n = tokio::task::spawn_blocking(move || copy_tree(&from, &to))
                    .await
                    .map_err(|e| DesktopError::Other(format!("Kopieren: {e}")))?
                    .map_err(crate::io_err(format!("{} kopieren", m.display())))?;
                tracing::info!(files = n, from = %m.display(), "Medienordner kopiert");
            }
        }
        on_event(Event::Stage("Datenbank aktualisieren"));
        self.migrate(pg).await?;
        if let Some(SetupChoice::ImportExport { source }) = &choice {
            on_event(Event::Stage("paperless-ngx-Export importieren"));
            self.import_export(pg, source).await?;
        }
        self.migrate_settings(pg).await;
        let (preset, switched) = match choice {
            Some(SetupChoice::Fresh(c)) => (c, None),
            other => (None, other),
        };
        self.ensure_superuser(pg, preset).await?;
        on_event(Event::Stage("Suchindex prüfen"));
        self.ensure_index(pg).await?;
        Ok(switched)
    }

    /// Switching (package Z1): verification report in the background after the first start –
    /// `paperrust-migrate-db verify` with a sample via the API of the just-started server
    /// (temporary token). Result `logs/umstieg-bericht.{html,json,txt}`; source only read.
    fn spawn_switch_report(
        &self,
        choice: &SetupChoice,
        pg: &PgCluster,
        app_port: u16,
    ) -> Option<tokio::task::JoinHandle<SwitchCheck>> {
        let bin = self.layout.migrate_db_bin.clone()?;
        let db = &self.cfg.database;
        let target = format!(
            "postgres://{}:{}@127.0.0.1:{}/{}?sslmode=disable",
            db.app_user, db.app_password, pg.port, db.name
        );
        let mut c = tokio::process::Command::new(&bin);
        crate::pg::scrub_env(&mut c);
        crate::pg::detach_console(&mut c);
        c.arg("verify")
            .arg("--quiet")
            .arg("--target-media")
            .arg(&self.paths.media)
            .arg("--report-dir")
            .arg(&self.paths.logs)
            .arg("--api-url")
            .arg(base_url(app_port))
            .arg("--api-temp-token")
            .env("PAPERRUST_MIGRATE_TARGET", &target)
            .env("NO_COLOR", "1");
        match choice {
            SetupChoice::ImportExport { source } => {
                c.arg("--from").arg(source);
            }
            SetupChoice::MigrateDb {
                source_url,
                media_dir,
            } => {
                // URL (possibly with a password) only via the environment
                c.env("PAPERRUST_MIGRATE_SOURCE", source_url);
                if let Some(m) = media_dir {
                    c.arg("--media").arg(m);
                }
            }
            SetupChoice::Fresh(_) => return None,
        }
        let log = crate::logs::open_append(&self.paths.logs.join("migrate-db.log")).ok()?;
        let err = log.try_clone().ok()?;
        c.current_dir(&self.paths.run)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true);
        let logs = self.paths.logs.clone();
        // remove the old report: otherwise an aborted run would show the result of an earlier
        for ext in ["json", "html", "txt"] {
            let _ = std::fs::remove_file(logs.join(format!("umstieg-bericht.{ext}")));
        }
        Some(tokio::spawn(async move {
            match c.status().await {
                Ok(st) => {
                    let check = SwitchCheck::from_report(&logs, st.code());
                    if check.verdict == "OK" {
                        tracing::info!(report = ?check.report, "Umstieg geprüft: OK");
                    } else {
                        tracing::warn!(report = ?check.report, verdict = %check.verdict, "Umstieg geprüft: Befunde – Bericht ansehen");
                    }
                    check
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Prüfbericht zum Umstieg nicht erstellt");
                    SwitchCheck {
                        verdict: "FAIL".into(),
                        summary: vec![format!("Der Abgleich konnte nicht gestartet werden: {e}")],
                        report: None,
                    }
                }
            }
        }))
    }

    async fn run_with_db(
        &mut self,
        pg: &PgCluster,
        prompt: &dyn AdminPrompt,
        on_event: &(dyn Fn(Event) + Sync),
        shutdown: impl Future<Output = ()>,
        mut control: Control,
    ) -> Result<()> {
        let mut switched = self.prepare(pg, prompt, on_event).await?;
        let mut switch_report: Option<tokio::task::JoinHandle<SwitchCheck>> = None;

        let app_port = pick_port(Some(self.cfg.server.port), &[pg.port])
            .map_err(crate::io_err("Port für PaperRust wählen"))?;
        if app_port != self.cfg.server.port {
            tracing::warn!(
                old = self.cfg.server.port,
                new = app_port,
                "App-Port belegt – neue Adresse (Lesezeichen/Anmeldung neu)"
            );
            self.cfg.server.port = app_port;
            self.save()?;
        }
        let mut app_port = app_port;
        self.publish_login_token();
        let mut spec = self.serve_spec(pg.port, app_port);
        let mut url = format!("{}/", base_url(app_port));
        let mut port_retries = 0u32;
        tokio::pin!(shutdown);

        let mut restarts = 0u32;
        let mut window_start = Instant::now();
        let mut backup: Option<tokio::task::JoinHandle<()>> = None;
        // Display values for the UI (docs/SETTINGS.md §2.6).
        crate::settings_ctl::write_values(&self.paths.run, &self.cfg);
        let _ = std::fs::remove_file(self.paths.run.join(crate::settings_ctl::REQUEST_FILE));
        'serve: loop {
            on_event(Event::Stage("PaperRust starten"));
            let mut server = spec.spawn_server()?;
            tracing::info!(pid = ?server.pid(), port = app_port, "PaperRust-Server gestartet");
            tokio::select! {
                r = server.wait_healthy(app_port, SERVER_START_TIMEOUT) => {
                    match r {
                        Ok(()) => {}
                        // D-N5: port meanwhile taken by another process → choose again once.
                        Err(DesktopError::PortTaken(msg)) if port_retries == 0 => {
                            let _ = server.stop(SERVER_STOP_GRACE).await;
                            port_retries += 1;
                            let new = pick_port(None, &[pg.port, app_port])
                                .map_err(crate::io_err("Port für PaperRust wählen"))?;
                            tracing::warn!(old = app_port, new, reason = %msg, "App-Port belegt – neuer Versuch");
                            app_port = new;
                            self.cfg.server.port = new;
                            self.save()?;
                            spec = self.serve_spec(pg.port, app_port);
                            url = format!("{}/", base_url(app_port));
                            continue;
                        }
                        Err(e) => {
                            let _ = server.stop(SERVER_STOP_GRACE).await;
                            return Err(e);
                        }
                    }
                }
                () = &mut shutdown => {
                    finish_backup(&mut backup).await;
                    return server.stop(SERVER_STOP_GRACE).await;
                }
            }
            // before `Ready`: the shell then shows the reconciliation instead of the UI right away
            let check_pending = switched.is_some() && self.layout.migrate_db_bin.is_some();
            if check_pending {
                on_event(Event::SwitchCheck(SwitchCheck::running()));
            }
            self._lock.publish(&url)?;
            on_event(Event::Ready { url: url.clone() });
            if let Some(c) = switched.take() {
                switch_report = self.spawn_switch_report(&c, pg, app_port);
                if check_pending && switch_report.is_none() {
                    on_event(Event::SwitchCheck(SwitchCheck::from_report(&self.paths.logs, None)));
                }
            }
            let st = loop {
                tokio::select! {
                    st = server.wait() => break st,
                    check = join_switch_report(&mut switch_report), if switch_report.is_some() => {
                        switch_report = None;
                        on_event(Event::SwitchCheck(check));
                    }
                    reply = control.next_backup() => {
                        if backup.as_ref().is_some_and(|h| !h.is_finished()) {
                            let _ = reply.send(Err(DesktopError::Other("Es läuft bereits eine Sicherung.".into())));
                            continue;
                        }
                        on_event(Event::BackupStarted);
                        let job = self.backup_job(pg, self.paths.backups.join(backup_stamp()));
                        tracing::info!(dir = %job.dir.display(), "Sicherung im laufenden Betrieb");
                        backup = Some(tokio::spawn(async move {
                            let r = job.run().await;
                            if let Err(e) = &r {
                                tracing::error!(error = %e, "Sicherung fehlgeschlagen");
                            }
                            let _ = reply.send(r);
                        }));
                    }
                    () = crate::lock::stop_requested(self.paths.backup_request_file()) => {
                        // Backup request via file (`paperrust-desktop backup` with the app running).
                        let result_file = self.paths.backup_result_file();
                        if backup.as_ref().is_some_and(|h| !h.is_finished()) {
                            let _ = std::fs::write(&result_file, "error Es läuft bereits eine Sicherung.");
                            continue;
                        }
                        on_event(Event::BackupStarted);
                        let job = self.backup_job(pg, self.paths.backups.join(backup_stamp()));
                        tracing::info!(dir = %job.dir.display(), "Sicherung im laufenden Betrieb (Anforderung per Datei)");
                        backup = Some(tokio::spawn(async move {
                            let text = match job.run().await {
                                Ok(dir) => format!("ok {}", dir.display()),
                                Err(e) => {
                                    tracing::error!(error = %e, "Sicherung fehlgeschlagen");
                                    format!("error {e}")
                                }
                            };
                            let _ = crate::config::write_private(&result_file, text.as_bytes());
                        }));
                    }
                    req = crate::settings_ctl::next_request(self.paths.run.join(crate::settings_ctl::REQUEST_FILE)) => {
                        // Desktop settings from the UI (WP7); restart on port/LAN.
                        if self.apply_settings_request(req) {
                            let _ = server.stop(SERVER_STOP_GRACE).await;
                            app_port = self.cfg.server.port;
                            spec = self.serve_spec(pg.port, app_port);
                            url = format!("{}/", base_url(app_port));
                            tracing::info!(port = app_port, "PaperRust-Server mit neuer Desktop-Konfiguration neu starten");
                            continue 'serve;
                        }
                    }
                    () = &mut shutdown => {
                        // `Event::Stopping` is reported by `run` (once, also on error paths).
                        finish_backup(&mut backup).await;
                        return server.stop(SERVER_STOP_GRACE).await;
                    }
                }
            };
            let st = st.map_err(crate::io_err("PaperRust-Server"))?;
            if st.code() == Some(RESTART_EXIT_CODE) {
                tracing::info!(
                    "PaperRust-Server beendet sich für einen Neustart (Exit-Code 75) – starte neu"
                );
                continue;
            }
            tracing::error!(status = %st, "PaperRust-Server unerwartet beendet");
            if window_start.elapsed() > Duration::from_secs(600) {
                restarts = 0;
                window_start = Instant::now();
            }
            restarts += 1;
            if restarts > MAX_RESTARTS {
                finish_backup(&mut backup).await;
                return Err(DesktopError::Server(format!(
                    "{MAX_RESTARTS} Abstürze in 10 min – aufgegeben; Details in logs/server.log"
                )));
            }
            on_event(Event::Restarting { attempt: restarts });
            tokio::time::sleep(Duration::from_secs(u64::from(restarts) * 2)).await;
        }
    }

    /// Backup with the app closed: start PostgreSQL briefly, `migrate`, then as during
    /// operation ([`BackupJob`]).
    pub async fn backup(mut self, target: Option<PathBuf>) -> Result<PathBuf> {
        let dir = target.unwrap_or_else(|| self.paths.backups.join(backup_stamp()));
        let pg = self.start_database().await?;
        let r = async {
            self.migrate(&pg).await?;
            self.backup_job(&pg, dir).run().await
        }
        .await;
        let stopped = pg.stop().await;
        let dir = r?;
        stopped?;
        Ok(dir)
    }

    /// `paperrust manage <args…>` with the desktop environment (the app must be closed).
    pub async fn manage(mut self, args: &[String]) -> Result<i32> {
        let pg = self.start_database().await?;
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let r = self
            .spec(pg.port, self.cfg.server.port)
            .manage(&argv, &[], Duration::from_secs(24 * 3600))
            .await;
        let stopped = pg.stop().await;
        let st = r?;
        stopped?;
        Ok(st.code().unwrap_or(1))
    }
}

/// `paperrust-desktop backup` with the app running: request a backup from the instance and wait for the
/// result. `Ok(None)` = no instance running (then the caller backs up itself).
pub async fn request_live_backup(paths: &AppPaths, timeout: Duration) -> Result<Option<PathBuf>> {
    if let Acquire::Acquired(l) = InstanceLock::acquire(&paths.lock_file(), &paths.instance_file())?
    {
        drop(l);
        return Ok(None);
    }
    let result = paths.backup_result_file();
    let _ = std::fs::remove_file(&result);
    let req = paths.backup_request_file();
    std::fs::write(&req, b"backup")
        .map_err(crate::io_err(format!("{} schreiben", req.display())))?;
    let start = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(text) = std::fs::read_to_string(&result) {
            let _ = std::fs::remove_file(&result);
            return match text.split_once(' ') {
                Some(("ok", dir)) => Ok(Some(PathBuf::from(dir.trim()))),
                Some((_, msg)) => Err(DesktopError::Other(format!(
                    "Sicherung fehlgeschlagen: {}",
                    msg.trim()
                ))),
                None => Err(DesktopError::Other(format!(
                    "Sicherung: unerwartete Antwort {text:?}"
                ))),
            };
        }
        if let Acquire::Acquired(l) =
            InstanceLock::acquire(&paths.lock_file(), &paths.instance_file())?
        {
            drop(l);
            let _ = std::fs::remove_file(&req);
            return Err(DesktopError::Other(
                "PaperRust wurde während der Sicherung beendet".into(),
            ));
        }
        if start.elapsed() > timeout {
            return Err(DesktopError::Other(format!(
                "Sicherung: keine Antwort nach {} s",
                timeout.as_secs()
            )));
        }
    }
}

/// Wait for a running backup on exit (at most [`BACKUP_SHUTDOWN_WAIT`], then abort –
/// `kill_on_drop` ends the exporter, `pg_dump` ends with the stop of PostgreSQL).
async fn finish_backup(task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(mut h) = task.take()
        && !h.is_finished()
    {
        tracing::info!("Beenden: warte auf laufende Sicherung");
        if tokio::time::timeout(BACKUP_SHUTDOWN_WAIT, &mut h)
            .await
            .is_err()
        {
            tracing::warn!("Sicherung abgebrochen (Beenden)");
            h.abort();
        }
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) without a date crate.
pub fn backup_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    stamp_from_unix(secs)
}

pub fn stamp_from_unix(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Howard Hinnant, civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Waits for the reconciliation run (only called if one is running).
async fn join_switch_report(h: &mut Option<tokio::task::JoinHandle<SwitchCheck>>) -> SwitchCheck {
    match h.as_mut() {
        Some(j) => j.await.unwrap_or_else(|e| SwitchCheck {
            verdict: "FAIL".into(),
            summary: vec![format!("Der Abgleich ist abgebrochen: {e}")],
            report: None,
        }),
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_check_reads_report() {
        let d = tempfile::tempdir().unwrap();
        // no report: verdict from the exit code of `verify`
        let c = SwitchCheck::from_report(d.path(), Some(3));
        assert_eq!(c.verdict, "WARN");
        assert!(c.report.is_none());
        assert_eq!(SwitchCheck::from_report(d.path(), None).verdict, "FAIL");
        std::fs::write(
            d.path().join("umstieg-bericht.json"),
            r#"{"verdict":"OK","summary":["Dokumente: alle 204 übernommen","Tags: alle 20 übernommen"],
               "findings":[{"level":"INFO","message":"x"},{"level":"WARN","message":"Vorschaubild fehlt"}]}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("umstieg-bericht.html"), "<html></html>").unwrap();
        let c = SwitchCheck::from_report(d.path(), Some(1));
        assert_eq!(c.verdict, "OK", "Bericht geht vor Exit-Code");
        assert_eq!(c.summary, vec!["Dokumente: alle 204 übernommen", "Tags: alle 20 übernommen", "WARN: Vorschaubild fehlt"]);
        assert_eq!(c.report.as_deref(), Some(d.path().join("umstieg-bericht.html").as_path()));
        assert_eq!(SwitchCheck::running().verdict, "RUNNING");
    }

    fn c(u: &str, e: &str, p: &str) -> AdminCredentials {
        AdminCredentials {
            username: u.into(),
            email: e.into(),
            password: p.into(),
        }
    }

    #[test]
    fn admin_validation() {
        assert!(validate_admin(&c("jane", "jane@example.org", "geheim123")).is_ok());
        assert!(validate_admin(&c("d.i+x@y-z_", "", "12345678")).is_ok());
        assert!(validate_admin(&c("", "", "12345678")).is_err());
        assert!(validate_admin(&c("a b", "", "12345678")).is_err());
        assert!(validate_admin(&c("a", "", "kurz")).is_err());
        assert!(validate_admin(&c("a", "keine-mail", "12345678")).is_err());
    }

    #[test]
    fn fresh_start_needs_no_credentials() {
        struct Plain;
        impl AdminPrompt for Plain {}
        let opts = SetupOptions::default();
        // default choice: start fresh, owner automatic (no form for user/password)
        let choice = Plain.choose_setup(&opts).unwrap();
        assert!(matches!(choice, SetupChoice::Fresh(None)));
        assert!(validate_setup(&choice, &opts).is_ok());
        // credentials from the environment are still validated
        assert!(validate_setup(&SetupChoice::Fresh(Some(c("a", "", "kurz"))), &opts).is_err());
        // without PAPERRUST_DESKTOP_ADMIN_* the shell asks (form/console) – no default
        assert!(setup_from_env(|_| None).is_none());
    }

    #[test]
    fn setup_validation() {
        let dir = tempfile::tempdir().unwrap();
        let with = SetupOptions {
            can_migrate_db: true,
        };
        let without = SetupOptions::default();
        let export = dir.path().join("export");
        std::fs::create_dir_all(&export).unwrap();
        let imp = |p: &Path| SetupChoice::ImportExport {
            source: p.to_path_buf(),
        };
        assert!(
            validate_setup(&imp(&export), &without).is_err(),
            "ohne manifest.json"
        );
        std::fs::write(export.join("manifest.json"), "[]").unwrap();
        assert!(validate_setup(&imp(&export), &without).is_ok());
        let zip = dir.path().join("Export.ZIP");
        std::fs::write(&zip, b"PK").unwrap();
        assert!(validate_setup(&imp(&zip), &without).is_ok());
        assert!(validate_setup(&imp(&dir.path().join("fehlt.zip")), &without).is_err());
        assert!(validate_setup(&imp(&export.join("manifest.json")), &without).is_err());

        let db = dir.path().join("db.sqlite3");
        std::fs::write(&db, b"").unwrap();
        let url = format!("sqlite:///{}", db.display().to_string().replace('\\', "/"));
        let mig = |u: &str, m: Option<PathBuf>| SetupChoice::MigrateDb {
            source_url: u.into(),
            media_dir: m,
        };
        assert!(validate_setup(&mig(&url, None), &with).is_ok(), "{url}");
        assert!(validate_setup(&mig(&url, None), &without).is_err());
        assert!(validate_setup(&mig("sqlite:///C:/gibt/es/nicht.sqlite3", None), &with).is_err());
        assert!(validate_setup(&mig("mysql://u:p@h/paperless", None), &with).is_ok());
        assert!(validate_setup(&mig("ftp://x", None), &with).is_err());
        assert!(
            validate_setup(
                &mig("mysql://u:p@h/p", Some(dir.path().join("kein-ordner"))),
                &with
            )
            .is_err()
        );
        assert!(validate_setup(&mig("mysql://u:p@h/p", Some(export.clone())), &with).is_ok());
    }

    #[test]
    fn detects_switch_source() {
        let d = tempfile::tempdir().unwrap();
        let exp = d.path().join("export");
        std::fs::create_dir_all(&exp).unwrap();
        std::fs::write(exp.join("manifest.json"), "[]").unwrap();
        assert!(
            matches!(detect_setup(exp.to_str().unwrap(), None), Ok(SetupChoice::ImportExport { ref source }) if source == &exp)
        );
        let zip = d.path().join("e.zip");
        std::fs::write(&zip, b"PK\x03\x04rest").unwrap();
        assert!(matches!(
            detect_setup(&format!("\"{}\"", zip.display()), None),
            Ok(SetupChoice::ImportExport { .. })
        ));
        // paperless folder with data/db.sqlite3 and media/documents
        let root = d.path().join("paperless");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("media/documents")).unwrap();
        std::fs::write(root.join("data/db.sqlite3"), b"SQLite format 3\0xx").unwrap();
        for input in [
            root.clone(),
            root.join("data"),
            root.join("data/db.sqlite3"),
        ] {
            match detect_setup(input.to_str().unwrap(), None) {
                Ok(SetupChoice::MigrateDb {
                    source_url,
                    media_dir,
                }) => {
                    assert!(source_url.starts_with("sqlite://"), "{source_url}");
                    assert_eq!(
                        sqlite_path(&source_url).unwrap(),
                        std::path::absolute(root.join("data/db.sqlite3")).unwrap()
                    );
                    assert_eq!(
                        media_dir,
                        Some(root.join("data").parent().unwrap().join("media"))
                    );
                }
                _ => panic!("{input:?}"),
            }
        }
        assert!(matches!(
            detect_setup("postgres://u:p@h/db", None),
            Ok(SetupChoice::MigrateDb {
                media_dir: None,
                ..
            })
        ));
        assert!(detect_setup(d.path().to_str().unwrap(), None).is_err());
        assert!(detect_setup("", None).is_err());
        // environment: PAPERRUST_DESKTOP_IMPORT also detects data folders
        let env = |k: &str| (k == "PAPERRUST_DESKTOP_IMPORT").then(|| root.display().to_string());
        assert_eq!(setup_from_env(env).unwrap().kind(), "migrate-db");
    }

    #[test]
    fn sqlite_paths() {
        assert_eq!(
            sqlite_path("sqlite:///C:/x/db.sqlite3"),
            Some(PathBuf::from("C:/x/db.sqlite3"))
        );
        assert_eq!(
            sqlite_path("sqlite:///home/u/db.sqlite3?mode=ro"),
            Some(PathBuf::from("/home/u/db.sqlite3"))
        );
        assert_eq!(sqlite_path("mysql://x"), None);
    }

    #[test]
    fn setup_from_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        assert!(setup_from_env(env(&[])).is_none());
        let s = setup_from_env(env(&[
            ("PAPERRUST_DESKTOP_ADMIN_USER", "admin"),
            ("PAPERRUST_DESKTOP_ADMIN_PASSWORD", "12345678"),
        ]))
        .unwrap();
        assert!(
            matches!(s, SetupChoice::Fresh(Some(ref a)) if a.username == "admin" && a.email.is_empty())
        );
        let s = setup_from_env(env(&[
            ("PAPERRUST_DESKTOP_IMPORT", "C:/export.zip"),
            ("PAPERRUST_DESKTOP_ADMIN_USER", "admin"),
            ("PAPERRUST_DESKTOP_ADMIN_PASSWORD", "12345678"),
        ]))
        .unwrap();
        assert_eq!(s.kind(), "import");
        let s = setup_from_env(env(&[
            ("PAPERRUST_DESKTOP_IMPORT", " "),
            ("PAPERRUST_DESKTOP_IMPORT_DB", "sqlite:///x"),
            ("PAPERRUST_DESKTOP_IMPORT_MEDIA", "C:/media"),
        ]))
        .unwrap();
        assert!(
            matches!(s, SetupChoice::MigrateDb { ref media_dir, .. } if media_dir.as_deref() == Some(Path::new("C:/media")))
        );
    }

    #[test]
    fn copies_tree_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(a.join("documents/originals")).unwrap();
        std::fs::write(a.join("documents/originals/0001.pdf"), "neu").unwrap();
        std::fs::write(a.join("x.txt"), "neu").unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("x.txt"), "alt").unwrap();
        assert_eq!(copy_tree(&a, &b).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(b.join("x.txt")).unwrap(), "alt");
        assert_eq!(
            std::fs::read_to_string(b.join("documents/originals/0001.pdf")).unwrap(),
            "neu"
        );
    }

    #[tokio::test]
    async fn control_without_channel_never_fires() {
        let mut c = Control::default();
        let r = tokio::time::timeout(Duration::from_millis(50), c.next_backup()).await;
        assert!(r.is_err());
        let (tx, mut c) = control_channel();
        let (otx, _orx) = tokio::sync::oneshot::channel();
        tx.send(otx).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), c.next_backup())
                .await
                .is_ok()
        );
        drop(tx);
        let r = tokio::time::timeout(Duration::from_millis(50), c.next_backup()).await;
        assert!(r.is_err(), "geschlossener Kanal darf nicht feuern");
    }

    #[test]
    fn stamps() {
        assert_eq!(stamp_from_unix(0), "19700101-000000");
        assert_eq!(stamp_from_unix(951_782_400), "20000229-000000");
        assert_eq!(stamp_from_unix(1_794_139_199), "20261108-115959");
    }

    #[test]
    fn open_reports_missing_install_and_second_instance() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path().join("home"));
        let layout = InstallLayout::resolve(&dir.path().join("install"), |_| None, |_| false);
        let err = match Supervisor::open(paths.clone(), layout.clone()) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("unvollständige Installation muss scheitern"),
        };
        assert!(err.contains("pg_ctl"), "{err}");
        // D-N2: a left-over initdb password file is removed on open
        std::fs::create_dir_all(&paths.run).unwrap();
        let stale_pw = paths.run.join(crate::pg::INITDB_PWFILE);
        std::fs::write(&stale_pw, "geheim").unwrap();
        let _ = Supervisor::open(paths.clone(), layout.clone());
        assert!(!stale_pw.exists(), "initdb.pw muss weg sein");
        // lock was released again on the error; with a held lock: AlreadyRunning
        let Acquire::Acquired(held) =
            InstanceLock::acquire(&paths.lock_file(), &paths.instance_file()).unwrap()
        else {
            panic!()
        };
        held.publish("http://127.0.0.1:1234/").unwrap();
        match Supervisor::open(paths, layout).unwrap() {
            Opened::AlreadyRunning(Some(u)) => assert_eq!(u, "http://127.0.0.1:1234/"),
            _ => panic!("erwartet AlreadyRunning"),
        }
    }
}
