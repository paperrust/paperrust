//! `desktop.toml`: settings created and maintained by the supervisor.
//!
//! Contains secrets (DB password, `PAPERLESS_SECRET_KEY`) → file readable only by the user
//! (Unix 0600; Windows: `%LOCALAPPDATA%` is user-only by default ACL). `Debug` shows
//! no secrets.

use std::fmt;
use std::path::Path;

use rand::distributions::{Alphanumeric, DistString};
use serde::{Deserialize, Serialize};

use crate::ports::{DEFAULT_APP_PORT, DEFAULT_DB_PORT};
use crate::{DesktopError, Result, io_err};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopConfig {
    pub version: u32,
    pub database: DatabaseConfig,
    pub server: ServerSettings,
    #[serde(default)]
    pub setup: SetupState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Most recently used port (preferred port at the next start).
    pub port: u16,
    pub user: String,
    pub password: String,
    pub name: String,
    /// `builtin` (default, C.UTF-8, platform independent) or `icu` (language-correct sorting,
    /// only if the bundled binaries contain ICU). Only affects `initdb`.
    #[serde(default = "default_locale")]
    pub locale_provider: String,
    /// Application role (D-N3): `paperrust serve`/`manage` connect as this role without
    /// superuser rights (owner of the database `name`). `user` stays the cluster superuser for
    /// initdb/createdb/pg_dump of the supervisor.
    #[serde(default = "default_app_user")]
    pub app_user: String,
    /// Empty in configurations from before D-N3 – created and saved at the next start.
    #[serde(default)]
    pub app_password: String,
}

fn default_app_user() -> String {
    "paperless".into()
}

/// ICU (`und`): language-correct sorting like Docker Postgres with `en_US.utf8` (`a,A,b,B` instead of
/// `A,B,a,b`). Included in the theseus-rs binaries of all platforms (manifest.json 17.11.1,
/// checked 2026-11-09). Only affects `initdb`; existing clusters keep their provider.
fn default_locale() -> String {
    "icu".into()
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSettings {
    pub port: u16,
    pub secret_key: String,
    /// Time zone for `PAPERLESS_TIME_ZONE`; empty = the system time zone cannot be determined portably
    /// without an extra crate, hence default UTC as upstream.
    #[serde(default = "default_tz")]
    pub time_zone: String,
    /// `PAPERLESS_OCR_LANGUAGE`.
    #[serde(default = "default_ocr")]
    pub ocr_language: String,
    /// Open the browser on start.
    #[serde(default = "yes")]
    pub open_browser: bool,
    /// "Share on the local network" (default off): the app additionally listens on the LAN
    /// (`0.0.0.0:<port>`), share links use the LAN address (`PAPERLESS_URL`). Login
    /// stays mandatory; only `/share/…` is reachable without login, as upstream. docs/DESKTOP.md
    /// section 21.
    #[serde(default)]
    pub lan_sharing: bool,
    /// Address of the app on the LAN for share links (IPv4 or hostname, without port); empty = IPv4 of the
    /// network card with the default route, determined at startup.
    #[serde(default)]
    pub lan_address: String,
}

fn default_tz() -> String {
    "UTC".into()
}
fn default_ocr() -> String {
    "deu+eng".into()
}
fn yes() -> bool {
    true
}

/// Progress of the first start (each step individually, so an aborted first start
/// resumes at the right place next time).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupState {
    #[serde(default)]
    pub cluster_initialized: bool,
    #[serde(default)]
    pub database_created: bool,
    #[serde(default)]
    pub superuser_created: bool,
    /// Windows ACL or Unix 0700 applied to the data directory (M4, once).
    #[serde(default)]
    pub home_protected: bool,
    /// `time_zone`/`ocr_language` moved into the database (docs/SETTINGS.md §2.6, once):
    /// afterwards the supervisor no longer sets them as environment (otherwise they would be locked
    /// in the UI). The fields in `[server]` stay readable (way back).
    #[serde(default)]
    pub settings_migrated: bool,
    /// User name of the local owner: automatic login applies to them
    /// (docs/DESKTOP.md "Anmeldung im Desktop-Betrieb"). Empty until the first start with this
    /// version; then created automatically or the first active superuser is adopted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_user: String,
}

impl fmt::Debug for DesktopConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopConfig")
            .field("version", &self.version)
            .field("db_port", &self.database.port)
            .field("db_user", &self.database.user)
            .field("db_name", &self.database.name)
            .field("db_password", &"***")
            .field("db_app_user", &self.database.app_user)
            .field("db_app_password", &"***")
            .field("app_port", &self.server.port)
            .field("secret_key", &"***")
            .field("setup", &self.setup)
            .finish()
    }
}

/// Random secret from `[A-Za-z0-9]` (no special characters: ends up in command-line
/// environments, `--pwfile` and connection URLs without escaping).
pub fn random_secret(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

impl DesktopConfig {
    /// New configuration with fresh secrets.
    pub fn generate() -> Self {
        Self {
            version: CONFIG_VERSION,
            database: DatabaseConfig {
                port: DEFAULT_DB_PORT,
                user: "paperrust".into(),
                password: random_secret(32),
                name: "paperless".into(),
                locale_provider: default_locale(),
                app_user: default_app_user(),
                app_password: random_secret(32),
            },
            server: ServerSettings {
                port: DEFAULT_APP_PORT,
                secret_key: random_secret(64),
                time_zone: default_tz(),
                ocr_language: default_ocr(),
                open_browser: true,
                lan_sharing: false,
                lan_address: String::new(),
            },
            setup: SetupState::default(),
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        let cfg: Self = toml::from_str(text).map_err(|e| DesktopError::Config { path: path.display().to_string(), msg: e.to_string() })?;
        cfg.validate(path)?;
        Ok(cfg)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        let bad = |msg: &str| Err(DesktopError::Config { path: path.display().to_string(), msg: msg.into() });
        if self.version != CONFIG_VERSION {
            return bad(&format!("unbekannte Version {} (erwartet {CONFIG_VERSION})", self.version));
        }
        let ident_ok = |s: &str| !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ident_ok(&self.database.user) || !ident_ok(&self.database.name) || !ident_ok(&self.database.app_user) {
            return bad("database.user/app_user/name: nur [A-Za-z0-9_], 1–63 Zeichen");
        }
        if self.database.app_user == self.database.user {
            return bad("database.app_user muss sich von database.user (Superuser) unterscheiden");
        }
        if !self.database.app_password.is_empty()
            && (self.database.app_password.len() < 16 || !self.database.app_password.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return bad("database.app_password: mindestens 16 Zeichen [A-Za-z0-9]");
        }
        if self.database.password.len() < 16 || !self.database.password.chars().all(|c| c.is_ascii_alphanumeric()) {
            return bad("database.password: mindestens 16 Zeichen [A-Za-z0-9]");
        }
        if self.server.secret_key.len() < 32 || self.server.secret_key == "change-me" {
            return bad("server.secret_key: mindestens 32 Zeichen");
        }
        let lan = self.server.lan_address.trim();
        if !lan.is_empty() && (lan.len() > 253 || !lan.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
            return bad("server.lan_address: IPv4-Adresse oder Rechnername (ohne Port), z. B. 192.168.1.20");
        }
        if !matches!(self.database.locale_provider.as_str(), "builtin" | "icu") {
            return bad("database.locale_provider: builtin oder icu");
        }
        if self.database.port == 0 || self.server.port == 0 || self.database.port == self.server.port {
            return bad("Ports müssen gesetzt und verschieden sein");
        }
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| DesktopError::Other(format!("desktop.toml serialisieren: {e}")))
    }

    /// Loads `path`; if the file is missing, a new one is created and saved.
    /// Returns: (configuration, newly created?).
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((Self::parse(&text, path)?, false)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::generate();
                cfg.save(path)?;
                Ok((cfg, true))
            }
            Err(e) => Err(io_err(format!("{} lesen", path.display()))(e)),
        }
    }

    /// Save atomically (temporary file + rename), Unix permissions 0600.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.to_toml()?;
        let tmp = path.with_extension("toml.tmp");
        write_private(&tmp, text.as_bytes()).map_err(io_err(format!("{} schreiben", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(io_err(format!("{} ersetzen", path.display())))
    }
}

/// Write a file that only the user may read.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_valid_and_random() {
        let a = DesktopConfig::generate();
        let b = DesktopConfig::generate();
        a.validate(Path::new("x")).unwrap();
        assert_ne!(a.database.password, b.database.password);
        assert_ne!(a.server.secret_key, b.server.secret_key);
        assert_eq!(a.server.secret_key.len(), 64);
        assert!(a.database.password.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn debug_hides_secrets() {
        let c = DesktopConfig::generate();
        let d = format!("{c:?}");
        assert!(!d.contains(&c.database.password));
        assert!(!d.contains(&c.server.secret_key));
        assert!(!d.contains(&c.database.app_password));
    }

    #[test]
    fn roundtrip_and_reload_keeps_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("desktop.toml");
        let (a, created) = DesktopConfig::load_or_create(&p).unwrap();
        assert!(created);
        let (b, created) = DesktopConfig::load_or_create(&p).unwrap();
        assert!(!created);
        assert_eq!(a, b);
        let mut c = b.clone();
        c.setup.cluster_initialized = true;
        c.database.port = 50_000;
        c.save(&p).unwrap();
        assert_eq!(DesktopConfig::load_or_create(&p).unwrap().0, c);
        assert!(!p.with_extension("toml.tmp").exists());
    }

    #[test]
    fn rejects_bad_values() {
        let base = DesktopConfig::generate();
        let check = |f: &dyn Fn(&mut DesktopConfig)| {
            let mut c = base.clone();
            f(&mut c);
            DesktopConfig::parse(&c.to_toml().unwrap(), Path::new("d.toml")).is_err()
        };
        assert!(check(&|c| c.database.user = "a;drop".into()));
        assert!(check(&|c| c.database.password = "short".into()));
        assert!(check(&|c| c.server.secret_key = "change-me".into()));
        assert!(check(&|c| c.server.port = c.database.port));
        assert!(check(&|c| c.database.locale_provider = "libc".into()));
        assert!(check(&|c| c.version = 2));
        assert!(check(&|c| c.database.app_user = c.database.user.clone()));
        assert!(check(&|c| c.database.app_password = "kurz".into()));
        assert!(check(&|c| c.database.app_user = "x'y".into()));
        // Configuration from before D-N3 (without app_*): valid, password empty → supervisor creates it
        let old: String = base.to_toml().unwrap().lines().filter(|l| !l.starts_with("app_")).collect::<Vec<_>>().join("\n");
        let parsed = DesktopConfig::parse(&old, Path::new("d")).unwrap();
        assert_eq!(parsed.database.app_user, "paperless");
        assert!(parsed.database.app_password.is_empty());
        assert!(DesktopConfig::parse("version = 1\nunknown = 1", Path::new("d")).is_err());
        // Required values are not silently missing: setup is optional, server is not
        let t = base.to_toml().unwrap();
        let without_setup: String = t.split("[setup]").next().unwrap().to_string();
        assert!(DesktopConfig::parse(&without_setup, Path::new("d")).is_ok());
    }

    #[test]
    fn lan_sharing_is_off_by_default_and_optional() {
        let base = DesktopConfig::generate();
        assert!(!base.server.lan_sharing && base.server.lan_address.is_empty());
        // Configuration without the new keys (older version): valid, sharing off
        let old: String = base.to_toml().unwrap().lines().filter(|l| !l.starts_with("lan_")).collect::<Vec<_>>().join(
            "
",
        );
        let parsed = DesktopConfig::parse(&old, Path::new("d")).unwrap();
        assert!(!parsed.server.lan_sharing);
        let mut on = base.clone();
        on.server.lan_sharing = true;
        on.server.lan_address = "192.168.1.20".into();
        assert_eq!(DesktopConfig::parse(&on.to_toml().unwrap(), Path::new("d")).unwrap(), on);
        for bad in ["192.168.1.20:8000", "http://x", "a b", "x/y"] {
            let mut c = base.clone();
            c.server.lan_address = bad.into();
            assert!(DesktopConfig::parse(&c.to_toml().unwrap(), Path::new("d")).is_err(), "{bad}");
        }
    }
}
