//! Layering and snapshot (docs/SETTINGS.md §2.1).
//!
//! * **Rule N** (class A/B without AC field): environment (`PAPERRUST_X` > `PAPERLESS_X` >
//!   `PAPERLESS_RS_X`) > `paperrust_setting` > configuration file > default. Environment locks.
//! * **Rule AC** (45 fields): column (bool: not NULL, otherwise "truthy") > environment/file >
//!   default ([`crate::appconfig::resolve`]); no lock.
//! * **Rule C/D/X**: environment/file only (or desktop files) > default.
//!
//! [`Settings`] is the immutable snapshot per process (in [`crate::reload`] behind
//! `ArcSwap`). [`Settings::env`] returns an [`Env`] in which the DB values of rule N are already
//! inserted – consumers read them with the same functions as at startup.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use pl_core::config::{Env, EnvOrigin};
use serde::Serialize;

use crate::appconfig::{self, AcRow};
use crate::schema::{Class, Effect, SCHEMA, SettingDef};

/// Keys set as environment by the desktop supervisor (comma-separated, §2.6).
pub const MANAGED_KEYS_VAR: &str = "PAPERRUST_SUPERVISOR_MANAGED_KEYS";

/// Origin of the effective value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Env,
    File,
    Db,
    #[serde(rename = "appconfig")]
    AppConfig,
    Desktop,
    Default,
}

/// Value of a row in `paperrust_setting` (secrets already decrypted).
#[derive(Clone, PartialEq)]
pub enum DbValue {
    Plain(String),
    /// Secret with a different `SECRET_KEY` or damaged: counts as not set.
    Unreadable,
}

/// Row from `paperrust_setting`.
#[derive(Clone)]
pub struct DbEntry {
    pub value: DbValue,
    pub rev: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<String>,
}

impl std::fmt::Debug for DbEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbEntry").field("rev", &self.rev).finish_non_exhaustive()
    }
}

/// Inputs of the layering.
#[derive(Debug, Clone, Default)]
pub struct Layers {
    /// Process environment + configuration file with origin per name.
    pub env: Env,
    /// `paperrust_setting` (canonical key → row).
    pub db: BTreeMap<String, DbEntry>,
    /// `paperless_applicationconfiguration` (first row).
    pub ac: Option<AcRow>,
    /// Display values from `desktop.toml` (from the supervisor, [`crate::desktop`]).
    pub desktop: BTreeMap<String, String>,
}

impl Layers {
    pub fn new(env: Env) -> Self {
        Self { env, ..Self::default() }
    }

    fn managed(&self) -> BTreeSet<String> {
        self.env
            .get(MANAGED_KEYS_VAR)
            .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
            .unwrap_or_default()
    }
}

/// Shadowed value (display "overrides environment/file").
#[derive(Clone, PartialEq, Serialize)]
pub struct Shadowed {
    pub source: Source,
    pub value: Option<String>,
}

/// Effective value of a key.
#[derive(Clone)]
pub struct Entry {
    pub def: &'static SettingDef,
    /// Effective value (canonical env string). **Secrets in plain text** – never output.
    pub value: Option<String>,
    pub source: Source,
    /// Variable name (environment), path (file) or `AC:<field>`.
    pub source_detail: Option<String>,
    pub shadowed: Option<Shadowed>,
    /// Locked in the GUI (environment/desktop sets the key, or class C/D/X).
    pub locked: bool,
    pub lock_reason: Option<&'static str>,
    /// Secret exists in the DB but is not readable (different `SECRET_KEY`).
    pub unreadable: bool,
    /// Row in `paperrust_setting` exists (even if shadowed).
    pub in_db: bool,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<String>,
    /// S key whose value differs from the start value.
    pub restart_pending: bool,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v: Option<&str> = if self.def.secret { self.value.as_ref().map(|_| "***") } else { self.value.as_deref() };
        f.debug_struct("Entry")
            .field("key", &self.def.key)
            .field("value", &v)
            .field("source", &self.source)
            .field("locked", &self.locked)
            .finish_non_exhaustive()
    }
}

/// Start values of the S keys (for `restart_pending`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BootValues(pub BTreeMap<&'static str, Option<String>>);

/// Immutable snapshot.
#[derive(Clone)]
pub struct Settings {
    /// State of `paperrust_setting_rev` at load time (0 = never written).
    pub rev: i64,
    entries: Vec<Entry>,
    env: Env,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings").field("rev", &self.rev).field("entries", &self.entries.len()).finish()
    }
}

/// Under rule N (GUI-capable, own storage)?
pub fn rule_n(def: &SettingDef) -> bool {
    def.class.gui_writable() && def.ac_field.is_none() && def.is_env()
}

/// First set name with matching origin.
fn lookup_origin<'e>(env: &'e Env, def: &SettingDef, file: bool) -> Option<(String, &'e str, EnvOrigin)> {
    for name in def.env_names() {
        if let Some(origin) = env.origin_exact(&name)
            && matches!(origin, EnvOrigin::File(_)) == file
        {
            let v = env.get_exact(&name)?;
            return Some((name, v, origin));
        }
    }
    None
}

fn origin_source(origin: &EnvOrigin, name: &str, managed: &BTreeSet<String>) -> (Source, String) {
    match origin {
        EnvOrigin::File(p) => (Source::File, p.display().to_string()),
        EnvOrigin::Process if managed.contains(name) => (Source::Desktop, name.to_string()),
        EnvOrigin::Process => (Source::Env, name.to_string()),
    }
}

impl Settings {
    /// Builds the snapshot. `boot` = start values (for `restart_pending`), `None` on first load.
    pub fn build(layers: &Layers, rev: i64, boot: Option<&BootValues>) -> Self {
        let managed = layers.managed();
        let mut entries = Vec::with_capacity(SCHEMA.len());
        let mut overlay: Vec<(&'static SettingDef, String)> = Vec::new();
        for def in SCHEMA {
            let mut e = Entry {
                def,
                value: None,
                source: Source::Default,
                source_detail: None,
                shadowed: None,
                locked: !def.class.gui_writable(),
                lock_reason: (!def.class.gui_writable()).then_some(match def.class {
                    Class::C => "bootstrap",
                    Class::D => "environment_only",
                    _ => "no_effect",
                }),
                unreadable: false,
                in_db: false,
                updated_at: None,
                updated_by: None,
                restart_pending: false,
            };
            let process = lookup_origin(&layers.env, def, false);
            let file = lookup_origin(&layers.env, def, true);
            if let Some(field) = def.ac_field {
                // Rule AC
                let column = layers.ac.as_ref().map(|r| r.get(field).clone()).unwrap_or(serde_json::Value::Null);
                let env_hit = process.as_ref().or(file.as_ref());
                let r = appconfig::resolve(def, &column, env_hit.map(|h| h.1), def.default);
                e.value = r.value;
                if r.from_column {
                    e.source = Source::AppConfig;
                    e.source_detail = Some(format!("AC:{field}"));
                    if let Some((name, v, origin)) = env_hit {
                        e.shadowed = Some(Shadowed { source: origin_source(origin, name, &managed).0, value: Some(v.to_string()) });
                    }
                } else if let Some((name, _, origin)) = env_hit {
                    let (s, d) = origin_source(origin, name, &managed);
                    e.source = s;
                    e.source_detail = Some(d);
                }
            } else if rule_n(def) {
                let db = layers.db.get(def.key);
                if let Some(row) = db {
                    e.in_db = true;
                    e.updated_at = row.updated_at;
                    e.updated_by.clone_from(&row.updated_by);
                    e.unreadable = row.value == DbValue::Unreadable;
                }
                let db_value = db.and_then(|r| match &r.value {
                    DbValue::Plain(v) => Some(v.clone()),
                    DbValue::Unreadable => None,
                });
                if let Some((name, v, origin)) = &process {
                    let (s, d) = origin_source(origin, name, &managed);
                    e.value = Some(v.to_string());
                    e.source = s;
                    e.source_detail = Some(d);
                    e.locked = true;
                    e.lock_reason = Some(if s == Source::Desktop { "desktop" } else { "environment" });
                    if let Some(dv) = db_value {
                        e.shadowed = Some(Shadowed { source: Source::Db, value: Some(dv) });
                    }
                } else if let Some(dv) = db_value {
                    e.value = Some(dv.clone());
                    e.source = Source::Db;
                    if let Some((_, fv, _)) = &file {
                        e.shadowed = Some(Shadowed { source: Source::File, value: Some(fv.to_string()) });
                    }
                    overlay.push((def, dv));
                } else if let Some((name, v, origin)) = &file {
                    let (s, d) = origin_source(origin, name, &managed);
                    e.value = Some(v.to_string());
                    e.source = s;
                    e.source_detail = Some(d);
                } else {
                    e.value = def.default.map(String::from);
                }
            } else if def.is_env() {
                // Rule C/D/X (and A/B keys without environment names do not exist)
                if let Some((name, v, origin)) = process.as_ref().or(file.as_ref()) {
                    let (s, d) = origin_source(origin, name, &managed);
                    e.value = Some(v.to_string());
                    e.source = s;
                    e.source_detail = Some(d);
                } else {
                    e.value = def.default.map(String::from);
                }
            } else {
                // desktop.toml/app.toml: belongs to the supervisor or the shell (WP7). Only the keys that
                // the supervisor accepts are changeable, and only under it.
                match layers.desktop.get(def.key) {
                    Some(v) => {
                        e.value = Some(v.clone());
                        e.source = Source::Desktop;
                        e.source_detail = Some("desktop.toml".into());
                    }
                    None => e.value = def.default.map(String::from),
                }
                let writable = crate::desktop::WRITABLE.contains(&def.key) && crate::desktop::control_dir(&layers.env).is_some();
                e.locked = !writable;
                e.lock_reason = (!writable).then_some("desktop");
            }
            if let (Some(boot), Effect::Server) = (boot, def.effect)
                && let Some(b) = boot.0.get(def.key)
            {
                e.restart_pending = *b != e.value;
            }
            entries.push(e);
        }
        // Effective env: DB values of rule N replace file values (process values win anyway).
        let env = if overlay.is_empty() {
            layers.env.clone()
        } else {
            let mut vars = layers.env.vars().clone();
            for (def, v) in overlay {
                for name in def.env_names() {
                    vars.remove(&name);
                }
                vars.insert(def.key.to_string(), v);
            }
            Env::from_pairs(vars)
        };
        Self { rev, entries, env }
    }

    /// Start values of all S keys of this snapshot.
    pub fn boot_values(&self) -> BootValues {
        BootValues(self.entries.iter().filter(|e| e.def.effect == Effect::Server).map(|e| (e.def.key, e.value.clone())).collect())
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Entry for a key (also name variants, [`crate::schema::find`]).
    pub fn get(&self, key: &str) -> Option<&Entry> {
        let def = crate::schema::find(key)?;
        self.entries.iter().find(|e| std::ptr::eq(e.def, def))
    }

    /// Effective value (canonical env string).
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|e| e.value.as_deref())
    }

    /// Environment with DB values inserted (rule N); reading with `Env::bool/int/…` as at startup.
    /// AC fields appear here as in Django's `settings.*` (environment only); their DB values are provided by
    /// [`Settings::value`] or [`crate::appconfig`].
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// Number of keys with "restart needed".
    pub fn restart_pending_count(&self) -> usize {
        self.entries.iter().filter(|e| e.restart_pending).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn db(v: &str) -> DbEntry {
        DbEntry { value: DbValue::Plain(v.into()), rev: 1, updated_at: None, updated_by: Some("admin".into()) }
    }

    #[test]
    fn rule_n_precedence_and_lock() {
        let env = Env::from_pairs_with_file(
            [("PAPERLESS_EMAIL_PORT", "2525")],
            Path::new("/etc/paperless.conf"),
            [("PAPERLESS_EMAIL_HOST", "file.example"), ("PAPERLESS_EMAIL_PORT", "26"), ("PAPERRUST_LOG", "debug")],
        );
        let mut l = Layers::new(env);
        l.db.insert("PAPERLESS_EMAIL_HOST".into(), db("db.example"));
        l.db.insert("PAPERLESS_EMAIL_PORT".into(), db("587"));
        let s = Settings::build(&l, 7, None);
        // DB > file
        let h = s.get("PAPERLESS_EMAIL_HOST").unwrap();
        assert_eq!((h.value.as_deref(), h.source, h.locked), (Some("db.example"), Source::Db, false));
        assert_eq!(h.shadowed.as_ref().unwrap().source, Source::File);
        assert_eq!(s.env().get("PAPERLESS_EMAIL_HOST"), Some("db.example"));
        // Environment > DB, locked
        let p = s.get("PAPERLESS_EMAIL_PORT").unwrap();
        assert_eq!((p.value.as_deref(), p.source, p.locked), (Some("2525"), Source::Env, true));
        assert_eq!(p.source_detail.as_deref(), Some("PAPERLESS_EMAIL_PORT"));
        assert_eq!(p.shadowed.as_ref().unwrap().value.as_deref(), Some("587"));
        // File > default
        let lg = s.get("PAPERRUST_LOG").unwrap();
        assert_eq!((lg.value.as_deref(), lg.source), (Some("debug"), Source::File));
        assert_eq!(lg.source_detail.as_deref(), Some(Path::new("/etc/paperless.conf").display().to_string().as_str()));
        // Standard
        let tls = s.get("PAPERLESS_EMAIL_USE_TLS").unwrap();
        assert_eq!((tls.value.as_deref(), tls.source), (Some("no"), Source::Default));
        // Alias name finds the entry
        assert!(std::ptr::eq(s.get("PAPERRUST_EMAIL_HOST").unwrap().def, h.def));
        assert_eq!(s.rev, 7);
    }

    #[test]
    fn primary_name_wins_and_desktop_managed() {
        let env = Env::from_pairs([
            ("PAPERRUST_TIME_ZONE", "Europe/Zurich"),
            ("PAPERLESS_TIME_ZONE", "UTC"),
            (MANAGED_KEYS_VAR, "PAPERRUST_TIME_ZONE"),
        ]);
        let s = Settings::build(&Layers::new(env), 0, None);
        let tz = s.get("PAPERLESS_TIME_ZONE").unwrap();
        assert_eq!((tz.value.as_deref(), tz.source, tz.lock_reason), (Some("Europe/Zurich"), Source::Desktop, Some("desktop")));
    }

    #[test]
    fn ac_rule_no_lock_and_shadow() {
        let env = Env::from_pairs([("PAPERLESS_OCR_LANGUAGE", "deu"), ("PAPERLESS_OCR_PAGES", "4")]);
        let mut l = Layers::new(env);
        let mut row = AcRow::default();
        row.values.insert("language".into(), serde_json::json!("deu+eng"));
        row.values.insert("pages".into(), serde_json::json!(0));
        row.values.insert("deskew".into(), serde_json::json!(false));
        l.ac = Some(row);
        // DB row for an AC key is ignored (no copy in paperrust_setting)
        l.db.insert("PAPERLESS_OCR_LANGUAGE".into(), db("fra"));
        let s = Settings::build(&l, 0, None);
        let lang = s.get("PAPERLESS_OCR_LANGUAGE").unwrap();
        assert_eq!((lang.value.as_deref(), lang.source, lang.locked), (Some("deu+eng"), Source::AppConfig, false));
        assert_eq!(lang.shadowed.as_ref().unwrap().value.as_deref(), Some("deu"));
        let pages = s.get("PAPERLESS_OCR_PAGES").unwrap();
        assert_eq!((pages.value.as_deref(), pages.source), (Some("4"), Source::Env));
        assert_eq!(s.value("PAPERLESS_OCR_DESKEW"), Some("false"));
        // Django's settings.OCR_LANGUAGE stays the environment
        assert_eq!(s.env().get("PAPERLESS_OCR_LANGUAGE"), Some("deu"));
    }

    #[test]
    fn unreadable_secret_falls_through() {
        let mut l = Layers::new(Env::from_pairs([("X", "1")]));
        l.db.insert(
            "PAPERLESS_EMAIL_HOST_PASSWORD".into(),
            DbEntry { value: DbValue::Unreadable, rev: 1, updated_at: None, updated_by: None },
        );
        let s = Settings::build(&l, 1, None);
        let e = s.get("PAPERLESS_EMAIL_HOST_PASSWORD").unwrap();
        assert!(e.unreadable && e.in_db);
        assert_eq!((e.value.as_deref(), e.source), (Some(""), Source::Default));
        assert!(!format!("{e:?}").contains("hunter"));
    }

    #[test]
    fn bootstrap_env_only_and_restart_pending() {
        let mut l = Layers::new(Env::from_pairs([("PAPERLESS_DBHOST", "db")]));
        l.db.insert("PAPERLESS_DBHOST".into(), db("ignored"));
        let s0 = Settings::build(&l, 0, None);
        let h = s0.get("PAPERLESS_DBHOST").unwrap();
        assert_eq!((h.value.as_deref(), h.locked, h.lock_reason), (Some("db"), true, Some("bootstrap")));
        let boot = s0.boot_values();
        assert_eq!(s0.restart_pending_count(), 0);
        l.db.insert("PAPERLESS_TIME_ZONE".into(), db("Europe/Berlin"));
        let s1 = Settings::build(&l, 1, Some(&boot));
        assert!(s1.get("PAPERLESS_TIME_ZONE").unwrap().restart_pending);
        assert_eq!(s1.restart_pending_count(), 1);
        // L keys never "restart needed"
        l.db.insert("PAPERLESS_EMAIL_HOST".into(), db("x"));
        assert!(!Settings::build(&l, 2, Some(&boot)).get("PAPERLESS_EMAIL_HOST").unwrap().restart_pending);
    }
}
