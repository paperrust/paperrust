//! Mode and server profiles: `<data folder>/app.toml` (docs/REMOTE-APP.md §2).
//!
//! ```toml
//! mode = "remote"              # "local" | "remote"
//! active_profile = "a1b2c3"
//!
//! [[profile]]
//! id = "a1b2c3"
//! name = "Büro"
//! url = "https://docs.example.ch/"
//! flavor = "paperrust"         # "paperrust" | "ngx"
//! ui = "server"
//! tls_pin = ""                 # SPKI-SHA-256 (hex), only for a confirmed own certificate
//! allow_http = false
//! extra_origins = []
//! sync = false
//!
//! [updates]                    # automatic updates (docs/DESKTOP.md §23)
//! enabled = true
//! channel = "stable"           # "stable" | "beta"
//! interval_hours = 24
//! endpoint = ""                # leer = Standard (GitHub-Release `updater`)
//!
//! [ui]
//! design = "modern"            # most recently chosen UI: "modern" | "classic" (default)
//! ```
//!
//! No secrets in the file (the login lives in the profile's WebView data folder).
//! Missing fields get default values (older/handwritten files), unknown ones are
//! ignored; invalid profiles (ID, address) are discarded on load instead of locking the app.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::remote;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Local,
    Remote,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" | "lokal" => Some(Self::Local),
            "remote" | "fern" => Some(Self::Remote),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

/// Counterpart (detected on connecting, REMOTE-APP §3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    Paperrust,
    Ngx,
    /// Profile from a file without `flavor` – detected on the next connect.
    #[default]
    Unknown,
}

impl Flavor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Paperrust => "PaperRust",
            Self::Ngx => "paperless-ngx",
            Self::Unknown => "unbekannt",
        }
    }
}

/// UI: `server` = mode 2a (UI from the server), `app` = 2b/3 (not built yet).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ui {
    #[default]
    Server,
    App,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Base address: origin + optional subpath, always with `/` at the end.
    pub url: String,
    #[serde(default)]
    pub flavor: Flavor,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub tls_pin: String,
    #[serde(default)]
    pub allow_http: bool,
    #[serde(default)]
    pub extra_origins: Vec<String>,
    #[serde(default)]
    pub sync: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub active_profile: String,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
    /// `[updates]` (Z4, [`crate::update::UpdateSettings`]).
    #[serde(default)]
    pub updates: crate::update::UpdateSettings,
    /// `[ui]` – most recently chosen UI (docs/DESIGN2.md §6); not written without a choice.
    #[serde(default, skip_serializing_if = "UiPrefs::is_unset")]
    pub ui: UiPrefs,
}

/// `[ui]` in `app.toml`. `design` as text, so that an unknown value does not make the file unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPrefs {
    /// `"modern"` | `"classic"` (leer/unbekannt = klassisch)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub design: String,
}

impl UiPrefs {
    fn is_unset(&self) -> bool {
        self.design.is_empty()
    }

    pub fn design(&self) -> crate::design::Design {
        crate::design::Design::parse(&self.design).unwrap_or(crate::design::Design::Classic)
    }
}

/// Profile IDs are folder names: only `[0-9a-z]`, 1–32 characters.
pub fn valid_profile_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

pub fn new_profile_id(existing: &[Profile]) -> String {
    loop {
        let id = paperrust_desktop::config::random_secret(8).to_ascii_lowercase();
        if !existing.iter().any(|p| p.id == id) {
            return id;
        }
    }
}

/// Display name: trimmed, without control characters, at most 60 characters; empty → host of the address.
pub fn clean_name(name: &str, url: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(60).collect();
    if n.is_empty() {
        tauri::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "Server".into())
    } else {
        n
    }
}

impl AppConfig {
    pub fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        let mut cfg: Self = toml::from_str(text).map_err(|e| format!("app.toml: {e}"))?;
        let notes = cfg.sanitize();
        Ok((cfg, notes))
    }

    /// Discard invalid profiles, normalize addresses, repair `active_profile`.
    /// Returns hints for the log.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.profiles.retain_mut(|p| {
            if !valid_profile_id(&p.id) || !seen.insert(p.id.clone()) {
                notes.push(format!("Profil mit ungültiger/doppelter ID {:?} ignoriert", p.id));
                return false;
            }
            match remote::parse_server_url(&p.url) {
                Ok(u) => {
                    if !u.https && u.scope == remote::HostScope::Public {
                        notes.push(format!("Profil {}: http zu öffentlicher Adresse nicht erlaubt – ignoriert", p.id));
                        return false;
                    }
                    p.url = u.base;
                }
                Err(e) => {
                    notes.push(format!("Profil {}: {e} – ignoriert", p.id));
                    return false;
                }
            }
            p.name = clean_name(&p.name, &p.url);
            p.extra_origins.retain(|o| remote::parse_origin(o).is_some_and(|n| &n == o));
            p.tls_pin = p.tls_pin.trim().to_ascii_lowercase();
            if !p.tls_pin.is_empty() && !remote::valid_pin(&p.tls_pin) {
                notes.push(format!("Profil {}: tls_pin ungültig – entfernt", p.id));
                p.tls_pin.clear();
            }
            true
        });
        if !self.profiles.iter().any(|p| p.id == self.active_profile) {
            self.active_profile = self.profiles.first().map(|p| p.id.clone()).unwrap_or_default();
        }
        notes
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| format!("app.toml: {e}"))
    }

    /// `None` = file missing (first start).
    pub fn load(path: &Path) -> Result<Option<(Self, Vec<String>)>, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::parse(&t).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} lesen: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text =
            format!("# PaperRust Desktop – Betriebsart und Server-Profile (docs/REMOTE-APP.md). Keine Geheimnisse.\n{}", self.to_toml()?);
        let tmp = path.with_extension("toml.tmp");
        paperrust_desktop::config::write_private(&tmp, text.as_bytes()).map_err(|e| format!("{} schreiben: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{} ersetzen: {e}", path.display()))
    }

    pub fn active(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == self.active_profile)
    }

    pub fn profile_mut(&mut self, id: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    /// Replace the profile with the same base address or create a new one; becomes active.
    pub fn upsert(&mut self, mut p: Profile) -> String {
        if let Some(old) = self.profiles.iter_mut().find(|o| o.url == p.url) {
            p.id = old.id.clone();
            *old = p;
            return old.id.clone();
        }
        if !valid_profile_id(&p.id) || self.profiles.iter().any(|o| o.id == p.id) {
            p.id = new_profile_id(&self.profiles);
        }
        let id = p.id.clone();
        self.profiles.push(p);
        id
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.sanitize();
        before != self.profiles.len()
    }
}

/// What the shell starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// No `app.toml`, no local data, no preset: question "Wie möchtest du PaperRust nutzen?".
    FirstRun,
    Local,
    Remote,
}

/// Decision at startup. `env_mode` = `PAPERRUST_DESKTOP_MODE`; `managed_local` = first start answered via
/// environment (`PAPERRUST_DESKTOP_ADMIN_*`/`_IMPORT*`); `local_data` = `desktop.toml` or
/// `pgdata/` present (installation from before the remote app → stays local, without a question).
pub fn decide(cfg: Option<&AppConfig>, env_mode: Option<Mode>, managed_local: bool, local_data: bool) -> Start {
    let m = env_mode.or(cfg.map(|c| c.mode)).or_else(|| (managed_local || local_data).then_some(Mode::Local));
    match m {
        Some(Mode::Local) => Start::Local,
        Some(Mode::Remote) => Start::Remote,
        None => Start::FirstRun,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, url: &str) -> Profile {
        Profile {
            id: id.into(),
            name: "Büro".into(),
            url: url.into(),
            flavor: Flavor::Paperrust,
            ui: Ui::Server,
            tls_pin: String::new(),
            allow_http: false,
            extra_origins: vec![],
            sync: false,
        }
    }

    #[test]
    fn roundtrip() {
        let mut p = profile("a1b2c3", "https://docs.example.ch/");
        p.extra_origins = vec!["https://login.example.ch".into()];
        p.tls_pin = "ab".repeat(32);
        let cfg = AppConfig {
            mode: Mode::Remote,
            active_profile: "a1b2c3".into(),
            profiles: vec![p, profile("x9", "http://192.168.1.20:8000/")],
            updates: crate::update::UpdateSettings { channel: crate::update::Channel::Beta, interval_hours: 12, ..Default::default() },
            ui: UiPrefs { design: "modern".into() },
        };
        let text = cfg.to_toml().unwrap();
        assert!(text.contains("[ui]") && text.contains("design = \"modern\""), "{text}");
        assert!(text.contains("[[profile]]"), "{text}");
        assert!(text.contains("[updates]") && text.contains("channel = \"beta\""), "{text}");
        assert!(text.contains("mode = \"remote\""));
        let (back, notes) = AppConfig::parse(&text).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, cfg);
        // file round trip
        let dir = std::env::temp_dir().join(format!("p1-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("app.toml");
        cfg.save(&f).unwrap();
        assert_eq!(AppConfig::load(&f).unwrap().unwrap().0, cfg);
        assert!(AppConfig::load(&dir.join("fehlt.toml")).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ui_design_lenient() {
        let (c, _) = AppConfig::parse(
            "mode = \"local\"
[ui]
design = \"modern\"
",
        )
        .unwrap();
        assert_eq!(c.ui.design(), crate::design::Design::Modern);
        let (c, _) = AppConfig::parse(
            "mode = \"local\"
[ui]
design = \"bunt\"
",
        )
        .unwrap();
        assert_eq!(c.ui.design(), crate::design::Design::Classic, "unbekannt = klassisch");
        let (c, _) = AppConfig::parse(
            "mode = \"local\"
",
        )
        .unwrap();
        assert_eq!(c.ui.design(), crate::design::Design::Classic);
        assert!(!c.to_toml().unwrap().contains("[ui]"), "ohne Wahl kein [ui]");
    }

    #[test]
    fn missing_fields_get_defaults() {
        let (c, notes) =
            AppConfig::parse("mode = \"remote\"\n[[profile]]\nid = \"abc\"\nname = \"\"\nurl = \"https://Docs.Example.ch\"\nzukunft = 1\n")
                .unwrap();
        assert!(notes.is_empty());
        let p = &c.profiles[0];
        assert_eq!(c.active_profile, "abc", "aktives Profil ergänzt");
        assert_eq!(p.url, "https://docs.example.ch/", "normalisiert");
        assert_eq!(p.name, "docs.example.ch");
        assert_eq!(p.flavor, Flavor::Unknown);
        assert_eq!(p.ui, Ui::Server);
        assert!(!p.allow_http && !p.sync && p.tls_pin.is_empty() && p.extra_origins.is_empty());
        // completely empty = local, no profiles
        let (c, _) = AppConfig::parse("").unwrap();
        assert_eq!(c.mode, Mode::Local);
        assert!(c.active().is_none());
    }

    #[test]
    fn invalid_profiles_are_dropped() {
        let (c, notes) = AppConfig::parse(
            r#"
active_profile = "gone"
[[profile]]
id = "../x"
name = "a"
url = "https://a.example"
[[profile]]
id = "ok1"
name = "b"
url = "http://example.com"
[[profile]]
id = "ok2"
name = "c"
url = "https://user@c.example"
[[profile]]
id = "ok3"
name = "d"
url = "http://10.0.0.5:8000"
tls_pin = "kaputt"
extra_origins = ["https://sso.example", "https://sso.example/pfad", "javascript:alert(1)"]
"#,
        )
        .unwrap();
        assert_eq!(c.profiles.len(), 1, "{notes:?}");
        assert_eq!(c.profiles[0].id, "ok3");
        assert_eq!(c.active_profile, "ok3");
        assert!(c.profiles[0].tls_pin.is_empty());
        assert_eq!(c.profiles[0].extra_origins, vec!["https://sso.example".to_string()]);
        assert_eq!(notes.len(), 4, "{notes:?}");
    }

    #[test]
    fn upsert_and_remove() {
        let mut c = AppConfig::default();
        let a = c.upsert(profile("", "https://a.example/"));
        assert!(valid_profile_id(&a) && a.len() == 8);
        let b = c.upsert(profile("", "https://b.example/"));
        assert_ne!(a, b);
        let mut again = profile("", "https://a.example/");
        again.name = "Neu".into();
        assert_eq!(c.upsert(again), a, "gleiche Adresse = gleiches Profil");
        assert_eq!(c.profiles.len(), 2);
        assert_eq!(c.profiles[0].name, "Neu");
        c.active_profile = a.clone();
        assert!(c.remove(&a));
        assert_eq!(c.active_profile, b, "aktives Profil rückt nach");
        assert!(!c.remove("nichtda"));
    }

    #[test]
    fn start_decision() {
        let remote = AppConfig { mode: Mode::Remote, ..AppConfig::default() };
        assert_eq!(decide(None, None, false, false), Start::FirstRun);
        assert_eq!(decide(None, None, false, true), Start::Local, "alte Installation");
        assert_eq!(decide(None, None, true, false), Start::Local, "verwaltet");
        assert_eq!(decide(Some(&remote), None, false, true), Start::Remote);
        assert_eq!(decide(Some(&remote), Some(Mode::Local), false, false), Start::Local);
        assert_eq!(decide(None, Some(Mode::Remote), true, true), Start::Remote);
        assert_eq!(Mode::parse(" Remote "), Some(Mode::Remote));
        assert_eq!(Mode::parse("x"), None);
    }

    #[test]
    fn names_and_ids() {
        assert!(valid_profile_id("a1b2c3"));
        for bad in ["", "A1", "a-b", "../a", &"a".repeat(33)] {
            assert!(!valid_profile_id(bad), "{bad}");
        }
        assert_eq!(clean_name("  Büro\u{7}  ", "https://x.example/"), "Büro");
        assert_eq!(clean_name("", "https://x.example/"), "x.example");
        assert_eq!(clean_name(&"z".repeat(100), "https://x/").len(), 60);
    }
}
