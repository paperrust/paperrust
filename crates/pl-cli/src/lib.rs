//! pl-cli – `paperrust manage …`: everything you need `manage.py` for with paperless-ngx v3.4.2,
//! without Python. Contract: docs/contract/pl-cli.md.
//!
//! Integration (W1, pl-server `main`):
//! ```ignore
//! if args.get(1).map(String::as_str) == Some("manage") {
//!     return pl_cli::run(args[2..].to_vec());   // before starting its own tokio runtime
//! }
//! // or inside a running runtime: pl_cli::run_async(args).await -> i32
//! // Routes: all_routes().extend(pl_cli::routes())   (OpenAPI /api/schema/, /api/schema/view/)
//! ```
//!
//! upstream: src/manage.py, django/core/management/__init__.py (ManagementUtility, CommandError →
//! "CommandError: …" on stderr, exit 1), src/documents/management/commands/*.py,
//! src/paperless_mail/management/commands/mail_fetcher.py (all @v3.4.2).

use std::process::ExitCode;

pub mod archiver;
pub mod args;
pub mod baseline;
pub mod check;
pub mod classifier;
pub mod consumer;
pub mod ctx;
pub mod datamodel;
pub mod export;
pub mod filemove;
pub mod fuzzy;
pub mod index;
pub mod llmindex;
pub mod mail;
pub mod migrate;
pub mod openapi;
pub mod par;
pub mod prune;
pub mod pyjson;
/// CPython `set` order (shared version in pl-core, W14).
pub use pl_core::pyset;
pub mod retagger;
pub mod sanity;
pub mod settings;
pub mod thumbnails;
pub mod trash;
pub mod users;

pub use openapi::routes;

/// Error of a command. `Command` corresponds to Django's `CommandError` (exit 1, message on stderr).
#[derive(Debug)]
pub enum CliError {
    /// `CommandError(msg)`
    Command(String),
    /// Usage error (argparse: exit 2)
    Usage(String),
    /// other error (traceback upstream; one line here)
    Other(String),
    /// Message unchanged on stderr, exit 1 (e.g. `SystemCheckError` from `check`).
    Raw(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Command(m) | CliError::Usage(m) | CliError::Other(m) | CliError::Raw(m) => f.write_str(m),
        }
    }
}

impl From<sqlx::Error> for CliError {
    fn from(e: sqlx::Error) -> Self {
        CliError::Other(format!("database error: {e}"))
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(format!("I/O error: {e}"))
    }
}

pub type CliResult<T = ()> = Result<T, CliError>;

/// Commands in help order.
pub const COMMANDS: &[(&str, &str)] = &[
    ("migrate", "Neuinstallation (v3.4.2-Schema) bzw. Prüfung einer bestehenden DB + paperrust_*-Migrationen"),
    ("showmigrations", "Migrationsstand anzeigen"),
    ("datamodel", "Eigenes Datenmodell (L6): status | check | rollback --to <stufe>"),
    ("createsuperuser", "Superuser anlegen (interaktiv oder --no-input mit DJANGO_SUPERUSER_*)"),
    ("manage_superuser", "Superuser aus PAPERRUST_ADMIN_USER/PASSWORD/MAIL (wie beim Containerstart)"),
    ("changepassword", "Passwort eines Nutzers ändern"),
    ("document_exporter", "Export (manifest.json, Dateien, --zip, --split-manifest, --passphrase …)"),
    ("document_importer", "Import eines Exports (Verzeichnis oder ZIP)"),
    ("document_sanity_checker", "Dokumentablage prüfen"),
    ("document_archiver", "Archiv-PDFs (neu) erzeugen [-f] [-d ID] [--processes N]"),
    ("document_consumer", "Eingangsordner einlesen/beobachten [ordner] [--oneshot]"),
    ("document_fuzzy_match", "Fast gleiche Dokumente finden [--ratio R] [--delete [--yes]] [--url URL]"),
    ("document_llmindex", "KI-Index: rebuild | update | compact | migrate"),
    ("check", "Systemprüfungen (Ordner, Programme, Einstellungen) [--fail-level LEVEL]"),
    ("document_retagger", "Zuordnungen per Regeln/Klassifikator neu setzen"),
    ("document_index", "Suchindex: reindex | optimize"),
    ("document_create_classifier", "Klassifikator trainieren"),
    ("document_thumbnails", "Vorschaubilder neu erzeugen"),
    ("document_renamer", "Dateien nach Dateinamen-Vorlage umbenennen"),
    ("prune_audit_logs", "Audit-Log-Einträge gelöschter Objekte entfernen"),
    ("empty_trash", "Papierkorb leeren (wie Aufgabe empty_trash)"),
    ("mail_fetcher", "Mail-Konten abrufen (W6)"),
    ("sync", "Sync-Änderungserfassung: status | enable | disable [--purge] | rebuild | reset-identity"),
    ("grants", "Freigabe-Index paperrust_grant (L5): status | check | rebuild | enable | disable [--purge]"),
    ("settings", "Systemeinstellungen: list | get | set | reset | export | import | rekey (docs/SETTINGS.md)"),
    ("timeline_backfill", "Zeitstrahl: Fristen im Inhalt aller Dokumente erkennen [--force] (PAPERRUST_TIMELINE_EXTRACT_DATES)"),
    ("guard", "Ransomware-Wächter: status | lift [--note TEXT] | scan (docs/user/ransomware.md)"),
    ("backup", "Verschlüsselte Sicherung: init | run | list | check | restore (PAPERRUST_BACKUP_*)"),
];

fn help_text() -> String {
    let mut s = String::from("Usage: paperrust manage <command> [options]\n\nAvailable subcommands:\n");
    for (c, d) in COMMANDS {
        s.push_str(&format!("    {c:<28} {d}\n"));
    }
    s
}

/// Synchronous entry point: builds its own tokio runtime. `args` without program name and without
/// `manage`, e.g. `["document_exporter", "/export", "--zip"]`.
pub fn run(args: Vec<String>) -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cannot start runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(run_async(args));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Asynchronous entry point (inside a running runtime). Return value = exit code.
pub async fn run_async(args: Vec<String>) -> i32 {
    let Some(cmd) = args.first().cloned() else {
        print!("{}", help_text());
        return 1;
    };
    let rest: Vec<String> = args[1..].to_vec();
    let result = dispatch(&cmd, rest).await;
    match result {
        Ok(()) => 0,
        Err(CliError::Command(m)) => {
            eprintln!("CommandError: {m}");
            1
        }
        Err(CliError::Usage(m)) => {
            eprintln!("usage: manage {cmd}: error: {m}");
            2
        }
        Err(CliError::Other(m)) => {
            eprintln!("Error: {m}");
            1
        }
        Err(CliError::Raw(m)) => {
            eprintln!("{m}");
            1
        }
    }
}

async fn dispatch(cmd: &str, rest: Vec<String>) -> CliResult {
    match cmd {
        "help" | "--help" | "-h" => {
            print!("{}", help_text());
            Ok(())
        }
        "migrate" => migrate::command(rest).await,
        "showmigrations" => migrate::showmigrations(rest).await,
        "datamodel" => datamodel::command(rest).await,
        "createsuperuser" => users::createsuperuser(rest).await,
        "manage_superuser" => users::manage_superuser(rest).await,
        "changepassword" => users::changepassword(rest).await,
        "document_exporter" => export::exporter::command(rest).await,
        "document_importer" => export::importer::command(rest).await,
        "document_sanity_checker" => sanity::command(rest).await,
        "document_archiver" => archiver::command(rest).await,
        "document_consumer" => consumer::command(rest).await,
        "document_fuzzy_match" => fuzzy::command(rest).await,
        "document_llmindex" => llmindex::command(rest).await,
        "check" => check::command(rest).await,
        "document_retagger" => retagger::command(rest).await,
        "document_index" => index::command(rest).await,
        "document_create_classifier" => classifier::command(rest).await,
        "document_thumbnails" => thumbnails::command(rest).await,
        "document_renamer" => filemove::renamer_command(rest).await,
        "prune_audit_logs" => prune::command(rest).await,
        "empty_trash" => trash::command(rest).await,
        "mail_fetcher" => mail::command(rest).await,
        "sync" => sync_command(rest).await,
        "grants" => grants_command(rest).await,
        "settings" => settings::command(rest).await,
        "timeline_backfill" => timeline_backfill(rest).await,
        "guard" => guard_command(rest).await,
        "backup" => backup_command(rest).await,
        other => Err(CliError::Command(format!("Unknown command: '{other}'\nType 'paperrust manage help' for usage."))),
    }
}

/// `manage sync …` (pl-sync, docs/contract/pl-sync.md).
/// `timeline_backfill [--force]`: content-based deadline detection for all (outdated) documents
/// (pl-timeline, like the task `paperrust.timeline.backfill`).
async fn timeline_backfill(rest: Vec<String>) -> CliResult {
    const OPTS: &[args::Opt] = &[args::flag(&["--force"], "force")];
    let a = args::parse(&rest, OPTS, 0)?;
    let c = ctx::Ctx::connect().await?;
    if !pl_timeline::refresh::enabled(&|k| pl_core::config::var(k)) {
        return Err(CliError::Command(format!("Date extraction is disabled; set {}=yes first.", pl_timeline::SETTING_EXTRACT)));
    }
    let dp = pl_timeline::extract::DateParse::from_lookup(pl_core::config::var);
    match pl_timeline::refresh::backfill(&c.pool, &dp, a.flag("force")).await {
        Ok(Some(r)) => {
            println!("Extracted dates for {} documents ({} dates found)", r.documents, r.dates);
            Ok(())
        }
        Ok(None) => Err(CliError::Command("A backfill is already running (on this database).".into())),
        Err(e) => Err(CliError::Command(e.to_string())),
    }
}

/// `manage grants …` (L5, pl-authz grant index, docs/contract/pl-authz.md "L5").
async fn grants_command(rest: Vec<String>) -> CliResult {
    let c = ctx::Ctx::connect().await?;
    match pl_authz::grants::command(&c.pool, &rest).await {
        Ok(out) => {
            print!("{out}");
            Ok(())
        }
        Err(pl_authz::grants::ManageError::Usage(m)) => Err(CliError::Usage(m)),
        Err(pl_authz::grants::ManageError::Mismatch(m)) => {
            print!("{m}");
            Err(CliError::Command("permission index differs from guardian (run `manage grants rebuild`)".into()))
        }
        Err(pl_authz::grants::ManageError::Db(e)) => Err(CliError::Command(e.to_string())),
    }
}

fn guard_outcome(o: pl_guard::cli::Outcome) -> CliResult {
    match o {
        pl_guard::cli::Outcome::Ok(s) => {
            println!("{s}");
            Ok(())
        }
        pl_guard::cli::Outcome::Usage(m) => Err(CliError::Usage(m)),
        pl_guard::cli::Outcome::Err(m) => Err(CliError::Command(m)),
    }
}

/// `manage guard status|lift|scan` (pl-guard, ransomware protection).
async fn guard_command(rest: Vec<String>) -> CliResult {
    let ctx = ctx::Ctx::connect().await?;
    guard_outcome(pl_guard::cli::guard_command(&rest, &ctx.pool, &ctx.cfg.media_root).await)
}

/// `manage backup init|run|list|check|restore` (pl-guard). `restore --database` migrates first.
async fn backup_command(rest: Vec<String>) -> CliResult {
    let ctx = ctx::Ctx::connect().await?;
    if rest.first().map(String::as_str) == Some("restore") && rest.iter().any(|a| a == "--database") {
        migrate::command(Vec::new()).await?;
    }
    guard_outcome(pl_guard::cli::backup_command(&rest, &ctx.pool, &ctx.cfg.media_root, &ctx.cfg.data_dir).await)
}

async fn sync_command(rest: Vec<String>) -> CliResult {
    let c = ctx::Ctx::connect().await?;
    match pl_sync::manage::command(&c.pool, &rest).await {
        Ok(out) => {
            print!("{out}");
            Ok(())
        }
        Err(pl_sync::manage::ManageError::Usage(m)) => Err(CliError::Usage(m)),
        Err(pl_sync::manage::ManageError::Sync(e)) => Err(CliError::Command(e.to_string())),
    }
}
