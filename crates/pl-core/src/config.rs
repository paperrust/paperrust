//! Configuration from `PAPERLESS_*` with the same defaults as upstream.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2, settings/custom.py (parse_db_settings,
//! parse_hosting_settings), settings/parsers.py (str_to_bool, get_list_from_env …).
//!
//! ## Variable names (docs/BRANDING.md, table "Umgebungsvariablen" = environment variables)
//!
//! * The primary name of every setting is `PAPERRUST_<NAME>`.
//! * **Compatibility with paperless-ngx:** every upstream setting `PAPERLESS_<NAME>` continues to
//!   apply unchanged (same semantics). If both are set, `PAPERRUST_<NAME>` wins.
//! * **Transition:** the old own names `PAPERLESS_RS_<NAME>` (working name paperless-rs) are
//!   an alias for `PAPERRUST_<NAME>`; at startup a warning appears once per name
//!   ([`log_alias_warnings`]).
//!
//! Implemented in [`Env::get`] (applies to everything read via [`Config`]/[`Env`], incl.
//! `paperless.conf`), [`var`] (process environment) and [`apply_env_aliases`] (mirrors at
//! program start into the process environment, so that crates with a direct `std::env::var` and
//! Django child processes see the same values).
//!
//! Own settings (PaperRust only, complete list: [`OWN_SETTINGS`]), e.g.:
//! * `PAPERRUST_BIND` – address PaperRust listens on (default `127.0.0.1:8080`, loopback
//!   only; the check in `pl-server`, which has the same default, is authoritative).
//! * `PAPERRUST_BASE_DIR` – corresponds to Django's `BASE_DIR.parent` (installation root in which
//!   `media/`, `data/`, `consume/`, `static/` lie by default). Default: working directory.
//! * `PAPERRUST_DB_MAX_CONNECTIONS` – pool size if `PAPERLESS_DB_POOLSIZE` is missing (default 10).
//!
//! As upstream, a configuration file is loaded beforehand: `PAPERRUST_CONFIGURATION_PATH`
//! or `PAPERLESS_CONFIGURATION_PATH`, otherwise the first existing one of `../paperrust.conf`,
//! `../paperless.conf`, `/etc/paperrust.conf`, `/etc/paperless.conf`,
//! `/usr/local/etc/paperrust.conf`, `/usr/local/etc/paperless.conf`; set
//! environment variables take precedence (python-dotenv `override=False`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono_tz::Tz;

/// Error while loading the configuration (corresponds to `ImproperlyConfigured`/`ValueError` at
/// Django startup).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(msg.into())
}

/// Snapshot of the environment variables (incl. `paperless.conf`).
#[derive(Clone, Default)]
pub struct Env {
    vars: BTreeMap<String, String>,
    /// Configuration file read and the names that come **from it** (not already set in the
    /// process environment). For the origin per key (docs/SETTINGS.md §2.1, rule N).
    file: Option<PathBuf>,
    file_keys: std::collections::BTreeSet<String>,
}

/// Origin of a variable name in [`Env`] (docs/SETTINGS.md §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvOrigin {
    /// Process environment (or [`Env::from_pairs`]).
    Process,
    /// Configuration file (`paperless.conf`/`paperrust.conf`).
    File(PathBuf),
}

impl Env {
    /// Origin of exactly this name (without aliases); `None` if not set.
    pub fn origin_exact(&self, name: &str) -> Option<EnvOrigin> {
        if !self.vars.contains_key(name) {
            return None;
        }
        match &self.file {
            Some(p) if self.file_keys.contains(name) => Some(EnvOrigin::File(p.clone())),
            _ => Some(EnvOrigin::Process),
        }
    }

    /// Configuration file read (if one was found).
    pub fn config_file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Like [`Env::from_pairs`], additionally with values from a configuration file `path`
    /// (process values take precedence, like [`Env::from_process`]; tests and settings registry).
    pub fn from_pairs_with_file<K: Into<String>, V: Into<String>>(
        process: impl IntoIterator<Item = (K, V)>,
        path: &Path,
        file: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut env = Self::from_pairs(process);
        env.merge_file(path, file.into_iter().map(|(k, v)| (k.into(), v.into())));
        env
    }

    fn merge_file(&mut self, path: &Path, pairs: impl IntoIterator<Item = (String, String)>) {
        for (k, v) in pairs {
            if !self.vars.contains_key(&k) {
                self.file_keys.insert(k.clone());
                self.vars.insert(k, v);
            }
        }
        self.file = Some(path.to_path_buf());
    }

    /// Process environment + first found `paperrust.conf`/`paperless.conf` (without
    /// overriding), plus the values saved in the UI ([`set_live_overlay`],
    /// precedence environment > database > file, docs/SETTINGS.md §2.1). Without an overlay (normal case
    /// outside the server) identical to [`Env::from_process_raw`].
    pub fn from_process() -> Self {
        let mut env = Self::from_process_raw();
        if let Some(ov) = live_overlay() {
            env.apply_overlay(&ov);
        }
        env
    }

    /// Copy with the overlay applied (see [`set_live_overlay`]).
    pub fn with_overlay(&self, ov: &BTreeMap<String, String>) -> Self {
        let mut e = self.clone();
        e.apply_overlay(ov);
        e
    }

    /// Applies the overlay values: only where no name of the key comes from the process environment;
    /// file values of the key are replaced.
    fn apply_overlay(&mut self, ov: &BTreeMap<String, String>) {
        for (key, value) in ov {
            let names = candidates(key);
            if names.iter().any(|n| self.vars.contains_key(n) && !self.file_keys.contains(n)) {
                continue;
            }
            for n in &names {
                self.vars.remove(n);
                self.file_keys.remove(n);
            }
            self.vars.insert(key.clone(), value.clone());
        }
    }

    /// Process environment + configuration file, **without** values from the UI (startup path,
    /// layering in `pl-settings`).
    pub fn from_process_raw() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let explicit = Self { vars: vars.clone(), ..Self::default() }.get("PAPERLESS_CONFIGURATION_PATH").map(String::from);
        let mut file: Option<(PathBuf, Vec<(String, String)>)> = None;
        let candidates = [
            explicit,
            Some("../paperrust.conf".to_string()),
            Some("../paperless.conf".to_string()),
            Some("/etc/paperrust.conf".to_string()),
            Some("/etc/paperless.conf".to_string()),
            Some("/usr/local/etc/paperrust.conf".to_string()),
            Some("/usr/local/etc/paperless.conf".to_string()),
        ];
        for path in candidates.into_iter().flatten() {
            if !path.is_empty() && Path::new(&path).exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    file = Some((PathBuf::from(&path), parse_dotenv(&content)));
                }
                break;
            }
        }
        let mut env = Self { vars, ..Self::default() };
        if let Some((path, pairs)) = file {
            env.merge_file(&path, pairs);
        }
        env
    }

    /// From a fixed list (tests).
    pub fn from_pairs<K: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
        Self { vars: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(), ..Self::default() }
    }

    /// `os.getenv(key)` with PaperRust's name rules ([`candidates`]): for
    /// `PAPERLESS_X` first `PAPERRUST_X`, for `PAPERRUST_X` afterwards the transitional name
    /// `PAPERLESS_RS_X`.
    pub fn get(&self, key: &str) -> Option<&str> {
        candidates(key).iter().find_map(|k| self.vars.get(k.as_str())).map(String::as_str)
    }

    /// Only exactly this name, without aliases (diagnostics, tests).
    pub fn get_exact(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// `os.getenv(key, default)`.
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or(default).to_string()
    }

    /// `key in os.environ` (with the same aliases as [`Env::get`]).
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// `get_bool_from_env(key, default)` (error for an unknown value like upstream).
    pub fn bool(&self, key: &str, default: &str) -> Result<bool, ConfigError> {
        let v = self.get(key).unwrap_or(default);
        str_to_bool(v).ok_or_else(|| invalid(format!("Cannot convert '{v}' to a boolean.")))
    }

    /// `get_int_from_env(key)` → `None` if not set.
    pub fn int(&self, key: &str) -> Result<Option<i64>, ConfigError> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => crate::pagination::python_int(v)
                .and_then(|i| i64::try_from(i).ok())
                .map(Some)
                .ok_or_else(|| invalid(format!("invalid literal for int() with base 10: '{v}' ({})", primary_name(key)))),
        }
    }

    /// `get_int_from_env(key, default)`.
    pub fn int_or(&self, key: &str, default: i64) -> Result<i64, ConfigError> {
        Ok(self.int(key)?.unwrap_or(default))
    }

    /// `get_float_from_env(key)`.
    pub fn float(&self, key: &str) -> Result<Option<f64>, ConfigError> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .trim()
                .parse::<f64>()
                .map(Some)
                .map_err(|_| invalid(format!("could not convert string to float: '{v}' ({})", primary_name(key)))),
        }
    }

    /// `get_list_from_env(key, default=…)` (comma, trim, remove empty ones).
    pub fn list(&self, key: &str, default: &[&str]) -> Vec<String> {
        match self.get(key) {
            Some(v) if !v.is_empty() => {
                v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
            }
            _ => default.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// `get_path_from_env(key, default)` – made absolute (Python `Path.resolve()` without
    /// resolving symlinks).
    pub fn path(&self, key: &str, default: &Path) -> PathBuf {
        absolute(&self.get(key).map(PathBuf::from).unwrap_or_else(|| default.to_path_buf()))
    }

    /// `get_path_from_env(key)` → `None` if not set.
    pub fn path_opt(&self, key: &str) -> Option<PathBuf> {
        self.get(key).map(|v| absolute(Path::new(v)))
    }

    /// `get_json_from_env(key, default)`.
    pub fn json(&self, key: &str) -> Result<Option<serde_json::Value>, ConfigError> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => serde_json::from_str(v)
                .map(Some)
                .map_err(|e| invalid(format!("Environment variable '{}' is not valid JSON: {e}", primary_name(key)))),
        }
    }

    /// All variables (e.g. for diagnostics; contains secrets – never print!).
    pub fn vars(&self) -> &BTreeMap<String, String> {
        &self.vars
    }
}

// ---------------------------------------------------------------------------------------------
// Names of the environment variables: PAPERRUST_* (primary), PAPERLESS_* (paperless-ngx), PAPERLESS_RS_*
// (transition). Central table: docs/BRANDING.md "Umgebungsvariablen" (environment variables).
// ---------------------------------------------------------------------------------------------

/// Prefix of the primary names.
pub const PREFIX: &str = "PAPERRUST_";
/// Prefix of paperless-ngx (compatibility with paperless-ngx, same semantics).
pub const UPSTREAM_PREFIX: &str = "PAPERLESS_";
/// Prefix of the working name paperless-rs (transitional alias for [`PREFIX`], with a warning).
pub const LEGACY_PREFIX: &str = "PAPERLESS_RS_";

/// A setting that exists only in PaperRust (no paperless-ngx counterpart).
#[derive(Debug, Clone, Copy)]
pub struct OwnSetting {
    /// Primary name (`PAPERRUST_…`).
    pub name: &'static str,
    /// Did the name already exist as `PAPERLESS_RS_…`? (then the old name counts as an alias)
    pub legacy_alias: bool,
    /// Meaning, default.
    pub meaning: &'static str,
}

const fn own(name: &'static str, legacy_alias: bool, meaning: &'static str) -> OwnSetting {
    OwnSetting { name, legacy_alias, meaning }
}

/// All own settings of server and worker (without test/desktop variables). These are
/// **not** mirrored to `PAPERLESS_*` at startup ([`apply_env_aliases`]). Every other
/// `PAPERRUST_X` counts as the primary name of the upstream setting `PAPERLESS_X`.
pub const OWN_SETTINGS: &[OwnSetting] = &[
    own("PAPERRUST_BIND", true, "Adresse:Port des Servers (Standard 127.0.0.1:8080)"),
    own("PAPERRUST_DJANGO_URL", true, "entfernt (2026-11-14): Weiterleitung an Django gibt es nicht mehr; gesetzt → Warnung beim Start, sonst wirkungslos"),
    own("PAPERRUST_ROUTES", true, "Pfad der routes.toml (Standard ./routes.toml)"),
    own("PAPERRUST_LOG", true, "Log-Filter (tracing EnvFilter, Standard `info`)"),
    own("PAPERRUST_BASE_DIR", true, "Installationswurzel = Django BASE_DIR.parent (Standard: Arbeitsverzeichnis)"),
    own("PAPERRUST_DB_MAX_CONNECTIONS", true, "Poolgröße, wenn PAPERRUST_DB_POOLSIZE fehlt (Standard 10)"),
    own("PAPERRUST_DB_REQUIRE_AUTH", false, "erlaubte DB-Anmeldeverfahren, z. B. scram-sha-256 (beim Start geprüft; Standard: keine Prüfung)"),
    own("PAPERRUST_INSTANCE_ID", false, "Kennung der Instanz, erscheint als `instance` in /__rs/health"),
    own("PAPERRUST_STOP_ON_STDIN_EOF", false, "Beenden bei stdin-EOF (Desktop-Supervisor, Standard no)"),
    own("PAPERRUST_BRANDING", false, "Logo/Name/Akzentfarbe von PaperRust (Standard on)"),
    own("PAPERRUST_ACCENT", false, "Akzentfarbe als Hex (Standard #B7410E)"),
    own("PAPERRUST_CACHE", false, "Cache: `local` (Standard, ein Prozess) oder `postgres` (gemeinsam, Mehrprozessbetrieb)"),
    own("PAPERRUST_QUEUE", false, "wirkungslos seit 2026-11-14: es gibt nur noch die eigene Warteschlange in PostgreSQL (`celery` → Warnung)"),
    own("PAPERRUST_QUEUE_BACKEND", false, "älterer Name von PAPERRUST_QUEUE (wirkungslos)"),
    own("PAPERRUST_QUEUE_NAMES", false, "abzuarbeitende Warteschlangen (Standard celery = historischer Name der Standard-Warteschlange)"),
    own("PAPERRUST_QUEUE_VISIBILITY_TIMEOUT", false, "Lease-Dauer einer Aufgabe (Standard 60 s)"),
    own("PAPERRUST_QUEUE_MAX_DELIVERIES", false, "Zustellungen je Versuch (Standard 3)"),
    own("PAPERRUST_QUEUE_POLL_INTERVAL", false, "Abfrageintervall ohne NOTIFY (Standard 1 s)"),
    own("PAPERRUST_QUEUE_SHUTDOWN_GRACE", false, "Wartezeit beim Herunterfahren (Standard 30 s)"),
    own("PAPERRUST_QUEUE_RESULT_EXPIRES", false, "Aufbewahrung erledigter Aufgaben (Standard 3600 s)"),
    own("PAPERRUST_WORKER_NAME", false, "Worker-Name (Standard paperrust@<host>:<pid>)"),
    own("PAPERRUST_INDEX_WRITER", false, "Suchindex nachführen: `queue` (Standard, über die Warteschlange), `rust` (direkt) oder `off`"),
    own("PAPERRUST_CLASSIFIER_MODEL_FILE", false, "Modell des Rust-Klassifikators (Standard DATA_DIR/paperrust_classifier.model)"),
    own("PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES", false, "Entpackgrenze beim Import (Standard 1 GiB + 100 × Archivgröße)"),
    own("PAPERRUST_TESSERACT_PATH", false, "tesseract-Programm (Standard aus PATH)"),
    own("PAPERRUST_TESSDATA_DIR", false, "tessdata-Verzeichnis"),
    own("PAPERRUST_PDFIUM_PATH", false, "PDFium-Bibliothek"),
    own("PAPERRUST_REMOTE_VERSION_URL", false, "Quelle der Versionsprüfung (pl-system)"),
    own("PAPERRUST_AI_API_BASE", true, "Basis-URL für interne API-Aufrufe des MCP-Servers"),
    own("PAPERRUST_AI_MCP_ENABLED", true, "/mcp ausliefern (Standard no)"),
    own("PAPERRUST_AI_MCP_RATE_LIMIT", true, "MCP-Anfragen pro Minute und Token (Standard 120)"),
    own("PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR", true, "MCP-Neuanlagen pro Token und Stunde (Standard 30)"),
    own("PAPERRUST_AI_MCP_ALLOWED_ORIGINS", true, "erlaubte Origins für /mcp (kommagetrennt)"),
    own("PAPERRUST_AI_MATCH_THRESHOLD", true, "Schwelle des erweiterten Namensabgleichs (Standard 0.8)"),
    own("PAPERRUST_AI_AUTO_MODE", true, "KI-Automatik: off | suggest | apply (Standard off)"),
    own("PAPERRUST_AI_AUTO_MIN_CONFIDENCE", true, "ab dieser Konfidenz automatisch übernehmen (Standard 0.9)"),
    own("PAPERRUST_AI_AUTO_TRIGGER_TAG", true, "Tag, der die KI-Verarbeitung auslöst"),
    own("PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS", true, "neue Objekte je Lauf (Standard 3)"),
    own("PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN", true, "Dokumente je Lauf (Standard 50)"),
    own("PAPERRUST_AI_AUTO_CREATE_MISSING", false, "fehlende Objekte anlegen (Standard no)"),
    own("PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR", false, "manuelle Läufe: Dokumente je Stunde (Standard 100)"),
    own("PAPERRUST_AI_TRIGGER_TAG_CRON", false, "Zeitplan des Auslöser-Tags (Crontab `m h dom mon dow`)"),
    own("PAPERRUST_AI_VISION_ENABLED", false, "KI-Texterkennung verfügbar (Standard no; Vorgabe für paperrust_ai_option)"),
    own("PAPERRUST_AI_VISION_MODEL", false, "Bildmodell der KI-Texterkennung (leer = Textmodell)"),
    own("PAPERRUST_AI_VISION_WHEN", false, "KI-Texterkennung: off | difficult | always (Standard difficult)"),
    own("PAPERRUST_AI_VISION_MAX_PAGES", false, "KI-Texterkennung: Seiten je Dokument (Standard 10)"),
    own("PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY", false, "KI-Texterkennung: Seiten je Tag und Prozess (Standard 500, 0 = unbegrenzt)"),
    own("PAPERRUST_AI_CUSTOM_FIELDS_ENABLED", false, "Benutzerfelder per KI ausfüllen (Standard no)"),
    own("PAPERRUST_INTEGRITY_INCREMENTAL", false, "Integritätsprüfung: unveränderte Dateien nicht erneut hashen (Standard no)"),
    own("PAPERRUST_INTEGRITY_FULL_HASH_DAYS", false, "Integritätsprüfung: Höchstalter einer Bestätigung in Tagen (Standard 28, 0 = immer hashen)"),
    own("PAPERRUST_INTEGRITY_THREADS", false, "Integritätsprüfung: parallele Leser (Standard 0 = automatisch, höchstens 4)"),
    own("PAPERRUST_INTEGRITY_REPORT_KEEP", false, "Integritätsprüfung: aufbewahrte JSON-Berichte (Standard 10, 0 = keine)"),
];

/// Is `name` an own setting ([`OWN_SETTINGS`])?
pub fn is_own_setting(name: &str) -> bool {
    OWN_SETTINGS.iter().any(|s| s.name == name)
}

/// Names in search order for a requested variable:
/// * `PAPERLESS_RS_X` (old code) and `PAPERRUST_X` → `PAPERRUST_X`, `PAPERLESS_RS_X`
/// * `PAPERLESS_X` → `PAPERRUST_X`, `PAPERLESS_X`
/// * otherwise only the name itself.
pub fn candidates(key: &str) -> Vec<String> {
    if let Some(rest) = key.strip_prefix(LEGACY_PREFIX).or_else(|| key.strip_prefix(PREFIX)) {
        vec![format!("{PREFIX}{rest}"), format!("{LEGACY_PREFIX}{rest}")]
    } else if let Some(rest) = key.strip_prefix(UPSTREAM_PREFIX) {
        vec![format!("{PREFIX}{rest}"), key.to_string()]
    } else {
        vec![key.to_string()]
    }
}

/// Name for messages and displays: `PAPERLESS_X` → `PAPERRUST_X`; `PAPERLESS_RS_X` →
/// `PAPERRUST_X`; everything else unchanged. (All names are still read, [`candidates`].)
pub fn primary_name(key: &str) -> String {
    match key.strip_prefix(LEGACY_PREFIX).or_else(|| key.strip_prefix(UPSTREAM_PREFIX)) {
        Some(rest) => format!("{PREFIX}{rest}"),
        None => key.to_string(),
    }
}

/// [`candidates`] over any source (e.g. `|k| std::env::var(k).ok()` or a map).
pub fn lookup(get: impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    candidates(key).iter().find_map(|k| get(k))
}

/// File name of the PaperRust classifier model in `DATA_DIR` (default).
pub const CLASSIFIER_MODEL_FILENAME: &str = "paperrust_classifier.model";

/// Path of the PaperRust classifier model: `PAPERRUST_CLASSIFIER_MODEL_FILE` (via `get`, so that
/// each caller keeps its source: process environment, `Config::env`, settings) or
/// `DATA_DIR/paperrust_classifier.model`. An empty value counts as not set. Single place
/// for ingestion (pl-consumer), training/commands (pl-cli), suggestions (pl-documents) and status
/// (pl-system).
pub fn classifier_model_file(get: impl FnOnce(&str) -> Option<String>, data_dir: &Path) -> PathBuf {
    get("PAPERRUST_CLASSIFIER_MODEL_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join(CLASSIFIER_MODEL_FILENAME))
}

/// `std::env::var(key).ok()` with PaperRust's name rules ([`candidates`]); if the name is missing
/// from the process environment, a value saved in the UI applies ([`set_live_overlay`]).
pub fn var(key: &str) -> Option<String> {
    lookup(|k| std::env::var(k).ok(), key).or_else(|| {
        let ov = live_overlay()?;
        lookup(|k| ov.get(k).cloned(), key)
    })
}

/// Set values saved in the UI (rule N, docs/SETTINGS.md §2.1) for this process
/// – canonical key → env string. Set by `pl-settings` with every new snapshot;
/// affects [`var`], [`Env::from_process`] and [`vars_live`]. Process environment takes precedence.
pub fn set_live_overlay(values: BTreeMap<String, String>) {
    // shared storage with lightweight crates without pl-core (pl-ocr, pl-pdfops, pl-templating)
    pl_envlive::set(values);
}

/// Current overlay (see [`set_live_overlay`]).
pub fn live_overlay() -> Option<std::sync::Arc<BTreeMap<String, String>>> {
    pl_envlive::get()
}

/// `std::env::vars()` plus overlay (for readers with their own variable map).
pub fn vars_live() -> std::collections::HashMap<String, String> {
    let mut m: std::collections::HashMap<String, String> = std::env::vars().collect();
    if let Some(ov) = live_overlay() {
        for (k, v) in ov.iter() {
            if !candidates(k).iter().any(|n| m.contains_key(n)) {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    m
}

/// Result of [`apply_env_aliases`] (for the one-time warnings at startup).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliasReport {
    /// `(old, new)`: `PAPERLESS_RS_X` adopted or equal to `PAPERRUST_X`.
    pub legacy_used: Vec<(String, String)>,
    /// `(old, new)`: `PAPERLESS_RS_X` ignored because `PAPERRUST_X` is set with a different value.
    pub legacy_ignored: Vec<(String, String)>,
    /// `(new, upstream)`: `PAPERRUST_X` overrides a differing `PAPERLESS_X`.
    pub upstream_overridden: Vec<(String, String)>,
}

static ALIAS_REPORT: std::sync::OnceLock<AliasReport> = std::sync::OnceLock::new();

/// Computes the mirrorings for a set of variables without applying them: `(name, value)` to
/// set and the report. Pure function (tests).
pub fn plan_env_aliases(vars: &BTreeMap<String, String>) -> (Vec<(String, String)>, AliasReport) {
    let mut set: BTreeMap<String, String> = BTreeMap::new();
    let mut report = AliasReport::default();
    for (k, v) in vars {
        let Some(rest) = k.strip_prefix(LEGACY_PREFIX) else { continue };
        let new = format!("{PREFIX}{rest}");
        match vars.get(&new) {
            None => {
                set.insert(new.clone(), v.clone());
                report.legacy_used.push((k.clone(), new));
            }
            Some(nv) if nv != v => report.legacy_ignored.push((k.clone(), new)),
            Some(_) => report.legacy_used.push((k.clone(), new)),
        }
    }
    let mut effective: BTreeMap<String, String> =
        vars.iter().filter(|(k, _)| k.starts_with(PREFIX)).map(|(k, v)| (k.clone(), v.clone())).collect();
    effective.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
    for (k, v) in effective {
        let rest = &k[PREFIX.len()..];
        if rest.is_empty() || is_own_setting(&k) {
            continue;
        }
        let upstream = format!("{UPSTREAM_PREFIX}{rest}");
        match vars.get(&upstream) {
            Some(u) if *u == v => {}
            Some(_) => {
                report.upstream_overridden.push((k.clone(), upstream.clone()));
                set.insert(upstream, v);
            }
            None => {
                set.insert(upstream, v);
            }
        }
    }
    (set.into_iter().collect(), report)
}

/// Copy of `vars` with the name rules applied (like [`apply_env_aliases`], but for a
/// map, e.g. in tests or crates with their own variable map).
pub fn with_aliases(vars: &std::collections::HashMap<String, String>) -> std::collections::HashMap<String, String> {
    let sorted: BTreeMap<String, String> = vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut out = vars.clone();
    out.extend(plan_env_aliases(&sorted).0);
    out
}

/// Applies the name rules to the **process environment**: `PAPERLESS_RS_X` → `PAPERRUST_X`
/// (if missing), then every `PAPERRUST_X` except [`OWN_SETTINGS`] → `PAPERLESS_X`. This way
/// crates with a direct `std::env::var("PAPERLESS_…")` and child processes (Django) also read the same
/// values. Only the first call has an effect; the report is for [`log_alias_warnings`].
///
/// # Safety
/// Changes the process environment (`std::env::set_var`). Call only at the very start of `main`
/// while no other thread is running (before the Tokio runtime and logger).
pub unsafe fn apply_env_aliases() -> &'static AliasReport {
    ALIAS_REPORT.get_or_init(|| {
        // C1: first resolve `*_FILE` (Docker secrets), so that the mirroring sees the values.
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let (from_files, messages) = plan_file_env(&vars, |p| std::fs::read(p).ok());
        for (k, v) in from_files {
            // SAFETY: per this function's contract no other thread is running yet.
            unsafe { std::env::set_var(k, v) };
        }
        let _ = FILE_ENV_MESSAGES.set(messages);
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let (set, report) = plan_env_aliases(&vars);
        for (k, v) in set {
            // SAFETY: per this function's contract no other thread is running yet.
            unsafe { std::env::set_var(k, v) };
        }
        report
    })
}

/// Messages from [`plan_file_env`] at startup (names only, never values); printed by
/// [`log_alias_warnings`].
static FILE_ENV_MESSAGES: std::sync::OnceLock<Vec<FileEnvMessage>> = std::sync::OnceLock::new();

/// One message when resolving `*_FILE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEnvMessage {
    /// `{X} set from {X_FILE}`
    Set { var: String, from: String },
    /// `Your secret: {X_FILE} contains a trailing newline and may not work as expected`
    TrailingNewline { from: String },
    /// `cannot find secret in {X_FILE}`
    Missing { from: String },
    /// Content not usable as an environment value (not UTF-8 or NUL byte; upstream: the shell would
    /// corrupt the value) – variable stays unset.
    Unusable { from: String },
}

impl std::fmt::Display for FileEnvMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileEnvMessage::Set { var, from } => write!(f, "[env-init] {var} set from {from}"),
            FileEnvMessage::TrailingNewline { from } => {
                write!(f, "[env-init] Your secret: {from} contains a trailing newline and may not work as expected")
            }
            FileEnvMessage::Missing { from } => write!(f, "[env-init] cannot find secret in {from}"),
            FileEnvMessage::Unusable { from } => {
                write!(f, "[env-init] secret in {from} is not valid UTF-8 or contains a NUL byte; ignored")
            }
        }
    }
}

/// `*_FILE` names that are themselves path settings (upstream init-env-file skips
/// `PAPERLESS_OCR_SKIP_ARCHIVE_FILE` and `PAPERLESS_MODEL_FILE`; plus our own
/// `PAPERRUST_CLASSIFIER_MODEL_FILE`).
const FILE_ENV_SKIP: &[&str] = &["OCR_SKIP_ARCHIVE_FILE", "MODEL_FILE", "CLASSIFIER_MODEL_FILE"];

/// C1 – values from files like upstream `docker/rootfs/etc/s6-overlay/s6-rc.d/init-env-file/run`
/// (v3.4.2): for every variable `PAPERLESS_<X>_FILE` (here also `PAPERRUST_…`/`PAPERLESS_RS_…`)
/// except the path settings in [`FILE_ENV_SKIP`], if the named file exists,
/// `PAPERLESS_<X>` is set to its **complete** content (like upstream incl. a
/// trailing newline, then with a warning) and overrides a set value.
/// Pure function: `read` returns the file content (`None` = file missing). Returns: pairs
/// to set and messages (without contents).
///
/// Deviation: upstream removes **every** occurrence of `_FILE` with `${FILENAME//_FILE/}`
/// (`PAPERLESS_FILENAME_FORMAT_FILE` would become `PAPERLESSNAME_FORMAT`); here only the suffix.
pub fn plan_file_env(
    vars: &BTreeMap<String, String>,
    read: impl Fn(&Path) -> Option<Vec<u8>>,
) -> (Vec<(String, String)>, Vec<FileEnvMessage>) {
    let mut set = Vec::new();
    let mut msgs = Vec::new();
    for (name, path) in vars {
        let Some(target) = name.strip_suffix("_FILE") else { continue };
        let rest = [LEGACY_PREFIX, PREFIX, UPSTREAM_PREFIX].iter().find_map(|p| name.strip_prefix(p));
        let Some(rest) = rest else { continue };
        if rest.is_empty() || rest == "FILE" || FILE_ENV_SKIP.contains(&rest) {
            continue;
        }
        let from = name.clone();
        let Some(bytes) = (if path.is_empty() { None } else { read(Path::new(path)) }) else {
            msgs.push(FileEnvMessage::Missing { from });
            continue;
        };
        let Ok(value) = String::from_utf8(bytes) else {
            msgs.push(FileEnvMessage::Unusable { from });
            continue;
        };
        if value.contains('\0') {
            msgs.push(FileEnvMessage::Unusable { from });
            continue;
        }
        if value.ends_with('\n') {
            msgs.push(FileEnvMessage::TrailingNewline { from: from.clone() });
        }
        set.push((target.to_string(), value));
        msgs.push(FileEnvMessage::Set { var: target.to_string(), from });
    }
    (set, msgs)
}

/// One-time warnings at startup: transitional names `PAPERLESS_RS_*` (environment or
/// configuration file) and `PAPERRUST_X` that override a differing `PAPERLESS_X`.
/// Call after setting up the logger; further calls do nothing.
pub fn log_alias_warnings(env: &Env) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        for m in FILE_ENV_MESSAGES.get().map(Vec::as_slice).unwrap_or_default() {
            match m {
                FileEnvMessage::Set { .. } => tracing::info!("{m}"),
                _ => tracing::warn!("{m}"),
            }
        }
        let report = ALIAS_REPORT.get().cloned().unwrap_or_default();
        for line in alias_warnings(env, &report) {
            tracing::warn!("{line}");
        }
    });
}

/// Own variables that only trigger a notice ([`retired_warnings`]).
const RETIRED: &[&str] = &[
    "PAPERRUST_DJANGO_URL",
    "PAPERRUST_QUEUE",
    "PAPERRUST_QUEUE_BACKEND",
    "PAPERRUST_REDIS_CONNECT_TIMEOUT",
    "PAPERRUST_REDIS_RETRY_AFTER",
    "PAPERRUST_PROXY_CONNECT_TIMEOUT_SECS",
    "PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS",
    "PAPERRUST_PROXY_X_FORWARDED_FOR",
];

/// Removed operating modes (2026-11-14): forwarding to Django, Celery/Redis. The variables
/// are ignored; if one is set, a notice appears once at startup (no abort).
pub fn retired_warnings(env: &Env) -> Vec<String> {
    let mut out = Vec::new();
    if env.get("PAPERRUST_DJANGO_URL").is_some() {
        out.push(
            "PAPERRUST_DJANGO_URL is set but has no effect anymore: PaperRust serves every route itself, forwarding to Django was removed. Please remove the variable.".to_string(),
        );
    }
    for k in ["PAPERRUST_QUEUE", "PAPERRUST_QUEUE_BACKEND"] {
        if let Some(v) = env.get(k).map(|v| v.trim().to_ascii_lowercase())
            && !matches!(v.as_str(), "" | "postgres" | "postgresql" | "pg" | "db")
        {
            out.push(format!(
                "{k}={v} is no longer supported: Celery/Redis was removed, PaperRust always uses its own task queue in PostgreSQL. Please remove the variable."
            ));
        }
    }
    if env.get("PAPERLESS_REDIS").is_some() {
        out.push("PAPERLESS_REDIS is set but PaperRust does not use Redis anymore (task queue and cache live in PostgreSQL or in the process). The variable is ignored; Redis can be shut down.".to_string());
    }
    for k in ["PAPERRUST_REDIS_CONNECT_TIMEOUT", "PAPERRUST_REDIS_RETRY_AFTER", "PAPERRUST_PROXY_CONNECT_TIMEOUT_SECS", "PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS", "PAPERRUST_PROXY_X_FORWARDED_FOR"] {
        if env.get(k).is_some() {
            out.push(format!("{k} has no effect anymore (Redis/Django support was removed) and is ignored."));
        }
    }
    out
}

/// Texts of the warnings from [`log_alias_warnings`] (tests).
pub fn alias_warnings(env: &Env, report: &AliasReport) -> Vec<String> {
    let mut out = retired_warnings(env);
    for (old, new) in &report.legacy_ignored {
        out.push(format!("{old} is ignored because {new} is set ({LEGACY_PREFIX}* is the old name of {PREFIX}*)"));
    }
    for k in env.vars().keys().filter(|k| k.starts_with(LEGACY_PREFIX)) {
        if report.legacy_ignored.iter().any(|(o, _)| o == k) {
            continue;
        }
        let new = format!("{PREFIX}{}", &k[LEGACY_PREFIX.len()..]);
        if RETIRED.contains(&new.as_str()) {
            continue; // retired_warnings already reports the name
        }
        out.push(format!("{k} is deprecated, please rename it to {new} (still honoured for now)"));
    }
    for (new, upstream) in &report.upstream_overridden {
        out.push(format!("{new} overrides a different value of {upstream} ({PREFIX}* takes precedence)"));
    }
    out
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// upstream `str_to_bool`: `true/1/t/y/yes` or `false/0/f/n/no`, case-insensitive.
pub fn str_to_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Some(true),
        "false" | "0" | "f" | "n" | "no" => Some(false),
        _ => None,
    }
}

/// Minimal `.env` parser (python-dotenv compatible for `KEY=VALUE`, `export KEY=…`,
/// comments, single/double quotes).
pub fn parse_dotenv(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim();
        let v = v.trim();
        let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
            let inner = &v[1..v.len() - 1];
            if v.starts_with('"') { inner.replace("\\n", "\n").replace("\\\"", "\"") } else { inner.to_string() }
        } else {
            // Inline comment after a space
            match v.find(" #") {
                Some(i) => v[..i].trim_end().to_string(),
                None => v.to_string(),
            }
        };
        if !k.is_empty() {
            out.push((k.to_string(), v));
        }
    }
    out
}

/// Database engine (`PAPERLESS_DBENGINE`, default `postgresql` if `PAPERLESS_DBHOST`
/// is set, otherwise `sqlite`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbEngine {
    Postgresql,
    Sqlite,
    Mariadb,
}

/// Database settings like `parse_db_settings`.
#[derive(Clone)]
pub struct DbConfig {
    pub engine: DbEngine,
    /// `PAPERLESS_DBHOST` (hostname or Unix socket directory).
    pub host: Option<String>,
    /// `PAPERLESS_DBPORT`.
    pub port: Option<u16>,
    /// `PAPERLESS_DBNAME` (default `paperless`).
    pub name: String,
    /// `PAPERLESS_DBUSER` (default `paperless`).
    pub user: String,
    /// `PAPERLESS_DBPASS` (default `paperless`). Never log.
    pub password: String,
    /// `sslmode` (PG default `prefer`, MariaDB `PREFERRED`), overridable via `PAPERLESS_DB_OPTIONS`.
    pub sslmode: String,
    pub sslrootcert: Option<String>,
    pub sslcert: Option<String>,
    pub sslkey: Option<String>,
    /// `PAPERLESS_DB_TIMEOUT` or option `connect_timeout`/`timeout` (seconds).
    pub connect_timeout: Option<u64>,
    /// `PAPERLESS_DB_POOLSIZE` or option `pool.max_size`.
    pub pool_max_size: Option<u32>,
    /// `application_name` (upstream `paperless-ngx`; PaperRust reports as `paperrust`
    /// unless set via `PAPERLESS_DB_OPTIONS`).
    pub application_name: String,
    /// `PAPERRUST_DB_REQUIRE_AUTH` (e.g. `scram-sha-256`): allowed authentication methods, checked at
    /// startup ([`crate::requireauth::check`]); `None` = no check (as before).
    pub require_auth: Option<String>,
    /// SQLite file (`DATA_DIR/db.sqlite3`).
    pub sqlite_path: PathBuf,
    /// All raw options from `PAPERLESS_DB_OPTIONS` (keys with dot notation).
    pub options: BTreeMap<String, String>,
}

/// Complete backend configuration. Fields are named like the Django settings (snake_case).
#[derive(Clone)]
pub struct Config {
    /// Raw values of all variables (for rare settings that are not a field here).
    pub env: Env,

    // --- PaperRust ---
    /// `PAPERRUST_BIND`, default `127.0.0.1:8080` (loopback only; same as `pl-server`,
    /// which validates the value itself).
    pub bind: String,
    /// `PAPERRUST_BASE_DIR` (= Django `BASE_DIR.parent`).
    pub base_dir: PathBuf,

    // --- General ---
    /// `PAPERLESS_DEBUG` (default `NO`).
    pub debug: bool,
    /// `PAPERLESS_SECRET_KEY` (mandatory, not `change-me`). Never log.
    pub secret_key: String,
    /// `PAPERLESS_TIME_ZONE` (default `UTC`).
    pub time_zone: String,
    /// Parsed time zone for `time_zone`.
    pub tz: Tz,

    // --- Directories ---
    pub static_root: PathBuf,
    pub media_root: PathBuf,
    pub originals_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub thumbnail_dir: PathBuf,
    pub share_link_bundle_dir: PathBuf,
    pub data_dir: PathBuf,
    /// `PAPERLESS_TRASH_DIR` or, deprecated, `PAPERLESS_EMPTY_TRASH_DIR`.
    pub empty_trash_dir: Option<PathBuf>,
    pub media_lock: PathBuf,
    pub index_dir: PathBuf,
    pub model_file: PathBuf,
    pub llm_index_dir: PathBuf,
    pub logging_dir: PathBuf,
    pub consumption_dir: PathBuf,
    pub scratch_dir: PathBuf,

    // --- Hosting / URLs ---
    /// `PAPERLESS_FORCE_SCRIPT_NAME`.
    pub force_script_name: Option<String>,
    /// `(FORCE_SCRIPT_NAME or "") + "/"`.
    pub base_url: String,
    pub login_url: String,
    pub login_redirect_url: String,
    /// `PAPERLESS_LOGOUT_REDIRECT_URL` (default `LOGIN_URL + "?loggedout=1"`).
    pub logout_redirect_url: String,
    /// `PAPERLESS_STATIC_URL` (default `BASE_URL + "static/"`).
    pub static_url: String,
    /// `PAPERLESS_URL`.
    pub paperless_url: Option<String>,
    /// `PAPERLESS_ALLOWED_HOSTS` (default `["*"]`; otherwise + `localhost`; + hostname from `PAPERLESS_URL`).
    pub allowed_hosts: Vec<String>,
    /// `PAPERLESS_CSRF_TRUSTED_ORIGINS` (+ `PAPERLESS_URL`, + `http://localhost:4200` with DEBUG).
    pub csrf_trusted_origins: Vec<String>,
    /// `PAPERLESS_CORS_ALLOWED_HOSTS` (default `["http://localhost:8000"]`, + URL, + DEBUG origin).
    pub cors_allowed_origins: Vec<String>,
    /// `PAPERLESS_USE_X_FORWARD_HOST` (default `false`).
    pub use_x_forwarded_host: bool,
    /// `PAPERLESS_USE_X_FORWARD_PORT` (default `false`).
    pub use_x_forwarded_port: bool,
    /// `PAPERLESS_PROXY_SSL_HEADER` as (META name, value), e.g. `("HTTP_X_FORWARDED_PROTO", "https")`.
    pub secure_proxy_ssl_header: Option<(String, String)>,
    /// `PAPERLESS_TRUSTED_PROXIES`.
    pub trusted_proxies: Vec<String>,
    /// `PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT` (default = number of `trusted_proxies`).
    pub allauth_trusted_proxy_count: i64,
    /// `PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER`.
    pub allauth_trusted_client_ip_header: Option<String>,

    // --- Cookies / session ---
    /// `PAPERLESS_COOKIE_PREFIX` (default empty).
    pub cookie_prefix: String,
    /// `{prefix}csrftoken`.
    pub csrf_cookie_name: String,
    /// `{prefix}sessionid`.
    pub session_cookie_name: String,
    /// `{prefix}django_language`.
    pub language_cookie_name: String,
    /// `PAPERLESS_SESSION_COOKIE_AGE` (default 3 weeks = 1814400 s).
    pub session_cookie_age: i64,
    /// `PAPERLESS_ACCOUNT_SESSION_REMEMBER` (default `True`).
    pub account_session_remember: bool,
    /// `not account_session_remember`.
    pub session_expire_at_browser_close: bool,

    // --- Authentication ---
    /// `PAPERLESS_ENABLE_HTTP_REMOTE_USER`.
    pub enable_http_remote_user: bool,
    /// `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API` (puts RemoteUser auth at position 0 of the
    /// DRF authenticators → `NotAuthenticated` becomes 403 instead of 401).
    pub enable_http_remote_user_api: bool,
    /// `PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME` in META format (default `HTTP_REMOTE_USER`).
    pub http_remote_user_header_name: String,
    /// `PAPERLESS_AUTO_LOGIN_USERNAME`.
    pub auto_login_username: Option<String>,
    /// `PAPERLESS_DISABLE_REGULAR_LOGIN`.
    pub disable_regular_login: bool,
    /// `PAPERLESS_REDIRECT_LOGIN_TO_SSO`.
    pub redirect_login_to_sso: bool,
    /// `PAPERLESS_ACCOUNT_ALLOW_SIGNUPS`.
    pub account_allow_signups: bool,
    /// `PAPERLESS_ACCOUNT_DEFAULT_GROUPS`.
    pub account_default_groups: Vec<String>,
    /// `PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS` (default `yes`).
    pub socialaccount_allow_signups: bool,
    /// `PAPERLESS_SOCIAL_AUTO_SIGNUP`.
    pub socialaccount_auto_signup: bool,
    /// `PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS`.
    pub social_account_default_groups: Vec<String>,
    /// `PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS`.
    pub social_account_sync_groups: bool,
    /// `PAPERLESS_ACCOUNT_EMAIL_VERIFICATION` (`none` if email is not configured; otherwise default `optional`).
    pub account_email_verification: String,
    /// `PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL` (default `https`).
    pub account_default_http_protocol: String,
    /// `PAPERLESS_TOKEN_THROTTLE_RATE` (default `5/min`, DRF throttle `login`).
    pub token_throttle_rate: String,

    // --- E-Mail (SMTP) ---
    pub email_host: String,
    pub email_port: i64,
    pub email_host_user: String,
    /// Never log.
    pub email_host_password: String,
    pub default_from_email: String,
    pub email_use_tls: bool,
    pub email_use_ssl: bool,
    /// `EMAIL_HOST != "localhost" or EMAIL_HOST_USER != ""`.
    pub email_enabled: bool,

    // --- Database ---
    pub db: DbConfig,

    // --- Domain settings that the backend needs directly ---
    /// `PAPERLESS_FILENAME_FORMAT`.
    pub filename_format: Option<String>,
    /// `PAPERLESS_FILENAME_FORMAT_REMOVE_NONE`.
    pub filename_format_remove_none: bool,
    /// `PAPERLESS_AUDIT_LOG_ENABLED` (default `true`).
    pub audit_log_enabled: bool,
    /// `PAPERLESS_EMPTY_TRASH_DELAY` (default 30, min. 1).
    pub empty_trash_delay: i64,
    /// `PAPERLESS_APP_TITLE`.
    pub app_title: Option<String>,
    /// `PAPERLESS_APP_LOGO`.
    pub app_logo: Option<String>,
    /// Product name in texts of the account pages and emails (page title "… sign in", subject
    /// "[…] …"). `None` = upstream text ("Paperless-ngx", parity); pl-server sets
    /// [`crate::brand::PRODUCT_NAME`] with `PAPERRUST_BRANDING=on`. The acknowledgment "by Paperless-ngx"
    /// always stays.
    pub product_name: Option<String>,
    /// `PAPERLESS_ENABLE_UPDATE_CHECK`: `None` = `"default"`, otherwise bool.
    pub enable_update_check: Option<bool>,
    /// `PAPERLESS_TASK_WORKERS` (default 1).
    pub task_workers: i64,
    /// `PAPERLESS_WORKER_TIMEOUT` (time limit per task, default 1800).
    pub worker_timeout: i64,
    /// `PAPERLESS_TIKA_ENABLED`.
    pub tika_enabled: bool,
    /// `PAPERLESS_AI_ENABLED`.
    pub ai_enabled: bool,
    /// `PAPERLESS_ENABLE_COMPRESSION` (default `yes`).
    pub enable_compression: bool,
    /// `PAPERLESS_DB_READ_CACHE_ENABLED` (cachalot; default `no`).
    pub db_read_cache_enabled: bool,

    // --- Search ---
    /// `PAPERLESS_OCR_LANGUAGE` (default `eng`).
    pub ocr_language: String,
    /// `SEARCH_LANGUAGE` (`_get_search_language_setting`): explicit `PAPERLESS_SEARCH_LANGUAGE`
    /// (checked against the Tantivy stemmer languages, otherwise startup error like upstream), otherwise derived from
    /// the first OCR language.
    pub search_language: Option<String>,
    /// `PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD`.
    pub advanced_fuzzy_search_threshold: Option<f64>,

    // --- Remote OCR (os.getenv: an empty value stays `Some("")`) ---
    /// `PAPERLESS_REMOTE_OCR_ENGINE`.
    pub remote_ocr_engine: Option<String>,
    /// `PAPERLESS_REMOTE_OCR_API_KEY` (secret, never log).
    pub remote_ocr_api_key: Option<String>,
    /// `PAPERLESS_REMOTE_OCR_ENDPOINT`.
    pub remote_ocr_endpoint: Option<String>,
    /// `PAPERLESS_REMOTE_OCR_MODE` (`always` | `workflow_only`, default `always`).
    pub remote_ocr_mode: String,
    /// `PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS` (default `true`).
    pub remote_ocr_allow_internal_endpoints: bool,

    // --- Mail ---
    /// `PAPERLESS_EMAIL_CERTIFICATE_LOCATION` (`EMAIL_CERTIFICATE_FILE`, made absolute).
    pub email_certificate_file: Option<PathBuf>,
    /// `PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS` (default `true`).
    pub email_allow_internal_hosts: bool,
    /// `PAPERLESS_OAUTH_CALLBACK_BASE_URL`.
    pub oauth_callback_base_url: Option<String>,
    /// `PAPERLESS_GMAIL_OAUTH_CLIENT_ID`.
    pub gmail_oauth_client_id: Option<String>,
    /// `PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET` (secret).
    pub gmail_oauth_client_secret: Option<String>,
    /// `GMAIL_OAUTH_ENABLED`.
    pub gmail_oauth_enabled: bool,
    /// `PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID`.
    pub outlook_oauth_client_id: Option<String>,
    /// `PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET` (secret).
    pub outlook_oauth_client_secret: Option<String>,
    /// `OUTLOOK_OAUTH_ENABLED`.
    pub outlook_oauth_enabled: bool,
}

/// Default for `PAPERRUST_BIND` (loopback only).
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// `documents/search/_tokenizer.py::SUPPORTED_LANGUAGES` (Tantivy-Stemmer).
pub const SEARCH_SUPPORTED_LANGUAGES: &[&str] = &[
    "ar", "arabic", "da", "danish", "nl", "dutch", "en", "english", "fi", "finnish", "fr", "french", "de",
    "german", "el", "greek", "hu", "hungarian", "it", "italian", "no", "norwegian", "pt", "portuguese", "ro",
    "romanian", "ru", "russian", "es", "spanish", "sv", "swedish", "ta", "tamil", "tr", "turkish",
];

/// `_get_search_language_setting(ocr_lang)`.
fn search_language_setting(env: &Env, ocr_lang: &str) -> Result<Option<String>, ConfigError> {
    if let Some(explicit) = env.get("PAPERLESS_SEARCH_LANGUAGE") {
        if SEARCH_SUPPORTED_LANGUAGES.contains(&explicit) {
            return Ok(Some(explicit.to_string()));
        }
        let mut sorted = SEARCH_SUPPORTED_LANGUAGES.to_vec();
        sorted.sort_unstable();
        return Err(invalid(format!(
            "Environment variable 'PAPERRUST_SEARCH_LANGUAGE' has invalid value '{explicit}'. Valid choices are: {}",
            sorted.join(", ")
        )));
    }
    let primary = ocr_lang.split('+').next().unwrap_or("").to_lowercase();
    let map: &[(&str, &str)] = &[
        ("ara", "ar"), ("dan", "da"), ("nld", "nl"), ("eng", "en"), ("fin", "fi"), ("fra", "fr"),
        ("deu", "de"), ("ell", "el"), ("hun", "hu"), ("ita", "it"), ("nor", "no"), ("por", "pt"),
        ("ron", "ro"), ("rus", "ru"), ("spa", "es"), ("swe", "sv"), ("tam", "ta"), ("tur", "tr"),
    ];
    Ok(map.iter().find(|(k, _)| *k == primary).map(|(_, v)| v.to_string()))
}

impl Config {
    /// Reads the process environment (+ `paperless.conf`).
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_map(Env::from_process())
    }

    /// Like [`Config::from_env`], but from a given set of variables (tests).
    pub fn from_env_map(env: Env) -> Result<Self, ConfigError> {
        let debug = env.bool("PAPERLESS_DEBUG", "NO")?;
        let base_dir = env.path("PAPERRUST_BASE_DIR", Path::new("."));

        let static_root = env.path("PAPERLESS_STATICDIR", &base_dir.join("static"));
        let media_root = env.path("PAPERLESS_MEDIA_ROOT", &base_dir.join("media"));
        let data_dir = env.path("PAPERLESS_DATA_DIR", &base_dir.join("data"));
        let empty_trash_dir = match (env.get("PAPERLESS_TRASH_DIR"), env.get("PAPERLESS_EMPTY_TRASH_DIR")) {
            (Some(t), _) if !t.is_empty() => Some(absolute(Path::new(t))),
            (_, Some(e)) if !e.is_empty() => Some(absolute(Path::new(e))),
            _ => None,
        };
        let scratch_default = std::env::temp_dir().join("paperrust");

        // Hosting
        let force_script_name = env.get("PAPERLESS_FORCE_SCRIPT_NAME").map(String::from);
        let base_url = format!("{}/", force_script_name.as_deref().unwrap_or(""));
        let login_url = format!("{base_url}accounts/login/");
        let login_redirect_url = format!("{base_url}dashboard");
        let logout_redirect_url = env.get_or("PAPERLESS_LOGOUT_REDIRECT_URL", &format!("{login_url}?loggedout=1"));
        let static_url = env.get_or("PAPERLESS_STATIC_URL", &format!("{base_url}static/"));

        // E-Mail
        let email_host = env.get_or("PAPERLESS_EMAIL_HOST", "localhost");
        let email_host_user = env.get_or("PAPERLESS_EMAIL_HOST_USER", "");
        let email_enabled = email_host != "localhost" || !email_host_user.is_empty();

        // Remote user
        let enable_http_remote_user = env.bool("PAPERLESS_ENABLE_HTTP_REMOTE_USER", "NO")?;
        let enable_http_remote_user_api = env.bool("PAPERLESS_ENABLE_HTTP_REMOTE_USER_API", "NO")?;

        // Hosts / CSRF / CORS
        let mut csrf_trusted_origins = env.list("PAPERLESS_CSRF_TRUSTED_ORIGINS", &[]);
        if debug {
            csrf_trusted_origins.push("http://localhost:4200".into());
        }
        let mut cors_allowed_origins = env.list("PAPERLESS_CORS_ALLOWED_HOSTS", &["http://localhost:8000"]);
        if debug {
            cors_allowed_origins.push("http://localhost:4200".into());
        }
        let paperless_url = env.get("PAPERLESS_URL").filter(|u| !u.is_empty()).map(String::from);
        let mut allowed_hosts = initial_allowed_hosts(env.get("PAPERLESS_ALLOWED_HOSTS"), paperless_url.is_some());
        if let Some(url) = &paperless_url {
            let host = url::Url::parse(url).ok().and_then(|u| u.host_str().and_then(allowed_host_from_url_hostname));
            let Some(host) = host else {
                return Err(invalid(format!(
                    "PAPERRUST_URL '{url}' has no valid hostname, expected a URL like https://example.com"
                )));
            };
            csrf_trusted_origins.push(url.clone());
            cors_allowed_origins.push(url.clone());
            allowed_hosts.push(host);
        }

        let trusted_proxies = env.list("PAPERLESS_TRUSTED_PROXIES", &[]);
        let allauth_trusted_proxy_count = env.int_or("PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT", trusted_proxies.len() as i64)?;
        if allauth_trusted_proxy_count < 0 {
            return Err(invalid("PAPERRUST_ALLAUTH_TRUSTED_PROXY_COUNT must be zero or greater"));
        }

        let secure_proxy_ssl_header = if env.contains("PAPERLESS_PROXY_SSL_HEADER") {
            match env.json("PAPERLESS_PROXY_SSL_HEADER")? {
                Some(serde_json::Value::Array(a)) => {
                    let parts: Vec<String> = a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect();
                    if parts.is_empty() {
                        None // empty tuple is falsy in Django → no proxy header
                    } else if parts.len() == 2 {
                        Some((parts[0].clone(), parts[1].clone()))
                    } else {
                        return Err(invalid("The SECURE_PROXY_SSL_HEADER setting must be a tuple containing two values."));
                    }
                }
                _ => return Err(invalid("Environment variable 'PAPERRUST_PROXY_SSL_HEADER' must be a JSON list")),
            }
        } else {
            None
        };

        let secret_key = env.get("PAPERLESS_SECRET_KEY").unwrap_or("").to_string();
        if secret_key.trim().is_empty() || secret_key == "change-me" {
            return Err(invalid(
                "PAPERRUST_SECRET_KEY is not set or is the default 'change-me' value. A unique, secret key is required for secure operation.",
            ));
        }

        let cookie_prefix = env.get_or("PAPERLESS_COOKIE_PREFIX", "");
        let account_session_remember = env.bool("PAPERLESS_ACCOUNT_SESSION_REMEMBER", "True")?;

        let time_zone = env.get_or("PAPERLESS_TIME_ZONE", "UTC");
        let tz: Tz = time_zone.parse().map_err(|_| invalid(format!("Incorrect timezone setting: {time_zone}")))?;

        let enable_update_check = match env.get("PAPERLESS_ENABLE_UPDATE_CHECK") {
            None | Some("default") => None,
            Some(_) => Some(env.bool("PAPERLESS_ENABLE_UPDATE_CHECK", "NO")?),
        };

        let db = parse_db_settings(&env, &data_dir)?;

        let ocr_language = env.get_or("PAPERLESS_OCR_LANGUAGE", "eng");
        let remote_ocr_mode = env.get_or("PAPERLESS_REMOTE_OCR_MODE", "always");
        if remote_ocr_mode != "always" && remote_ocr_mode != "workflow_only" {
            return Err(invalid(format!(
                "Environment variable 'PAPERRUST_REMOTE_OCR_MODE' has invalid value '{remote_ocr_mode}'. Valid choices are: always, workflow_only"
            )));
        }
        // GMAIL/OUTLOOK_OAUTH_ENABLED = bool((OAUTH_CALLBACK_BASE_URL or PAPERLESS_URL) and ID and SECRET)
        let nonempty = |k: &str| env.get(k).is_some_and(|v| !v.is_empty());
        let oauth_base =
            nonempty("PAPERLESS_OAUTH_CALLBACK_BASE_URL") || paperless_url.as_deref().is_some_and(|u| !u.is_empty());
        let gmail_oauth_enabled =
            oauth_base && nonempty("PAPERLESS_GMAIL_OAUTH_CLIENT_ID") && nonempty("PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET");
        let outlook_oauth_enabled =
            oauth_base && nonempty("PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID") && nonempty("PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET");

        Ok(Config {
            bind: env.get_or("PAPERRUST_BIND", DEFAULT_BIND),
            base_dir,
            debug,
            secret_key,
            time_zone,
            tz,
            originals_dir: media_root.join("documents").join("originals"),
            archive_dir: media_root.join("documents").join("archive"),
            thumbnail_dir: media_root.join("documents").join("thumbnails"),
            share_link_bundle_dir: media_root.join("documents").join("share_link_bundles"),
            media_lock: media_root.join("media.lock"),
            index_dir: data_dir.join("index"),
            model_file: env.path("PAPERLESS_MODEL_FILE", &data_dir.join("classification_model.pickle")),
            llm_index_dir: data_dir.join("llm_index"),
            logging_dir: env.path("PAPERLESS_LOGGING_DIR", &data_dir.join("log")),
            consumption_dir: env.path("PAPERLESS_CONSUMPTION_DIR", &base_dir_join(&env, "consume")),
            scratch_dir: env.path("PAPERLESS_SCRATCH_DIR", &scratch_default),
            static_root,
            media_root,
            data_dir,
            empty_trash_dir,
            force_script_name,
            base_url,
            login_url,
            login_redirect_url,
            logout_redirect_url,
            static_url,
            paperless_url,
            allowed_hosts,
            csrf_trusted_origins,
            cors_allowed_origins,
            use_x_forwarded_host: env.bool("PAPERLESS_USE_X_FORWARD_HOST", "false")?,
            use_x_forwarded_port: env.bool("PAPERLESS_USE_X_FORWARD_PORT", "false")?,
            secure_proxy_ssl_header,
            trusted_proxies,
            allauth_trusted_proxy_count,
            allauth_trusted_client_ip_header: env.get("PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER").map(String::from),
            csrf_cookie_name: format!("{cookie_prefix}csrftoken"),
            session_cookie_name: format!("{cookie_prefix}sessionid"),
            language_cookie_name: format!("{cookie_prefix}django_language"),
            cookie_prefix,
            session_cookie_age: env.int_or("PAPERLESS_SESSION_COOKIE_AGE", 60 * 60 * 24 * 7 * 3)?,
            account_session_remember,
            session_expire_at_browser_close: !account_session_remember,
            enable_http_remote_user,
            enable_http_remote_user_api,
            http_remote_user_header_name: env.get_or("PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME", "HTTP_REMOTE_USER"),
            auto_login_username: env.get("PAPERLESS_AUTO_LOGIN_USERNAME").filter(|s| !s.is_empty()).map(String::from),
            disable_regular_login: env.bool("PAPERLESS_DISABLE_REGULAR_LOGIN", "NO")?,
            redirect_login_to_sso: env.bool("PAPERLESS_REDIRECT_LOGIN_TO_SSO", "NO")?,
            account_allow_signups: env.bool("PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", "NO")?,
            account_default_groups: env.list("PAPERLESS_ACCOUNT_DEFAULT_GROUPS", &[]),
            socialaccount_allow_signups: env.bool("PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS", "yes")?,
            socialaccount_auto_signup: env.bool("PAPERLESS_SOCIAL_AUTO_SIGNUP", "NO")?,
            social_account_default_groups: env.list("PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS", &[]),
            social_account_sync_groups: env.bool("PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS", "NO")?,
            account_email_verification: if email_enabled {
                env.get_or("PAPERLESS_ACCOUNT_EMAIL_VERIFICATION", "optional")
            } else {
                "none".into()
            },
            account_default_http_protocol: env.get_or("PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL", "https"),
            token_throttle_rate: env.get_or("PAPERLESS_TOKEN_THROTTLE_RATE", "5/min"),
            email_port: env.int_or("PAPERLESS_EMAIL_PORT", 25)?,
            email_host_password: env.get_or("PAPERLESS_EMAIL_HOST_PASSWORD", ""),
            default_from_email: env.get_or("PAPERLESS_EMAIL_FROM", &email_host_user),
            email_use_tls: env.bool("PAPERLESS_EMAIL_USE_TLS", "NO")?,
            email_use_ssl: env.bool("PAPERLESS_EMAIL_USE_SSL", "NO")?,
            email_host,
            email_host_user,
            email_enabled,
            db,
            filename_format: env.get("PAPERLESS_FILENAME_FORMAT").map(String::from),
            filename_format_remove_none: env.bool("PAPERLESS_FILENAME_FORMAT_REMOVE_NONE", "NO")?,
            audit_log_enabled: env.bool("PAPERLESS_AUDIT_LOG_ENABLED", "true")?,
            empty_trash_delay: env.int_or("PAPERLESS_EMPTY_TRASH_DELAY", 30)?.max(1),
            app_title: env.get("PAPERLESS_APP_TITLE").map(String::from),
            app_logo: env.get("PAPERLESS_APP_LOGO").map(String::from),
            product_name: None,
            enable_update_check,
            task_workers: env.int_or("PAPERLESS_TASK_WORKERS", 1)?,
            worker_timeout: env.int_or("PAPERLESS_WORKER_TIMEOUT", 1800)?,
            tika_enabled: env.bool("PAPERLESS_TIKA_ENABLED", "NO")?,
            ai_enabled: env.bool("PAPERLESS_AI_ENABLED", "NO")?,
            enable_compression: env.bool("PAPERLESS_ENABLE_COMPRESSION", "yes")?,
            db_read_cache_enabled: env.bool("PAPERLESS_DB_READ_CACHE_ENABLED", "no")?,
            search_language: search_language_setting(&env, &ocr_language)?,
            ocr_language,
            advanced_fuzzy_search_threshold: env.float("PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD")?,
            remote_ocr_engine: env.get("PAPERLESS_REMOTE_OCR_ENGINE").map(String::from),
            remote_ocr_api_key: env.get("PAPERLESS_REMOTE_OCR_API_KEY").map(String::from),
            remote_ocr_endpoint: env.get("PAPERLESS_REMOTE_OCR_ENDPOINT").map(String::from),
            remote_ocr_mode,
            remote_ocr_allow_internal_endpoints: env.bool("PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS", "true")?,
            email_certificate_file: env.path_opt("PAPERLESS_EMAIL_CERTIFICATE_LOCATION"),
            email_allow_internal_hosts: env.bool("PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS", "true")?,
            gmail_oauth_enabled,
            outlook_oauth_enabled,
            oauth_callback_base_url: env.get("PAPERLESS_OAUTH_CALLBACK_BASE_URL").map(String::from),
            gmail_oauth_client_id: env.get("PAPERLESS_GMAIL_OAUTH_CLIENT_ID").map(String::from),
            gmail_oauth_client_secret: env.get("PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET").map(String::from),
            outlook_oauth_client_id: env.get("PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID").map(String::from),
            outlook_oauth_client_secret: env.get("PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET").map(String::from),
            env,
        })
    }

    /// HTTP header name for a Django META key (`HTTP_X_FORWARDED_PROTO` → `x-forwarded-proto`).
    /// `CONTENT_TYPE`/`CONTENT_LENGTH` are mapped as well; other non-`HTTP_` keys
    /// (e.g. `REMOTE_USER` as a WSGI variable) yield `None`.
    pub fn meta_key_to_header(meta_key: &str) -> Option<String> {
        match meta_key {
            "CONTENT_TYPE" => Some("content-type".into()),
            "CONTENT_LENGTH" => Some("content-length".into()),
            k => k.strip_prefix("HTTP_").map(|h| h.replace('_', "-").to_ascii_lowercase()),
        }
    }

    /// Django `validate_host` + `split_domain_port` against [`Config::allowed_hosts`]
    /// (with DEBUG and an empty list: `.localhost`, `127.0.0.1`, `[::1]`).
    pub fn host_allowed(&self, host: &str) -> bool {
        let (domain, _port) = split_domain_port(host);
        if domain.is_empty() {
            return false;
        }
        let defaults = [".localhost".to_string(), "127.0.0.1".into(), "[::1]".into()];
        let allowed: &[String] = if self.debug && self.allowed_hosts.is_empty() { &defaults } else { &self.allowed_hosts };
        allowed.iter().any(|p| p == "*" || is_same_domain(&domain, p))
    }
}

/// `ALLOWED_HOSTS` before the hostname from `PAPERLESS_URL`.
///
/// upstream fix 00096e025 (#14735, paperless-ngx `dev` after v3.4.2),
/// `paperless/settings/__init__.py::_get_allowed_hosts`: `["*"]` now only applies if **neither**
/// `PAPERLESS_ALLOWED_HOSTS` is set **nor** `PAPERLESS_URL` (non-empty). If one of them is
/// set, the list from `PAPERLESS_ALLOWED_HOSTS` (missing/empty → empty) plus `localhost`
/// (Docker healthcheck) applies; v3.4.2 had `["*", host]` with only `PAPERLESS_URL` set and thus allowed
/// every host. `allowed_hosts_env` is the raw value (`None` = not set).
pub fn initial_allowed_hosts(allowed_hosts_env: Option<&str>, paperless_url_set: bool) -> Vec<String> {
    let mut hosts: Vec<String> = if allowed_hosts_env.is_some() || paperless_url_set {
        allowed_hosts_env.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
    } else {
        vec!["*".into()]
    };
    if hosts != ["*"] {
        hosts.push("localhost".into());
    }
    hosts
}

/// Entry for `ALLOWED_HOSTS` from the hostname of `PAPERLESS_URL` or `None` if it is
/// invalid (startup error "has no valid hostname").
///
/// upstream fix 00096e025 (#14735), `_parse_paperless_url`: lowercased, **one** trailing dot
/// removed; empty, `*` or starting with `.` is invalid (otherwise the URL would become a
/// wildcard); IPv6 addresses get square brackets so that they match Django's `split_domain_port`
/// (`[::1]`). Accepts the hostname with or without brackets.
pub fn allowed_host_from_url_hostname(hostname: &str) -> Option<String> {
    let lower = hostname.trim_start_matches('[').trim_end_matches(']').to_lowercase();
    let h = lower.strip_suffix('.').unwrap_or(&lower);
    if h.is_empty() || h == "*" || h.starts_with('.') {
        return None;
    }
    Some(if h.contains(':') { format!("[{h}]") } else { h.to_string() })
}

fn base_dir_join(env: &Env, sub: &str) -> PathBuf {
    env.path("PAPERRUST_BASE_DIR", Path::new(".")).join(sub)
}

/// Django `split_domain_port`: (domain lowercased without trailing dot, port) or `("", "")`.
pub fn split_domain_port(host: &str) -> (String, String) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^([a-z0-9.-]+|\[[a-f0-9]*:[a-f0-9.:]+\])(?::([0-9]+))?$").expect("static regex")
    });
    let lower = host.to_lowercase();
    match re.captures(&lower) {
        Some(c) => {
            let d = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let p = c.get(2).map(|m| m.as_str()).unwrap_or("");
            (d.strip_suffix('.').unwrap_or(d).to_string(), p.to_string())
        }
        None => (String::new(), String::new()),
    }
}

/// Django `is_same_domain`.
pub fn is_same_domain(host: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let pattern = pattern.to_lowercase();
    (pattern.starts_with('.') && (host.ends_with(&pattern) || host == &pattern[1..])) || pattern == host
}

/// upstream `parse_db_settings`.
fn parse_db_settings(env: &Env, data_dir: &Path) -> Result<DbConfig, ConfigError> {
    let engine = match env.get("PAPERLESS_DBENGINE") {
        Some("postgresql") => DbEngine::Postgresql,
        Some("sqlite") => DbEngine::Sqlite,
        Some("mariadb") => DbEngine::Mariadb,
        Some(other) => {
            return Err(invalid(format!(
                "Environment variable 'PAPERRUST_DBENGINE' has invalid value '{other}'. Valid choices are: mariadb, postgresql, sqlite"
            )));
        }
        None => {
            if env.contains("PAPERLESS_DBHOST") { DbEngine::Postgresql } else { DbEngine::Sqlite }
        }
    };
    // PAPERLESS_DB_OPTIONS: "k=v,k2=v2", dot notation allowed.
    let mut options = BTreeMap::new();
    if let Some(o) = env.get("PAPERLESS_DB_OPTIONS") {
        for pair in o.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if let Some((k, v)) = pair.split_once('=') {
                let k = k.trim();
                if !k.is_empty() {
                    options.insert(k.to_string(), v.trim().to_string());
                }
            }
        }
    }
    let opt = |k: &str| options.get(k).cloned();
    let parse_u64 = |k: &str, v: &str| -> Result<u64, ConfigError> {
        v.trim().parse::<u64>().map_err(|_| invalid(format!("Error casting key '{k}' with value '{v}' to type 'int'")))
    };

    let default_ssl = if engine == DbEngine::Mariadb { "PREFERRED" } else { "prefer" };
    let port = match env.int("PAPERLESS_DBPORT")? {
        Some(p) => Some(u16::try_from(p).map_err(|_| invalid("PAPERRUST_DBPORT out of range"))?),
        None => None,
    };
    let timeout_key = if engine == DbEngine::Sqlite { "timeout" } else { "connect_timeout" };
    let mut connect_timeout = env.int("PAPERLESS_DB_TIMEOUT")?.map(|t| t.max(0) as u64);
    if let Some(v) = opt(timeout_key) {
        connect_timeout = Some(parse_u64(timeout_key, &v)?);
    }
    let mut pool_max_size = env.int("PAPERLESS_DB_POOLSIZE")?.map(|p| p.clamp(1, u32::MAX as i64) as u32);
    if let Some(v) = opt("pool.max_size") {
        pool_max_size = Some(parse_u64("pool.max_size", &v)?.clamp(1, u32::MAX as u64) as u32);
    }
    Ok(DbConfig {
        engine,
        host: env.get("PAPERLESS_DBHOST").map(String::from),
        port,
        name: env.get_or("PAPERLESS_DBNAME", "paperless"),
        user: env.get_or("PAPERLESS_DBUSER", "paperless"),
        password: env.get_or("PAPERLESS_DBPASS", "paperless"),
        sslmode: opt("sslmode").or_else(|| opt("ssl_mode")).unwrap_or_else(|| env.get_or("PAPERLESS_DBSSLMODE", default_ssl)),
        sslrootcert: opt("sslrootcert").or_else(|| env.get("PAPERLESS_DBSSLROOTCERT").map(String::from)),
        sslcert: opt("sslcert").or_else(|| env.get("PAPERLESS_DBSSLCERT").map(String::from)),
        sslkey: opt("sslkey").or_else(|| env.get("PAPERLESS_DBSSLKEY").map(String::from)),
        connect_timeout,
        pool_max_size,
        application_name: opt("application_name").unwrap_or_else(|| crate::brand::BIN_NAME.into()),
        require_auth: env.get("PAPERRUST_DB_REQUIRE_AUTH").filter(|v| !v.trim().is_empty()).map(String::from),
        sqlite_path: absolute(&data_dir.join("db.sqlite3")),
        options,
    })
}

// Review M1: Debug must not print secrets (SECRET_KEY, DB password, raw values).
impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env").field("vars", &format_args!("<{} Einträge, verborgen>", self.vars.len())).finish()
    }
}

impl std::fmt::Debug for DbConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbConfig").field("engine", &self.engine).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_env_like_init_env_file() {
        let vars: BTreeMap<String, String> = [
            ("PAPERLESS_DBPASS_FILE", "/run/secrets/db"),
            ("PAPERRUST_SECRET_KEY_FILE", "/run/secrets/key"),
            ("PAPERLESS_ADMIN_PASSWORD_FILE", "/missing"),
            ("PAPERLESS_MODEL_FILE", "/data/model.pickle"),
            ("PAPERLESS_OCR_SKIP_ARCHIVE_FILE", "with_text"),
            ("PAPERRUST_CLASSIFIER_MODEL_FILE", "/data/m.json"),
            ("PAPERLESS_FILENAME_FORMAT_FILE", "/run/secrets/fmt"),
            ("PAPERLESS_BIN_FILE", "/run/secrets/bin"),
            ("OTHER_FILE", "/run/secrets/db"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let read = |p: &Path| match p.to_str() {
            Some("/run/secrets/db") => Some(b"s3cret\n".to_vec()),
            Some("/run/secrets/key") => Some(b"k".to_vec()),
            Some("/run/secrets/fmt") => Some(b"{{ title }}".to_vec()),
            Some("/run/secrets/bin") => Some(vec![0xff, 0x00]),
            _ => None,
        };
        let (set, msgs) = plan_file_env(&vars, read);
        let set: BTreeMap<String, String> = set.into_iter().collect();
        assert_eq!(set.get("PAPERLESS_DBPASS").map(String::as_str), Some("s3cret\n"));
        assert_eq!(set.get("PAPERRUST_SECRET_KEY").map(String::as_str), Some("k"));
        assert_eq!(set.get("PAPERLESS_FILENAME_FORMAT").map(String::as_str), Some("{{ title }}"));
        assert_eq!(set.len(), 3, "{set:?}");
        assert!(msgs.contains(&FileEnvMessage::Missing { from: "PAPERLESS_ADMIN_PASSWORD_FILE".into() }));
        assert!(msgs.contains(&FileEnvMessage::TrailingNewline { from: "PAPERLESS_DBPASS_FILE".into() }));
        assert!(msgs.contains(&FileEnvMessage::Unusable { from: "PAPERLESS_BIN_FILE".into() }));
        // Messages never contain the content
        assert!(msgs.iter().all(|m| !m.to_string().contains("s3cret")));
    }

    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let mut v: Vec<(&str, &str)> = vec![("PAPERLESS_SECRET_KEY", "test-secret")];
        v.extend_from_slice(pairs);
        Config::from_env_map(Env::from_pairs(v.iter().copied()))
    }

    #[test]
    fn defaults() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.time_zone, "UTC");
        assert_eq!(c.allowed_hosts, vec!["*"]);
        assert_eq!(c.cors_allowed_origins, vec!["http://localhost:8000"]);
        assert_eq!(c.session_cookie_name, "sessionid");
        assert_eq!(c.session_cookie_age, 1_814_400);
        assert_eq!(c.logout_redirect_url, "/accounts/login/?loggedout=1");
        assert_eq!(c.db.engine, DbEngine::Sqlite);
        assert_eq!(c.http_remote_user_header_name, "HTTP_REMOTE_USER");
        assert!(c.secure_proxy_ssl_header.is_none());
        assert!(c.host_allowed("anything.example:8000"));
    }

    #[test]
    fn integration_fields() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.bind, "127.0.0.1:8080");
        assert_eq!(c.search_language.as_deref(), Some("en"));
        assert_eq!(c.advanced_fuzzy_search_threshold, None);
        assert_eq!(c.remote_ocr_mode, "always");
        assert!(c.email_allow_internal_hosts);
        assert!(!c.gmail_oauth_enabled);
        let c = cfg(&[
            ("PAPERLESS_OCR_LANGUAGE", "deu+eng"),
            ("PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD", "0.5"),
            ("PAPERLESS_REMOTE_OCR_ENGINE", ""),
            ("PAPERLESS_URL", "https://p.example"),
            ("PAPERLESS_GMAIL_OAUTH_CLIENT_ID", "id"),
            ("PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET", "s"),
        ])
        .unwrap();
        assert_eq!(c.search_language.as_deref(), Some("de"));
        assert_eq!(c.advanced_fuzzy_search_threshold, Some(0.5));
        assert_eq!(c.remote_ocr_engine.as_deref(), Some(""));
        assert!(c.gmail_oauth_enabled && !c.outlook_oauth_enabled);
        assert_eq!(cfg(&[("PAPERLESS_OCR_LANGUAGE", "chi_sim")]).unwrap().search_language, None);
        assert_eq!(cfg(&[("PAPERLESS_SEARCH_LANGUAGE", "german")]).unwrap().search_language.as_deref(), Some("german"));
        assert!(cfg(&[("PAPERLESS_SEARCH_LANGUAGE", "xx")]).is_err());
        assert!(cfg(&[("PAPERLESS_REMOTE_OCR_MODE", "never")]).is_err());
    }

    #[test]
    fn secret_required() {
        assert!(Config::from_env_map(Env::from_pairs([("PAPERLESS_SECRET_KEY", "change-me")])).is_err());
        assert!(Config::from_env_map(Env::from_pairs::<&str, &str>([])).is_err());
    }

    #[test]
    fn url_and_hosts() {
        let c = cfg(&[
            ("PAPERLESS_URL", "https://paper.example.com"),
            ("PAPERLESS_ALLOWED_HOSTS", "a.example, b.example"),
            ("PAPERLESS_COOKIE_PREFIX", "x_"),
            ("PAPERLESS_FORCE_SCRIPT_NAME", "/paperless"),
            ("PAPERLESS_PROXY_SSL_HEADER", r#"["HTTP_X_FORWARDED_PROTO", "https"]"#),
        ])
        .unwrap();
        assert_eq!(c.allowed_hosts, vec!["a.example", "b.example", "localhost", "paper.example.com"]);
        assert_eq!(c.csrf_trusted_origins, vec!["https://paper.example.com"]);
        assert_eq!(c.csrf_cookie_name, "x_csrftoken");
        assert_eq!(c.base_url, "/paperless/");
        assert_eq!(c.login_url, "/paperless/accounts/login/");
        assert_eq!(c.secure_proxy_ssl_header, Some(("HTTP_X_FORWARDED_PROTO".into(), "https".into())));
        assert!(c.host_allowed("PAPER.example.com."));
        assert!(c.host_allowed("localhost:8000"));
        assert!(!c.host_allowed("evil.example"));
        assert!(!c.host_allowed("bad host"));
        assert!(cfg(&[("PAPERLESS_URL", "nohost")]).is_err());
    }

    /// upstream fix 00096e025 (#14735): `test_allowed_hosts_at_startup` (all nine cases) and
    /// `test_paperless_url_without_valid_hostname_is_rejected`.
    #[test]
    fn allowed_hosts_at_startup_upstream_00096e025() {
        type Case<'a> = (Option<&'a str>, Option<&'a str>, &'a [&'a str], &'a str, Option<&'a str>);
        let cases: &[Case] = &[
            (None, None, &["*"], "other.example.com", None),
            (None, Some(""), &["localhost"], "localhost:8000", Some("other.example.com")),
            (None, Some("lan.example.com"), &["lan.example.com", "localhost"], "lan.example.com", Some("other.example.com")),
            (None, Some("*"), &["*"], "other.example.com", None),
            (Some("https://example.com:8443"), None, &["localhost", "example.com"], "example.com:8443", Some("other.example.com")),
            (Some("https://example.com."), Some(""), &["localhost", "example.com"], "example.com.", Some("other.example.com")),
            (Some("http://[::1]:8000"), None, &["localhost", "[::1]"], "[::1]:8000", Some("other.example.com")),
            (Some("https://example.com"), Some("*"), &["*", "example.com"], "other.example.com", None),
            (
                Some("https://example.com"),
                Some("lan.example.com"),
                &["lan.example.com", "localhost", "example.com"],
                "lan.example.com",
                Some("other.example.com"),
            ),
        ];
        for (url, hosts, expected, accepted, rejected) in cases {
            let mut pairs = Vec::new();
            if let Some(u) = url {
                pairs.push(("PAPERLESS_URL", *u));
            }
            if let Some(h) = hosts {
                pairs.push(("PAPERLESS_ALLOWED_HOSTS", *h));
            }
            let c = cfg(&pairs).unwrap();
            assert_eq!(c.allowed_hosts, *expected, "{url:?} {hosts:?}");
            assert!(c.host_allowed(accepted), "{url:?} {hosts:?} accepts {accepted}");
            if let Some(r) = rejected {
                assert!(!c.host_allowed(r), "{url:?} {hosts:?} rejects {r}");
            }
        }
        for url in ["example.com", "https://", "/just/a/path", "https://*", "https://.example.com"] {
            let e = cfg(&[("PAPERLESS_URL", url)]).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(e.contains("no valid hostname"), "{url}: {e}");
        }
    }

    #[test]
    fn db_settings() {
        let c = cfg(&[
            ("PAPERLESS_DBHOST", "db"),
            ("PAPERLESS_DBPORT", "5433"),
            ("PAPERLESS_DB_OPTIONS", "sslmode=require,pool.max_size=7"),
        ])
        .unwrap();
        assert_eq!(c.db.engine, DbEngine::Postgresql);
        assert_eq!(c.db.port, Some(5433));
        assert_eq!(c.db.sslmode, "require");
        assert_eq!(c.db.pool_max_size, Some(7));
        assert_eq!(c.db.name, "paperless");
        assert!(cfg(&[("PAPERLESS_DBENGINE", "oracle")]).is_err());
        assert!(cfg(&[("PAPERLESS_DEBUG", "maybe")]).is_err());
    }

    #[test]
    fn dotenv() {
        let v = parse_dotenv("# c\nPAPERLESS_A=1\nexport PAPERLESS_B=\"x y\"\nPAPERLESS_C='z' \nPAPERLESS_D=v # comment\n");
        assert_eq!(
            v,
            vec![
                ("PAPERLESS_A".into(), "1".into()),
                ("PAPERLESS_B".into(), "x y".into()),
                ("PAPERLESS_C".into(), "z".into()),
                ("PAPERLESS_D".into(), "v".into())
            ]
        );
    }

    #[test]
    fn names_primary_upstream_and_legacy() {
        // Primary name PAPERRUST_* for an upstream setting, PAPERLESS_* as fallback.
        let c = cfg(&[("PAPERRUST_TIME_ZONE", "Europe/Zurich"), ("PAPERLESS_TIME_ZONE", "UTC")]).unwrap();
        assert_eq!(c.time_zone, "Europe/Zurich");
        assert_eq!(cfg(&[("PAPERLESS_TIME_ZONE", "Europe/Berlin")]).unwrap().time_zone, "Europe/Berlin");
        // Secret also only under the new name.
        let c = Config::from_env_map(Env::from_pairs([("PAPERRUST_SECRET_KEY", "s")])).unwrap();
        assert_eq!(c.secret_key, "s");
        // Own setting: PAPERRUST_* primary, PAPERLESS_RS_* as transitional alias.
        assert_eq!(cfg(&[("PAPERRUST_BIND", "127.0.0.1:1")]).unwrap().bind, "127.0.0.1:1");
        assert_eq!(cfg(&[("PAPERLESS_RS_BIND", "127.0.0.1:2")]).unwrap().bind, "127.0.0.1:2");
        let both = cfg(&[("PAPERRUST_BIND", "127.0.0.1:1"), ("PAPERLESS_RS_BIND", "127.0.0.1:2")]).unwrap();
        assert_eq!(both.bind, "127.0.0.1:1");
        // Old code that still asks for PAPERLESS_RS_X sees PAPERRUST_X.
        let env = Env::from_pairs([("PAPERRUST_QUEUE", "postgres")]);
        assert_eq!(env.get("PAPERLESS_RS_QUEUE"), Some("postgres"));
        assert!(env.contains("PAPERLESS_RS_QUEUE"));
        assert_eq!(env.get_exact("PAPERLESS_RS_QUEUE"), None);
        assert_eq!(cfg(&[]).unwrap().db.application_name, "paperrust");
    }

    #[test]
    fn candidate_order() {
        assert_eq!(candidates("PAPERLESS_DBHOST"), ["PAPERRUST_DBHOST", "PAPERLESS_DBHOST"]);
        assert_eq!(candidates("PAPERRUST_BIND"), ["PAPERRUST_BIND", "PAPERLESS_RS_BIND"]);
        assert_eq!(candidates("PAPERLESS_RS_BIND"), ["PAPERRUST_BIND", "PAPERLESS_RS_BIND"]);
        assert_eq!(candidates("HOME"), ["HOME"]);
        let m: BTreeMap<String, String> = [("PAPERLESS_RS_LOG".to_string(), "debug".to_string())].into();
        assert_eq!(lookup(|k| m.get(k).cloned(), "PAPERRUST_LOG").as_deref(), Some("debug"));
    }

    #[test]
    fn process_alias_plan() {
        let vars: BTreeMap<String, String> = [
            ("PAPERLESS_RS_BIND", "127.0.0.1:9"),
            ("PAPERLESS_RS_LOG", "debug"),
            ("PAPERRUST_LOG", "info"),
            ("PAPERRUST_DBHOST", "db"),
            ("PAPERRUST_OCR_LANGUAGE", "deu"),
            ("PAPERLESS_OCR_LANGUAGE", "eng"),
            ("PAPERRUST_TIME_ZONE", "UTC"),
            ("PAPERLESS_TIME_ZONE", "UTC"),
            ("PAPERLESS_RS_AI_AUTO_MODE", "apply"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let (set, report) = plan_env_aliases(&vars);
        let set: BTreeMap<String, String> = set.into_iter().collect();
        // Transitional names → primary name (if missing), own settings not to PAPERLESS_*.
        assert_eq!(set.get("PAPERRUST_BIND").map(String::as_str), Some("127.0.0.1:9"));
        assert_eq!(set.get("PAPERRUST_AI_AUTO_MODE").map(String::as_str), Some("apply"));
        assert!(!set.contains_key("PAPERLESS_BIND") && !set.contains_key("PAPERRUST_LOG"));
        // Primary name of an upstream setting → PAPERLESS_* (for direct readers and Django).
        assert_eq!(set.get("PAPERLESS_DBHOST").map(String::as_str), Some("db"));
        assert_eq!(set.get("PAPERLESS_OCR_LANGUAGE").map(String::as_str), Some("deu"));
        assert!(!set.contains_key("PAPERLESS_TIME_ZONE"));
        assert_eq!(report.legacy_ignored, vec![("PAPERLESS_RS_LOG".to_string(), "PAPERRUST_LOG".to_string())]);
        assert_eq!(report.upstream_overridden, vec![("PAPERRUST_OCR_LANGUAGE".to_string(), "PAPERLESS_OCR_LANGUAGE".to_string())]);
        let env = Env::from_pairs(vars.clone());
        let w = alias_warnings(&env, &report);
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w.iter().any(|l| l.starts_with("PAPERLESS_RS_BIND is deprecated, please rename it to PAPERRUST_BIND")));
        assert!(w.iter().any(|l| l.starts_with("PAPERLESS_RS_LOG is ignored because PAPERRUST_LOG is set")));
    }

    #[test]
    fn retired_settings_warn_but_never_fail() {
        let env = Env::from_pairs([
            ("PAPERLESS_SECRET_KEY", "x"),
            ("PAPERLESS_RS_DJANGO_URL", "http://127.0.0.1:8000"),
            ("PAPERRUST_QUEUE", "celery"),
            ("PAPERLESS_REDIS", "redis://broker:6379"),
        ]);
        assert!(Config::from_env_map(env.clone()).is_ok());
        let w = alias_warnings(&env, &AliasReport::default());
        assert!(w.iter().any(|l| l.starts_with("PAPERRUST_DJANGO_URL is set but has no effect anymore")), "{w:?}");
        assert!(w.iter().any(|l| l.starts_with("PAPERRUST_QUEUE=celery is no longer supported")), "{w:?}");
        assert!(w.iter().any(|l| l.starts_with("PAPERLESS_REDIS is set but PaperRust does not use Redis")), "{w:?}");
        // no misleading "still honoured" for the transitional name
        assert!(!w.iter().any(|l| l.contains("PAPERLESS_RS_DJANGO_URL is deprecated")), "{w:?}");
        assert!(retired_warnings(&Env::from_pairs([("PAPERRUST_QUEUE", "postgres")])).is_empty());
    }

    #[test]
    fn own_settings_table() {
        let mut seen = std::collections::BTreeSet::new();
        for s in OWN_SETTINGS {
            assert!(s.name.starts_with(PREFIX), "{}", s.name);
            assert!(seen.insert(s.name), "doppelt: {}", s.name);
            assert!(!s.meaning.is_empty());
        }
        assert!(is_own_setting("PAPERRUST_DJANGO_URL") && !is_own_setting("PAPERRUST_DBHOST"));
    }

    /// docs/SETTINGS.md §2.1: overlay from the UI – process > DB > file.
    #[test]
    fn live_overlay_precedence() {
        let mut env = Env::from_pairs_with_file(
            [("PAPERLESS_ZZ_OV_PROC", "proc")],
            Path::new("p.conf"),
            [("PAPERRUST_ZZ_OV_FILE", "file"), ("PAPERLESS_ZZ_OV_PROC", "f2")],
        );
        let ov: BTreeMap<String, String> = [("PAPERLESS_ZZ_OV_PROC", "db"), ("PAPERLESS_ZZ_OV_FILE", "db2"), ("PAPERLESS_ZZ_OV_NEW", "db3")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        env.apply_overlay(&ov);
        assert_eq!(env.get("PAPERLESS_ZZ_OV_PROC"), Some("proc"));
        assert_eq!(env.get("PAPERLESS_ZZ_OV_FILE"), Some("db2"));
        assert_eq!(env.origin_exact("PAPERRUST_ZZ_OV_FILE"), None);
        assert_eq!(env.get("PAPERLESS_ZZ_OV_NEW"), Some("db3"));
        // global: var() and from_process()
        set_live_overlay([("PAPERLESS_ZZ_OV_GLOBAL".to_string(), "g".to_string())].into_iter().collect());
        assert_eq!(var("PAPERLESS_ZZ_OV_GLOBAL").as_deref(), Some("g"));
        assert_eq!(var("PAPERRUST_ZZ_OV_GLOBAL").as_deref(), None);
        assert_eq!(Env::from_process().get("PAPERLESS_ZZ_OV_GLOBAL"), Some("g"));
        assert_eq!(Env::from_process_raw().get("PAPERLESS_ZZ_OV_GLOBAL"), None);
        assert_eq!(vars_live().get("PAPERLESS_ZZ_OV_GLOBAL").map(String::as_str), Some("g"));
        set_live_overlay(BTreeMap::new());
        assert_eq!(var("PAPERLESS_ZZ_OV_GLOBAL"), None);
    }

    /// docs/SETTINGS.md §2.1: origin per name (process vs. configuration file), process wins.
    #[test]
    fn env_origin_per_key() {
        let p = Path::new("/etc/paperless.conf");
        let env = Env::from_pairs_with_file(
            [("PAPERLESS_EMAIL_HOST", "proc")],
            p,
            [("PAPERLESS_EMAIL_HOST", "file"), ("PAPERLESS_EMAIL_PORT", "587"), ("PAPERLESS_EMAIL_PORT", "2")],
        );
        assert_eq!(env.get("PAPERLESS_EMAIL_HOST"), Some("proc"));
        assert_eq!(env.origin_exact("PAPERLESS_EMAIL_HOST"), Some(EnvOrigin::Process));
        assert_eq!(env.get("PAPERLESS_EMAIL_PORT"), Some("587"));
        assert_eq!(env.origin_exact("PAPERLESS_EMAIL_PORT"), Some(EnvOrigin::File(p.to_path_buf())));
        assert_eq!(env.origin_exact("PAPERLESS_DBHOST"), None);
        assert_eq!(env.config_file(), Some(p));
        assert_eq!(Env::from_pairs([("A", "1")]).origin_exact("A"), Some(EnvOrigin::Process));
    }
}
