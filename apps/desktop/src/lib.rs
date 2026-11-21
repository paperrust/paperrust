//! PaperRust Desktop – supervisor (prototype, decision paper: docs/DESKTOP.md).
//!
//! Starts an **embedded PostgreSQL** (bundled binaries, own data directory,
//! only `127.0.0.1`) and `paperrust serve --worker` as a child process, checks both via health check
//! and shuts them down cleanly again (first the server, then `pg_ctl stop -m fast`).
//!
//! The shell (system browser today, later Tauri window/tray) is exchangeable: it only calls
//! [`supervisor::Supervisor`]. Pure logic (paths, ports, configuration, command lines)
//! is testable without processes.

pub mod autologin;
pub mod config;
pub mod job;
pub mod layout;
pub mod lock;
pub mod logs;
pub mod paths;
pub mod perms;
pub mod pg;
pub mod ports;
pub mod server;
pub mod settings_ctl;
pub mod supervisor;

/// Supervisor errors. Texts go to the user: no passwords/secrets in them.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Konfiguration {path}: {msg}")]
    Config { path: String, msg: String },
    #[error("PostgreSQL: {0}")]
    Postgres(String),
    #[error("PaperRust-Server: {0}")]
    Server(String),
    /// The app port was taken by another process between selection and start (D-N5).
    #[error("PaperRust-Server: {0}")]
    PortTaken(String),
    #[error("PaperRust läuft bereits ({0})")]
    AlreadyRunning(String),
    #[error("{0}")]
    Other(String),
}

impl DesktopError {
    /// Log file under `<home>/logs` that has the details for this error (error page of the shell:
    /// "Log öffnen").
    pub fn log_file(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "postgresql.log",
            Self::Server(m) | Self::PortTaken(m) if m.contains("manage.log") => "manage.log",
            Self::Server(m) if m.contains("migrate-db.log") => "migrate-db.log",
            Self::Server(_) | Self::PortTaken(_) => "server.log",
            _ => "desktop.log",
        }
    }

    /// Short hint for users (what to do?), if there is a typical cause.
    pub fn advice(&self) -> Option<&'static str> {
        match self {
            Self::PortTaken(_) => Some(concat!(
                "Ein anderes Programm belegt die Adresse von PaperRust (ein zweiter Port wurde schon ",
                "versucht). Andere Programme schließen oder den Rechner neu starten und erneut versuchen."
            )),
            Self::Postgres(m) if m.contains("mitgeliefert ist") => Some(concat!(
                "Das Datenbank-Verzeichnis passt nicht zu dieser PaperRust-Version. Nichts löschen: ",
                "Sicherung bereithalten und docs/DESKTOP.md Abschnitt 6 (Updates) lesen."
            )),
            Self::Postgres(_) => Some(concat!(
                "Die Datenbank ist nicht gestartet. Häufige Ursachen: Datenträger voll, Virenscanner ",
                "sperrt Dateien, Datenordner liegt in einem Cloud-Ordner (OneDrive/Dropbox). ",
                "Details im PostgreSQL-Log."
            )),
            Self::Io { .. } => Some("Eine Datei oder ein Ordner ist nicht zugänglich (Rechte, Datenträger voll?)."),
            Self::AlreadyRunning(_) => Some("PaperRust läuft bereits – das Fenster über das Tray-Symbol öffnen."),
            Self::Server(m) if m.contains("migrate") => Some(concat!(
                "Die Datenbank ließ sich nicht aktualisieren oder übernehmen. Details im Log; vor ",
                "einem neuen Versuch die letzte Sicherung bereithalten."
            )),
            Self::Server(m) if m.contains("document_importer") => Some(concat!(
                "Der Import ist fehlgeschlagen. Ist der Ordner/die ZIP-Datei ein vollständiger Export ",
                "von paperless-ngx (document_exporter, manifest.json)? Details im Log."
            )),
            _ => None,
        }
    }
}

pub type Result<T, E = DesktopError> = std::result::Result<T, E>;

/// `io::Error` with context (path/action).
pub(crate) fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> DesktopError {
    let context = context.into();
    move |source| DesktopError::Io { context, source }
}
