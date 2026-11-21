//! Storage `paperrust_setting` + history (docs/SETTINGS.md §2.2, §2.3, §2.5 no. 1, §2.7).
//!
//! * One change = one transaction: values + history, **one** new `rev = nextval(…)` for all
//!   changed rows, then `pg_notify('paperrust_settings', '{"rev":N,"keys":[…]}')`
//!   (delivered on commit). Writers are serialized via an advisory lock so that
//!   `expected_rev` is checked atomically.
//! * AC keys go via [`crate::appconfig::write`] into the same Django row (no copy),
//!   history entry `AC:<field>`.
//! * Secrets: encrypted ([`crate::crypto`]); always `***` in the history.

use std::collections::BTreeMap;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use pl_core::config::Env;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, PgPool, Row};

use crate::appconfig;
use crate::crypto::SecretBox;
use crate::layer::{DbEntry, DbValue, Layers, Settings, rule_n};
use crate::schema::{SettingDef, find};
use crate::value::{self, ValueError};

/// Channel for LISTEN/NOTIFY.
pub const NOTIFY_CHANNEL: &str = "paperrust_settings";
/// Placeholder for secrets in history/export/logs.
pub const MASK: &str = "***";
/// Migrations of this crate (registration: `pl_cli::migrate::paperrust_migrations`).
pub const MIGRATION_0001: (&str, &str) = ("pl_settings.0001_paperrust_setting", include_str!("../migrations/0001_paperrust_setting.sql"));

/// Who changes (for history and `updated_by`).
#[derive(Debug, Clone, Default)]
pub struct Actor {
    pub user_id: Option<i32>,
    pub user_name: Option<String>,
    pub remote_addr: Option<IpAddr>,
}

/// Kind of change (column `action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Set,
    Reset,
    Adopt,
    Import,
    Cli,
    Rekey,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Set => "set",
            Action::Reset => "reset",
            Action::Adopt => "adopt",
            Action::Import => "import",
            Action::Cli => "cli",
            Action::Rekey => "rekey",
        }
    }
}

/// New value of a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Typed JSON (API); `Value::Null` = reset.
    Json(Value),
    /// Canonical env string (CLI, import, switchover).
    Raw(String),
    /// To default (delete DB row or AC column NULL).
    Reset,
}

/// Options of [`apply`].
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions<'a> {
    /// 409 `rev_conflict` if the current state differs.
    pub expected_rev: Option<i64>,
    /// Environment for the lock check (409 `locked_by_environment`); `None` = do not check
    /// (CLI rescue path `--force`, switchover `adopt`).
    pub env: Option<&'a Env>,
    /// Only set where no DB value exists yet (`--if-unset`, desktop switchover §2.6).
    pub if_unset: bool,
}

/// Error when writing.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("unknown setting: {0}")]
    Unknown(String),
    /// Class C/D/X or desktop file – not writable via the DB.
    #[error("setting {0} cannot be changed here (bootstrap/environment only)")]
    NotWritable(String),
    #[error("settings were changed meanwhile (current rev {current})")]
    RevConflict { current: i64 },
    #[error("set via environment, cannot be changed here: {}", .0.join(", "))]
    LockedByEnvironment(Vec<String>),
    #[error("invalid value(s): {}", .0.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<ValueError>),
}

impl StoreError {
    /// v2 error code (§3).
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::Db(_) => "internal",
            StoreError::Unknown(_) => "unknown_setting",
            StoreError::NotWritable(_) => "not_writable",
            StoreError::RevConflict { .. } => "rev_conflict",
            StoreError::LockedByEnvironment(_) => "locked_by_environment",
            StoreError::Invalid(_) => "invalid",
        }
    }
}

/// Current state (`0` = never written). Also `0` without table/sequence.
pub async fn current_rev(conn: &mut PgConnection) -> Result<i64, sqlx::Error> {
    if !installed(conn).await? {
        return Ok(0);
    }
    sqlx::query_scalar("SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM paperrust_setting_rev").fetch_one(&mut *conn).await
}

/// Is the migration installed?
pub async fn installed(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT to_regclass('public.paperrust_setting') IS NOT NULL AND to_regclass('public.paperrust_setting_rev') IS NOT NULL",
    )
    .fetch_one(&mut *conn)
    .await
}

/// Installs the migration (idempotent).
pub async fn migrate(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(MIGRATION_0001.1).execute(&mut *conn).await?;
    Ok(())
}

/// All rows of `paperrust_setting` (secrets decrypted; different `SECRET_KEY` →
/// [`DbValue::Unreadable`] + warning). Unknown keys are skipped.
pub async fn load_db(conn: &mut PgConnection, sb: &SecretBox) -> Result<BTreeMap<String, DbEntry>, sqlx::Error> {
    let mut out = BTreeMap::new();
    if !installed(conn).await? {
        return Ok(out);
    }
    let rows =
        sqlx::query("SELECT key, value, secret_ct, secret_kid, rev, updated_at, updated_by_name FROM paperrust_setting ORDER BY key")
            .fetch_all(&mut *conn)
            .await?;
    for r in rows {
        let key: String = r.try_get(0)?;
        let value: Option<String> = r.try_get(1)?;
        let ct: Option<Vec<u8>> = r.try_get(2)?;
        let kid: Option<String> = r.try_get(3)?;
        let v = match (value, ct) {
            (Some(v), _) => DbValue::Plain(v),
            (None, Some(ct)) => match sb.open(&key, kid.as_deref().unwrap_or(""), &ct) {
                Ok(v) => DbValue::Plain(v),
                Err(e) => {
                    tracing::warn!(key = %key, error = %e, "stored secret is unreadable – treated as unset, please enter it again");
                    DbValue::Unreadable
                }
            },
            (None, None) => continue,
        };
        out.insert(key, DbEntry { value: v, rev: r.try_get(4)?, updated_at: r.try_get(5)?, updated_by: r.try_get(6)? });
    }
    Ok(out)
}

/// Loads all layers (environment + `paperrust_setting` + ApplicationConfiguration) and the state.
pub async fn load_layers(conn: &mut PgConnection, env: Env, sb: &SecretBox) -> Result<(Layers, i64), sqlx::Error> {
    let rev = current_rev(conn).await?;
    let db = load_db(conn, sb).await?;
    let ac = appconfig::load(conn).await?;
    let desktop = crate::desktop::control_dir(&env).map(|d| crate::desktop::read_values(&d)).unwrap_or_default();
    Ok((Layers { env, db, ac, desktop }, rev))
}

/// Loads a snapshot.
pub async fn load_settings(
    pool: &PgPool,
    env: Env,
    sb: &SecretBox,
    boot: Option<&crate::layer::BootValues>,
) -> Result<Settings, sqlx::Error> {
    // State and values must match: `nextval('paperrust_setting_rev')` in `apply` is not
    // transactional, so the new state is visible even before the COMMIT. Without the
    // shared lock (against the exclusive one in `apply`) a snapshot with a new `rev`
    // but old values arose – the notification after the COMMIT then counted as "unchanged" and
    // subscribers/live overlay stayed on the old values.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtext('paperrust_setting'))").execute(&mut *tx).await?;
    let (layers, rev) = load_layers(&mut tx, env, sb).await?;
    tx.commit().await?;
    Ok(Settings::build(&layers, rev, boot))
}

enum Planned {
    Row { def: &'static SettingDef, raw: Option<String> },
    Ac { def: &'static SettingDef, value: Value },
}

impl std::fmt::Debug for Planned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // without values (secrets)
        match self {
            Planned::Row { def, .. } => write!(f, "Row({})", def.key),
            Planned::Ac { def, .. } => write!(f, "Ac({})", def.key),
        }
    }
}

fn mask(def: &SettingDef, v: Option<&str>) -> Option<String> {
    v.map(|v| if def.secret { MASK.to_string() } else { v.to_string() })
}

fn plan(changes: &[(String, Input)], env: Option<&Env>) -> Result<Vec<Planned>, StoreError> {
    let mut out = Vec::new();
    let mut invalid = Vec::new();
    let mut locked = Vec::new();
    for (key, input) in changes {
        let def = find(key).ok_or_else(|| StoreError::Unknown(key.clone()))?;
        if def.ac_field.is_some() {
            let json = match input {
                Input::Reset => Value::Null,
                Input::Json(v) => v.clone(),
                Input::Raw(s) => match value::parse(def, s) {
                    Ok(_) => value::to_json(def, s),
                    Err(e) => {
                        invalid.push(e);
                        continue;
                    }
                },
            };
            match appconfig::validate(def, &json) {
                Ok(v) => out.push(Planned::Ac { def, value: v }),
                Err(e) => invalid.push(e),
            }
            continue;
        }
        if !rule_n(def) {
            return Err(StoreError::NotWritable(def.key.to_string()));
        }
        if let Some(env) = env {
            // Rule N: process environment locks (file does not).
            if def.env_names().iter().any(|n| env.origin_exact(n) == Some(pl_core::config::EnvOrigin::Process)) {
                locked.push(def.key.to_string());
                continue;
            }
        }
        let raw = match input {
            Input::Reset | Input::Json(Value::Null) => None,
            Input::Json(v) => match value::from_json(def, v) {
                Ok(r) => Some(r),
                Err(e) => {
                    invalid.push(e);
                    continue;
                }
            },
            Input::Raw(s) => match value::parse(def, s) {
                Ok(_) => Some(s.clone()),
                Err(e) => {
                    invalid.push(e);
                    continue;
                }
            },
        };
        out.push(Planned::Row { def, raw });
    }
    if !locked.is_empty() {
        return Err(StoreError::LockedByEnvironment(locked));
    }
    if !invalid.is_empty() {
        return Err(StoreError::Invalid(invalid));
    }
    Ok(out)
}

/// Writes changes atomically. Returns: new state (unchanged if there was nothing to do).
pub async fn apply(
    pool: &PgPool,
    sb: &SecretBox,
    changes: &[(String, Input)],
    opts: &ApplyOptions<'_>,
    actor: &Actor,
    action: Action,
) -> Result<i64, StoreError> {
    let planned = plan(changes, opts.env)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_setting'))").execute(&mut *tx).await?;
    let current = current_rev(&mut tx).await?;
    if let Some(exp) = opts.expected_rev
        && exp != current
    {
        return Err(StoreError::RevConflict { current });
    }
    let rev: i64 = sqlx::query_scalar("SELECT nextval('paperrust_setting_rev')").fetch_one(&mut *tx).await?;
    let mut keys = Vec::new();
    let mut ac_changes: Vec<(&'static SettingDef, Value)> = Vec::new();
    for p in &planned {
        match p {
            Planned::Row { def, raw } => {
                let old: Option<(Option<String>, Option<Vec<u8>>)> =
                    sqlx::query_as("SELECT value, secret_ct FROM paperrust_setting WHERE key = $1 FOR UPDATE")
                        .bind(def.key)
                        .fetch_optional(&mut *tx)
                        .await?;
                if opts.if_unset && old.is_some() {
                    continue;
                }
                let old_value = old.as_ref().map(|(v, ct)| if ct.is_some() { MASK.to_string() } else { v.clone().unwrap_or_default() });
                match raw {
                    None => {
                        if old.is_none() {
                            continue;
                        }
                        sqlx::query("DELETE FROM paperrust_setting WHERE key = $1").bind(def.key).execute(&mut *tx).await?;
                    }
                    Some(v) => {
                        let (value, ct, kid) = if def.secret {
                            (None, Some(sb.seal(def.key, v)), Some(sb.kid().to_string()))
                        } else {
                            (Some(v.clone()), None, None)
                        };
                        sqlx::query(
                            "INSERT INTO paperrust_setting (key, value, secret_ct, secret_kid, rev, updated_at, updated_by, updated_by_name)
                             VALUES ($1, $2, $3, $4, $5, now(), $6, $7)
                             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, secret_ct = EXCLUDED.secret_ct,
                               secret_kid = EXCLUDED.secret_kid, rev = EXCLUDED.rev, updated_at = now(),
                               updated_by = EXCLUDED.updated_by, updated_by_name = EXCLUDED.updated_by_name",
                        )
                        .bind(def.key)
                        .bind(value)
                        .bind(ct)
                        .bind(kid)
                        .bind(rev)
                        .bind(actor.user_id)
                        .bind(&actor.user_name)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
                history(&mut tx, def.key, old_value, mask(def, raw.as_deref()), if raw.is_none() { Action::Reset } else { action }, actor)
                    .await?;
                keys.push(def.key.to_string());
            }
            Planned::Ac { def, value } => ac_changes.push((def, value.clone())),
        }
    }
    if !ac_changes.is_empty() {
        let current_row = appconfig::load(&mut tx).await?;
        if opts.if_unset {
            ac_changes.retain(|(d, _)| current_row.as_ref().is_none_or(|r| r.get(d.ac_field.unwrap_or_default()).is_null()));
        }
        let old = appconfig::write(&mut tx, &ac_changes).await?;
        for ((def, new), old) in ac_changes.iter().zip(old) {
            let fmt = |v: &Value| {
                if v.is_null() {
                    None
                } else {
                    Some(if def.secret { MASK.to_string() } else { appconfig::to_env_string(v).unwrap_or_default() })
                }
            };
            let key = format!("AC:{}", def.ac_field.unwrap_or_default());
            history(&mut tx, &key, fmt(&old), fmt(new), if new.is_null() { Action::Reset } else { action }, actor).await?;
            keys.push(def.key.to_string());
        }
    }
    if keys.is_empty() {
        // Nothing changed: discard the transaction (rev not consumed? nextval is not
        // transactional – the state jumps anyway; consumers only compare for inequality).
        tx.rollback().await?;
        return Ok(current);
    }
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(NOTIFY_CHANNEL)
        .bind(json!({"rev": rev, "keys": keys}).to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(rev)
}

async fn history(
    conn: &mut PgConnection,
    key: &str,
    old: Option<String>,
    new: Option<String>,
    action: Action,
    actor: &Actor,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO paperrust_setting_history (key, old_value, new_value, action, user_id, user_name, remote_addr)
         VALUES ($1, $2, $3, $4, $5, $6, $7::text::inet)",
    )
    .bind(key)
    .bind(old)
    .bind(new)
    .bind(action.as_str())
    .bind(actor.user_id)
    .bind(&actor.user_name)
    .bind(actor.remote_addr.map(|a| a.to_string()))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A history entry.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryRow {
    pub id: i64,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub action: String,
    pub at: DateTime<Utc>,
    pub user_name: Option<String>,
}

/// History, newest first; `before` = cursor (id).
pub async fn history_list(
    conn: &mut PgConnection,
    key: Option<&str>,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<HistoryRow>, sqlx::Error> {
    if !installed(conn).await? {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, key, old_value, new_value, action, at, user_name FROM paperrust_setting_history
         WHERE ($1::text IS NULL OR key = $1) AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3",
    )
    .bind(key)
    .bind(before)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(HistoryRow {
                id: r.try_get(0)?,
                key: r.try_get(1)?,
                old_value: r.try_get(2)?,
                new_value: r.try_get(3)?,
                action: r.try_get(4)?,
                at: r.try_get(5)?,
                user_name: r.try_get(6)?,
            })
        })
        .collect()
}

/// Export of all DB values (`paperrust_setting` + AC columns, §2.7). Secrets only with
/// `include_secrets` in plain text, otherwise `{"secret":true,"is_set":…}`.
pub async fn export(conn: &mut PgConnection, sb: &SecretBox, include_secrets: bool) -> Result<Value, sqlx::Error> {
    let rev = current_rev(conn).await?;
    let db = load_db(conn, sb).await?;
    let mut settings = Map::new();
    for (k, e) in &db {
        let Some(def) = find(k) else { continue };
        let v = match (&e.value, def.secret, include_secrets) {
            (DbValue::Plain(v), false, _) | (DbValue::Plain(v), true, true) => Value::String(v.clone()),
            (DbValue::Plain(_), true, false) => json!({"secret": true, "is_set": true}),
            (DbValue::Unreadable, _, _) => {
                json!({"secret": true, "is_set": true, "unreadable": true})
            }
        };
        settings.insert(k.clone(), v);
    }
    let mut ac = Map::new();
    if let Some(row) = appconfig::load(conn).await? {
        for def in appconfig::defs() {
            let f = def.ac_field.unwrap_or_default();
            let v = row.get(f);
            if v.is_null() {
                continue;
            }
            ac.insert(f.to_string(), if def.secret && !include_secrets { json!({"secret": true, "is_set": true}) } else { v.clone() });
        }
    }
    Ok(json!({"format": "paperrust-settings", "version": 1, "rev": rev, "settings": settings, "appconfig": ac}))
}

/// `KEY=value` lines (way back to paperless-ngx, §5 no. 5). Secrets only with `include_secrets`.
pub fn export_env(settings: &Settings, include_secrets: bool) -> String {
    let mut out = String::from("# PaperRust settings export (database values)\n");
    for e in settings.entries() {
        if !matches!(e.source, crate::layer::Source::Db | crate::layer::Source::AppConfig) {
            continue;
        }
        let Some(v) = &e.value else { continue };
        if e.def.secret && !include_secrets {
            out.push_str(&format!("# {}=*** (secret, use --include-secrets)\n", e.def.key));
            continue;
        }
        let q = v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        out.push_str(&format!("{}=\"{q}\"\n", e.def.key));
    }
    out
}

/// Converts an export ([`export`]) into changes. Masked secrets are skipped.
pub fn import_changes(doc: &Value) -> Result<Vec<(String, Input)>, String> {
    if doc.get("format").and_then(Value::as_str) != Some("paperrust-settings") {
        return Err("not a paperrust-settings export".into());
    }
    let mut out = Vec::new();
    if let Some(m) = doc.get("settings").and_then(Value::as_object) {
        for (k, v) in m {
            match v {
                Value::String(s) => out.push((k.clone(), Input::Raw(s.clone()))),
                Value::Object(_) => {}
                other => return Err(format!("{k}: unexpected value {other}")),
            }
        }
    }
    if let Some(m) = doc.get("appconfig").and_then(Value::as_object) {
        for (f, v) in m {
            let def = appconfig::defs().find(|d| d.ac_field == Some(f.as_str())).ok_or_else(|| format!("unknown appconfig field {f}"))?;
            if v.get("secret").is_some() && !matches!(def.ty, crate::schema::Ty::Json { .. }) {
                continue;
            }
            out.push((def.key.to_string(), Input::Json(v.clone())));
        }
    }
    Ok(out)
}

/// Re-encrypts all secrets from `old` to `new` (`manage settings rekey`). Returns: number of
/// re-keyed rows; ones that cannot be decrypted stay unchanged (warning).
pub async fn rekey(pool: &PgPool, old: &SecretBox, new: &SecretBox, actor: &Actor) -> Result<usize, StoreError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_setting'))").execute(&mut *tx).await?;
    let rows: Vec<(String, Vec<u8>, Option<String>)> =
        sqlx::query_as("SELECT key, secret_ct, secret_kid FROM paperrust_setting WHERE secret_ct IS NOT NULL FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;
    let mut n = 0;
    let mut keys = Vec::new();
    let rev: i64 = sqlx::query_scalar("SELECT nextval('paperrust_setting_rev')").fetch_one(&mut *tx).await?;
    for (key, ct, kid) in rows {
        if kid.as_deref() == Some(new.kid()) {
            continue;
        }
        match old.open(&key, kid.as_deref().unwrap_or(""), &ct) {
            Ok(plain) => {
                sqlx::query("UPDATE paperrust_setting SET secret_ct = $2, secret_kid = $3, rev = $4 WHERE key = $1")
                    .bind(&key)
                    .bind(new.seal(&key, &plain))
                    .bind(new.kid())
                    .bind(rev)
                    .execute(&mut *tx)
                    .await?;
                history(&mut tx, &key, Some(MASK.into()), Some(MASK.into()), Action::Rekey, actor).await?;
                keys.push(key);
                n += 1;
            }
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "rekey: secret not readable with the old SECRET_KEY, left unchanged")
            }
        }
    }
    if n > 0 {
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(NOTIFY_CHANNEL)
            .bind(json!({"rev": rev, "keys": keys}).to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rejects_bootstrap_locked_and_invalid() {
        assert!(matches!(plan(&[("PAPERLESS_DBHOST".into(), Input::Raw("x".into()))], None), Err(StoreError::NotWritable(_))));
        assert!(matches!(plan(&[("NOPE".into(), Input::Raw("x".into()))], None), Err(StoreError::Unknown(_))));
        let env = Env::from_pairs([("PAPERRUST_EMAIL_HOST", "smtp")]);
        let e = plan(&[("PAPERLESS_EMAIL_HOST".into(), Input::Raw("x".into()))], Some(&env)).unwrap_err();
        assert_eq!(e.code(), "locked_by_environment");
        let e = plan(&[("PAPERLESS_EMAIL_PORT".into(), Input::Json(json!("abc")))], None).unwrap_err();
        assert_eq!(e.code(), "invalid");
        // File does not lock
        let fenv = Env::from_pairs_with_file(
            Vec::<(String, String)>::new(),
            std::path::Path::new("p"),
            [("PAPERLESS_EMAIL_HOST".to_string(), "f".to_string())],
        );
        assert!(plan(&[("PAPERLESS_EMAIL_HOST".into(), Input::Raw("x".into()))], Some(&fenv)).is_ok());
        // AC: environment does not lock, serializer rules apply
        let env = Env::from_pairs([("PAPERLESS_OCR_PAGES", "3")]);
        assert!(plan(&[("PAPERLESS_OCR_PAGES".into(), Input::Json(json!(2)))], Some(&env)).is_ok());
        assert_eq!(plan(&[("PAPERLESS_OCR_PAGES".into(), Input::Json(json!(0)))], Some(&env)).unwrap_err().code(), "invalid");
    }

    #[test]
    fn import_skips_masked_secrets() {
        let doc = json!({"format":"paperrust-settings","version":1,"settings":{
            "PAPERLESS_EMAIL_HOST":"smtp.example","PAPERLESS_EMAIL_HOST_PASSWORD":{"secret":true,"is_set":true}},
            "appconfig":{"pages":2,"llm_api_key":{"secret":true,"is_set":true},"user_args":{"a":1}}});
        let ch = import_changes(&doc).unwrap();
        assert_eq!(ch.len(), 3, "{ch:?}");
        assert!(ch.iter().any(|(k, _)| k == "PAPERLESS_OCR_USER_ARGS"));
        assert!(import_changes(&json!({})).is_err());
    }
}
