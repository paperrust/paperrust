//! `/rs/api/v2/settings` – system settings (docs/SETTINGS.md §3, WP5).
//!
//! * Data: `pl_settings` (schema, layering, storage). AC keys go into the same row
//!   `paperless_applicationconfiguration` as v1 `/api/config/` (no divergence, §2.8).
//! * Permissions:
//!   - Read/write system settings: superuser only.
//!   - AC keys additionally with the v1 permissions of `/api/config/` (pl-system): read
//!     `paperless.view_applicationconfiguration`, change `paperless.change_applicationconfiguration`.
//!   - `desktop.*`/`app.*`: superuser **and** loopback (otherwise invisible or 403).
//!   - Restart: superuser + password confirmation + `restart_supported`.
//! * Secrets are write-only: never a value in responses, only `is_set`.
//! * Errors in v2 format `{"error":{"code","message","fields"}}`: 409 `rev_conflict`,
//!   409 `locked_by_environment`, 409 `not_writable`, 422 `invalid` with `fields`.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_core::AppState;
use pl_domain::Principal;
use pl_settings::layer::{Entry, Source};
use pl_settings::schema::{SCHEMA, SettingDef, Store as DefStore};
use pl_settings::store::{self, Action, Actor, ApplyOptions, Input, StoreError};
use pl_settings::{Settings, SettingsHandle};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::http::{Caller, V2Json};

/// v1 permissions of `/api/config/`.
pub const PERM_VIEW_AC: &str = "paperless.view_applicationconfiguration";
pub const PERM_CHANGE_AC: &str = "paperless.change_applicationconfiguration";

/// Errors of this module (own codes according to §3).
#[derive(Debug)]
pub struct SettingsError {
    status: StatusCode,
    code: &'static str,
    message: String,
    fields: Option<Box<Map<String, Value>>>,
    extra: Option<Box<(&'static str, Value)>>,
}

impl SettingsError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            fields: None,
            extra: None,
        }
    }
    pub(crate) fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "You do not have permission to perform this action.",
        )
    }
    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Not found.")
    }
    pub(crate) fn invalid(fields: Map<String, Value>) -> Self {
        let mut e = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "Invalid value(s).",
        );
        e.fields = Some(Box::new(fields));
        e
    }
    pub(crate) fn field(key: &str, msg: impl Into<String>) -> Self {
        let mut m = Map::new();
        m.insert(key.into(), json!([msg.into()]));
        Self::invalid(m)
    }
    pub(crate) fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!(target: "paperrust.api_v2", "settings: {e}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal server error.",
        )
    }
}

impl From<StoreError> for SettingsError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Db(e) => Self::internal(e),
            StoreError::Unknown(k) => Self::field(&k, "Unknown setting."),
            StoreError::NotWritable(k) => {
                let mut e = Self::new(
                    StatusCode::CONFLICT,
                    "not_writable",
                    format!(
                        "{} is read at startup – set it via the environment / paperrust.conf.",
                        pl_settings::schema::display_name(&k)
                    ),
                );
                e.extra = Some(Box::new(("keys", json!([k]))));
                e
            }
            StoreError::RevConflict { current } => {
                let mut e = Self::new(
                    StatusCode::CONFLICT,
                    "rev_conflict",
                    "Settings were changed meanwhile – reload.",
                );
                e.extra = Some(Box::new(("current_rev", json!(current))));
                e
            }
            StoreError::LockedByEnvironment(keys) => {
                let mut e = Self::new(
                    StatusCode::CONFLICT,
                    "locked_by_environment",
                    format!(
                        "Set via environment, cannot be changed here: {}",
                        keys.join(", ")
                    ),
                );
                e.extra = Some(Box::new(("keys", json!(keys))));
                e
            }
            StoreError::Invalid(errs) => {
                let mut m = Map::new();
                for e in errs {
                    if let Some(a) = m.entry(e.key).or_insert_with(|| json!([])).as_array_mut() {
                        a.push(json!(e.message));
                    }
                }
                Self::invalid(m)
            }
        }
    }
}

impl IntoResponse for SettingsError {
    fn into_response(self) -> Response {
        let mut err = Map::new();
        err.insert("code".into(), json!(self.code));
        err.insert("message".into(), json!(self.message));
        if let Some(f) = self.fields {
            err.insert("fields".into(), Value::Object(*f));
        }
        if let Some(x) = self.extra {
            let (k, v) = *x;
            err.insert(k.into(), v);
        }
        (self.status, axum::Json(json!({ "error": err }))).into_response()
    }
}

pub(crate) type R<T> = Result<T, SettingsError>;

// ---------------------------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------------------------

/// What a caller may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub superuser: bool,
    pub view_ac: bool,
    pub change_ac: bool,
    pub loopback: bool,
}

impl Access {
    pub fn of(p: &Principal) -> Self {
        let ext = p.ext.downcast_ref::<pl_store::AuthExt>();
        let user = ext.and_then(|e| e.ctx.user.clone());
        let has = |perm: &str| user.as_ref().is_some_and(|u| u.has_perm(perm));
        let active_su = p.is_superuser && user.as_ref().is_none_or(|u| u.is_active);
        Access {
            superuser: active_su,
            view_ac: active_su || has(PERM_VIEW_AC),
            change_ac: active_su || has(PERM_CHANGE_AC),
            loopback: ext
                .and_then(|e| e.peer)
                .is_some_and(|a: SocketAddr| a.ip().is_loopback()),
        }
    }

    fn is_desktop(def: &SettingDef) -> bool {
        matches!(def.store, DefStore::DesktopToml | DefStore::AppToml)
    }

    /// May see the key?
    pub fn can_read(&self, def: &SettingDef) -> bool {
        if Self::is_desktop(def) {
            return self.superuser && self.loopback;
        }
        if def.ac_field.is_some() {
            return self.view_ac;
        }
        self.superuser
    }

    /// May change the key (locks/classes are checked afterwards by the storage)?
    pub fn can_write(&self, def: &SettingDef) -> bool {
        if Self::is_desktop(def) {
            return self.superuser && self.loopback;
        }
        if def.ac_field.is_some() {
            return self.change_ac;
        }
        self.superuser
    }

    /// May see the settings page at all?
    pub fn any(&self) -> bool {
        self.superuser || self.view_ac
    }
}

pub(crate) fn handle(state: &AppState) -> SettingsHandle {
    state.service::<SettingsHandle>().unwrap_or_else(|| {
        // Without a mounted holder (tests): environment only; saving still works via the DB.
        let c = state.config();
        let h = SettingsHandle::without_db(
            c.env.clone(),
            pl_settings::crypto::SecretBox::from_secret_key(&c.secret_key),
        );
        state.set_service(h.clone());
        h
    })
}

pub(crate) async fn snapshot(state: &AppState, h: &SettingsHandle) -> R<Settings> {
    h.fresh(state.db()).await.map_err(SettingsError::internal)
}

pub(crate) fn actor(p: &Principal) -> Actor {
    let peer = p
        .ext
        .downcast_ref::<pl_store::AuthExt>()
        .and_then(|e| e.peer);
    Actor {
        user_id: i32::try_from(p.user_id).ok(),
        user_name: Some(p.username.clone()),
        remote_addr: peer.map(|a| a.ip()),
    }
}

// ---------------------------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------------------------

fn source_str(s: Source) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

fn effect_str(d: &SettingDef) -> Value {
    serde_json::to_value(d.effect).unwrap_or(Value::Null)
}

fn doc_link(s: &Settings, d: &SettingDef) -> String {
    match s.value("PAPERRUST_DOCS_URL").filter(|v| !v.is_empty()) {
        Some(base) => format!("{}/configuration#{}", base.trim_end_matches('/'), d.name()),
        None => d.doc_anchor(),
    }
}

/// A list element (§3). Secrets: no value, only `is_set`.
pub fn item(s: &Settings, e: &Entry) -> Value {
    let d = e.def;
    let is_set = e.value.as_deref().is_some_and(|v| !v.is_empty()) && e.source != Source::Default;
    let typed = |v: &Option<String>| -> Value {
        match v {
            _ if d.secret => Value::Null,
            Some(v) => pl_settings::value::to_json(d, v),
            None => Value::Null,
        }
    };
    json!({
        "key": d.key,
        // displayed name (PAPERRUST_X); `key` stays the identifier of the API
        "name": d.name(),
        "aliases": d.aliases(),
        "alias_of": d.alias_of,
        "category": d.category.as_str(),
        "class": d.class.as_str(),
        "effect": effect_str(d),
        "reindex": d.reindex,
        "secret": d.secret,
        "type": serde_json::to_value(d.ty).unwrap_or(Value::Null),
        "default": if d.secret { Value::Null } else { d.default.map(|v| pl_settings::value::to_json(d, v)).unwrap_or(Value::Null) },
        "value": typed(&e.value),
        "is_set": is_set,
        "unreadable": e.unreadable,
        "source": source_str(e.source),
        "source_detail": e.source_detail,
        "shadowed": e.shadowed.as_ref().map(|sh| json!({"source": source_str(sh.source), "value": typed(&sh.value)})),
        "locked": e.locked,
        "lock_reason": e.lock_reason,
        "restart_pending": e.restart_pending,
        "django_consumer": d.django_consumer(),
        "appconfig_field": d.ac_field,
        "in_db": e.in_db,
        "updated_at": e.updated_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "updated_by": e.updated_by,
        "help": d.help,
        "help_en": d.help_en(),
        "doc": doc_link(s, d),
    })
}

fn with_rev(status: StatusCode, body: Value, rev: i64) -> Response {
    let mut r = (status, axum::Json(body)).into_response();
    if let Ok(h) = HeaderValue::from_str(&format!("\"{rev}\"")) {
        r.headers_mut().insert(header::ETAG, h);
    }
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    r
}

fn list_body(
    state: &AppState,
    s: &Settings,
    access: &Access,
    filter: impl Fn(&Entry) -> bool,
) -> Value {
    let items: Vec<Value> = s
        .entries()
        .iter()
        .filter(|e| !e.def.internal && access.can_read(e.def) && filter(e))
        .map(|e| item(s, e))
        .collect();
    let supported = pl_settings::restart::supported(&state.config().env);
    json!({
        "rev": s.rev,
        "restart_supported": supported,
        "restart_pending_count": s.restart_pending_count(),
        // Parallel operation with Django no longer exists (2026-11-14); the field stays for the UI.
        "mixed_mode": false,
        "items": items,
    })
}

// ---------------------------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------------------------

/// `GET /settings?category=&q=&class=&changed=&restart_pending=`
pub async fn list(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
) -> R<Response> {
    let access = Access::of(&p);
    if !access.any() {
        return Err(SettingsError::forbidden());
    }
    let h = handle(&state);
    let s = snapshot(&state, &h).await?;
    let cat = q.get("category").filter(|v| !v.is_empty()).cloned();
    let class = q
        .get("class")
        .filter(|v| !v.is_empty())
        .map(|c| c.to_ascii_uppercase());
    let needle = q
        .get("q")
        .map(|v| v.trim().to_lowercase())
        .filter(|v| !v.is_empty());
    let only_changed = crate::http::truthy(q.get("changed").map(String::as_str));
    let only_pending = crate::http::truthy(q.get("restart_pending").map(String::as_str));
    let body = list_body(&state, &s, &access, |e| {
        cat.as_deref().is_none_or(|c| c == e.def.category.as_str())
            && class.as_deref().is_none_or(|c| c == e.def.class.as_str())
            && needle.as_deref().is_none_or(|n| {
                e.def.key.to_lowercase().contains(n)
                    || e.def.name().to_lowercase().contains(n)
                    || e.def.help.to_lowercase().contains(n)
                    || e.def.help_en().to_lowercase().contains(n)
            })
            && (!only_changed || matches!(e.source, Source::Db | Source::AppConfig))
            && (!only_pending || e.restart_pending)
    });
    Ok(with_rev(StatusCode::OK, body, s.rev))
}

fn schema_etag() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(SCHEMA)
        .unwrap_or_default()
        .hash(&mut h);
    pl_settings::help_en::HELP_EN.hash(&mut h);
    format!(
        "\"schema-{}-{:016x}\"",
        pl_core::brand::PAPERRUST_VERSION,
        h.finish()
    )
}

/// `GET /settings/schema` (cacheable, ETag).
pub async fn schema(Caller(p): Caller, headers: HeaderMap) -> R<Response> {
    let access = Access::of(&p);
    if !access.any() {
        return Err(SettingsError::forbidden());
    }
    let etag = schema_etag();
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        let mut r = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(v) = HeaderValue::from_str(&etag) {
            r.headers_mut().insert(header::ETAG, v);
        }
        return Ok(r);
    }
    let items: Vec<Value> = SCHEMA
        .iter()
        .filter(|d| !d.internal && access.can_read(d))
        .map(|d| {
            let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("category".into(), json!(d.category.as_str()));
                o.insert("class".into(), json!(d.class.as_str()));
                o.insert("django_consumer".into(), json!(d.django_consumer()));
                o.insert("name".into(), json!(d.name()));
                o.insert("doc".into(), json!(d.doc_anchor()));
                o.insert("help_en".into(), json!(d.help_en()));
                if d.secret {
                    o.insert("default".into(), Value::Null);
                }
            }
            v
        })
        .collect();
    let categories: Vec<&str> = pl_settings::schema::Category::ALL
        .iter()
        .map(|c| c.as_str())
        .collect();
    let mut r = (
        StatusCode::OK,
        axum::Json(json!({"categories": categories, "items": items})),
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0, must-revalidate"),
    );
    Ok(r)
}

/// `GET /settings/{key}`
pub async fn get_one(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(key): Path<String>,
) -> R<Response> {
    let access = Access::of(&p);
    let def = pl_settings::schema::find(&key)
        .filter(|d| !d.internal)
        .ok_or_else(SettingsError::not_found)?;
    if !access.can_read(def) {
        return Err(if access.any() {
            SettingsError::not_found()
        } else {
            SettingsError::forbidden()
        });
    }
    let h = handle(&state);
    let s = snapshot(&state, &h).await?;
    let e = s.get(def.key).ok_or_else(SettingsError::not_found)?;
    Ok(with_rev(StatusCode::OK, item(&s, e), s.rev))
}

#[derive(Debug, Deserialize)]
pub struct PatchBody {
    pub values: Map<String, Value>,
    #[serde(default)]
    pub expected_rev: Option<i64>,
}

/// Protection against lock-out (§1.4 footnote ²): `DISABLE_REGULAR_LOGIN`/`REDIRECT_LOGIN_TO_SSO` only
/// if an SSO provider is configured and the saving user has a linked SSO account.
async fn lockout_check(
    state: &AppState,
    s: &Settings,
    p: &Principal,
    values: &Map<String, Value>,
) -> R<()> {
    let enabling = [
        "PAPERLESS_DISABLE_REGULAR_LOGIN",
        "PAPERLESS_REDIRECT_LOGIN_TO_SSO",
    ]
    .into_iter()
    .filter(|k| {
        values.iter().any(|(vk, v)| {
            pl_settings::schema::find(vk).is_some_and(|d| d.key == *k)
                && match v {
                    Value::Bool(b) => *b,
                    Value::String(s) => pl_core::config::str_to_bool(s).unwrap_or(false),
                    _ => false,
                }
        })
    })
    .collect::<Vec<_>>();
    if enabling.is_empty() {
        return Ok(());
    }
    let providers: Option<Value> = values
        .iter()
        .find(|(k, _)| {
            pl_settings::schema::find(k)
                .is_some_and(|d| d.key == "PAPERLESS_SOCIALACCOUNT_PROVIDERS")
        })
        .map(|(_, v)| match v {
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            other => other.clone(),
        })
        .or_else(|| {
            s.value("PAPERLESS_SOCIALACCOUNT_PROVIDERS")
                .and_then(|v| serde_json::from_str(v).ok())
        });
    let has_provider = providers
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty());
    let linked: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.socialaccount_socialaccount') IS NOT NULL
           AND EXISTS (SELECT 1 FROM socialaccount_socialaccount WHERE user_id = $1)",
    )
    .bind(i32::try_from(p.user_id).unwrap_or(-1))
    .fetch_one(state.db())
    .await
    .unwrap_or(false);
    if has_provider && linked {
        return Ok(());
    }
    let msg = if has_provider {
        "You would lock yourself out: link an SSO account to your user first."
    } else {
        "You would lock yourself out: configure an SSO provider (SOCIALACCOUNT_PROVIDERS) first."
    };
    let mut m = Map::new();
    for k in enabling {
        m.insert(k.into(), json!([msg]));
    }
    Err(SettingsError::invalid(m))
}

/// `PATCH /settings` `{"values":{KEY:value,…},"expected_rev":N}` – atomic.
pub async fn patch(
    State(state): State<AppState>,
    Caller(p): Caller,
    headers: HeaderMap,
    V2Json(body): V2Json<PatchBody>,
) -> R<Response> {
    let access = Access::of(&p);
    if !access.any() {
        return Err(SettingsError::forbidden());
    }
    if body.values.is_empty() {
        return Err(SettingsError::field("values", "No values given."));
    }
    let desktop_keys = body.values.keys().filter(|k| k.starts_with("desktop.") || k.starts_with("app.")).count();
    if desktop_keys > 0 {
        if desktop_keys != body.values.len() {
            return Err(SettingsError::field("values", "Desktop settings are saved separately."));
        }
        return patch_desktop(&state, &access, &body.values).await;
    }
    let mut changes = Vec::with_capacity(body.values.len());
    for (k, v) in &body.values {
        let Some(def) = pl_settings::schema::find(k).filter(|d| !d.internal) else {
            return Err(SettingsError::field(k, "Unknown setting."));
        };
        if !access.can_write(def) {
            return Err(SettingsError::forbidden());
        }
        if def.key == "PAPERLESS_APP_LOGO" && !v.is_null() {
            return Err(SettingsError::field(
                k,
                "Upload the logo with PUT /rs/api/v2/settings/app_logo (multipart).",
            ));
        }
        let input = if v.is_null() {
            Input::Reset
        } else {
            Input::Json(v.clone())
        };
        changes.push((def.key.to_string(), input));
    }
    let expected = match body.expected_rev {
        Some(r) => Some(r),
        None => crate::http::if_match(&headers)
            .map_err(|_| SettingsError::field("If-Match", "Expected the settings rev."))?,
    };
    let h = handle(&state);
    let s = snapshot(&state, &h).await?;
    lockout_check(&state, &s, &p, &body.values).await?;
    let opts = ApplyOptions {
        expected_rev: expected,
        env: Some(h.env()),
        if_unset: false,
    };
    let rev = store::apply(
        state.db(),
        h.secret_box(),
        &changes,
        &opts,
        &actor(&p),
        Action::Set,
    )
    .await?;
    finish_write(&state, &h, &access, &changes, rev).await
}

/// `desktop.*` via the supervisor (docs/SETTINGS.md §2.6, WP7): superuser + loopback, check
/// here against the schema, then `settings.request.json` → supervisor writes `desktop.toml`.
async fn patch_desktop(state: &AppState, access: &Access, values: &Map<String, Value>) -> R<Response> {
    // Permissions first (superuser + loopback), only then the question whether a supervisor is there.
    for k in values.keys() {
        if pl_settings::schema::find(k).is_some_and(|d| !access.can_write(d)) {
            return Err(SettingsError::forbidden());
        }
    }
    let h = handle(state);
    let Some(dir) = pl_settings::desktop::control_dir(h.env()) else {
        return Err(SettingsError::from(StoreError::NotWritable("desktop.*".into())));
    };
    let mut changes = std::collections::BTreeMap::new();
    let mut invalid = Map::new();
    for (k, v) in values {
        let Some(def) = pl_settings::schema::find(k).filter(|d| !d.internal) else {
            return Err(SettingsError::field(k, "Unknown setting."));
        };
        if !access.can_write(def) {
            return Err(SettingsError::forbidden());
        }
        if !pl_settings::desktop::WRITABLE.contains(&def.key) {
            return Err(SettingsError::from(StoreError::NotWritable(def.key.to_string())));
        }
        match pl_settings::value::from_json(def, v) {
            Ok(raw) => {
                changes.insert(def.key.to_string(), raw);
            }
            Err(e) => {
                invalid.insert(def.key.to_string(), json!([e.message]));
            }
        }
    }
    if !invalid.is_empty() {
        return Err(SettingsError::invalid(invalid));
    }
    let outcome = pl_settings::desktop::request(&dir, changes, std::time::Duration::from_secs(15))
        .await
        .map_err(|e| SettingsError::new(StatusCode::SERVICE_UNAVAILABLE, "desktop_unavailable", e))?;
    if !outcome.ok {
        let mut m = Map::new();
        m.insert("values".into(), json!([outcome.error.unwrap_or_else(|| "rejected by the desktop app".into())]));
        return Err(SettingsError::invalid(m));
    }
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(json!({"applied": true, "restarting": outcome.restart})),
    )
        .into_response())
}

async fn finish_write(
    state: &AppState,
    h: &SettingsHandle,
    access: &Access,
    changes: &[(String, Input)],
    rev: i64,
) -> R<Response> {
    // own process immediately (the others via NOTIFY)
    if let Err(e) = h.reload(state.db()).await {
        tracing::warn!(error = %e, "settings reload after write failed");
    }
    let s = snapshot(state, h).await?;
    let keys: Vec<&str> = changes
        .iter()
        .filter_map(|(k, _)| pl_settings::schema::find(k).map(|d| d.key))
        .collect();
    let mut body = list_body(state, &s, access, |e| keys.contains(&e.def.key));
    let reindex = s
        .entries()
        .iter()
        .any(|e| keys.contains(&e.def.key) && e.def.reindex);
    body["reindex_suggested"] = json!(reindex);
    Ok(with_rev(StatusCode::OK, body, rev.max(s.rev)))
}

/// `DELETE /settings/{key}` – back to default (delete DB row or set AC column to NULL).
pub async fn reset(
    State(state): State<AppState>,
    Caller(p): Caller,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> R<Response> {
    let access = Access::of(&p);
    let def = pl_settings::schema::find(&key)
        .filter(|d| !d.internal)
        .ok_or_else(SettingsError::not_found)?;
    if !access.can_write(def) {
        return Err(if access.can_read(def) {
            SettingsError::forbidden()
        } else {
            SettingsError::not_found()
        });
    }
    let expected = crate::http::if_match(&headers)
        .map_err(|_| SettingsError::field("If-Match", "Expected the settings rev."))?;
    let h = handle(&state);
    if def.key == "PAPERLESS_APP_LOGO" {
        remove_logo_file(&state).await;
    }
    let changes = vec![(def.key.to_string(), Input::Reset)];
    let opts = ApplyOptions {
        expected_rev: expected,
        env: Some(h.env()),
        if_unset: false,
    };
    let rev = store::apply(
        state.db(),
        h.secret_box(),
        &changes,
        &opts,
        &actor(&p),
        Action::Reset,
    )
    .await?;
    finish_write(&state, &h, &access, &changes, rev).await
}

/// `GET /settings/history?key=&cursor=&limit=`
pub async fn history(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
) -> R<Response> {
    let access = Access::of(&p);
    if !access.any() {
        return Err(SettingsError::forbidden());
    }
    let key = match q.get("key").filter(|k| !k.is_empty()) {
        None => None,
        Some(k) if k.starts_with("AC:") => Some(k.clone()),
        Some(k) => {
            let d = pl_settings::schema::find(k)
                .ok_or_else(|| SettingsError::field("key", "Unknown setting."))?;
            Some(match d.ac_field {
                Some(f) => format!("AC:{f}"),
                None => d.key.to_string(),
            })
        }
    };
    let cursor = q.get("cursor").and_then(|c| c.parse::<i64>().ok());
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let mut conn = state
        .db()
        .acquire()
        .await
        .map_err(SettingsError::internal)?;
    // Non-superuser: only AC entries (same view as the list)
    let mut out = Vec::new();
    let mut before = cursor;
    let mut next = None;
    loop {
        let rows = store::history_list(&mut conn, key.as_deref(), before, limit)
            .await
            .map_err(SettingsError::internal)?;
        let n = rows.len();
        for r in rows {
            before = Some(r.id);
            let visible = if let Some(f) = r.key.strip_prefix("AC:") {
                pl_settings::schema::find_ac(f).is_some_and(|d| access.can_read(d))
            } else {
                pl_settings::schema::find(&r.key).is_some_and(|d| access.can_read(d))
            };
            if visible {
                out.push(serde_json::to_value(&r).unwrap_or(Value::Null));
                if out.len() as i64 == limit {
                    next = Some(r.id);
                    break;
                }
            }
        }
        if next.is_some() || (n as i64) < limit {
            break;
        }
    }
    Ok(crate::http::json(
        &json!({"items": out, "next_cursor": next.map(|n| n.to_string())}),
    ))
}

#[derive(Debug, Deserialize)]
pub struct RestartBody {
    #[serde(default)]
    pub password: Option<String>,
}

/// `POST /settings/restart` `{"password":"…"}` – orderly shutdown with exit code 75.
pub async fn restart(
    State(state): State<AppState>,
    Caller(p): Caller,
    V2Json(body): V2Json<RestartBody>,
) -> R<Response> {
    let access = Access::of(&p);
    if !access.superuser {
        return Err(SettingsError::forbidden());
    }
    confirm_password(&p, body.password.as_deref())?;
    let h = handle(&state);
    if !pl_settings::restart::supported(h.env()) {
        return Err(SettingsError::new(
            StatusCode::CONFLICT,
            "restart_not_supported",
            "No supervisor restarts this process – please restart the service manually.",
        ));
    }
    tracing::warn!(user = %p.username, "restart requested via /rs/api/v2/settings/restart");
    // Deliver the response first, then shut down.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        pl_settings::restart::request("api");
    });
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(json!({"restarting": true, "exit_code": pl_settings::restart::EXIT_CODE})),
    )
        .into_response())
}

pub(crate) fn confirm_password(p: &Principal, password: Option<&str>) -> R<()> {
    let Some(pw) = password.filter(|p| !p.is_empty()) else {
        return Err(SettingsError::field(
            "password",
            "Please confirm with your password.",
        ));
    };
    let user = p
        .ext
        .downcast_ref::<pl_store::AuthExt>()
        .and_then(|e| e.ctx.user.clone())
        .ok_or_else(SettingsError::forbidden)?;
    if !pl_authz::hashers::check_password(pw, &user.password) {
        return Err(SettingsError::field("password", "Wrong password."));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Logo (multipart, §2.8) – same storage path as v1 (`upload_to="logo/"`, MEDIA_ROOT)
// ---------------------------------------------------------------------------------------------

async fn remove_logo_file(state: &AppState) {
    let cur: Option<Option<String>> = sqlx::query_scalar(
        "SELECT app_logo FROM paperless_applicationconfiguration ORDER BY id LIMIT 1",
    )
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten();
    if let Some(Some(name)) = cur.filter(|n| n.as_ref().is_some_and(|n| !n.is_empty()))
        && let Some(p) = crate::settings_logo::media_path(&state.config().media_root, &name)
    {
        let _ = tokio::fs::remove_file(p).await;
    }
}

/// `PUT /settings/app_logo` (multipart, field `app_logo`).
pub async fn put_logo(
    State(state): State<AppState>,
    Caller(p): Caller,
    mut mp: Multipart,
) -> R<Response> {
    let access = Access::of(&p);
    let def =
        pl_settings::schema::find("PAPERLESS_APP_LOGO").ok_or_else(SettingsError::not_found)?;
    if !access.can_write(def) {
        return Err(SettingsError::forbidden());
    }
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| SettingsError::field("app_logo", format!("Invalid upload: {e}")))?
    {
        if field.name() == Some("app_logo") {
            let name = field.file_name().unwrap_or("logo").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| SettingsError::field("app_logo", format!("Invalid upload: {e}")))?;
            file = Some((name, data.to_vec()));
        }
    }
    let Some((name, data)) = file else {
        return Err(SettingsError::field("app_logo", "No file was submitted."));
    };
    let h = handle(&state);
    let max_pixels = h
        .load()
        .env()
        .int("PAPERLESS_MAX_IMAGE_PIXELS")
        .ok()
        .flatten();
    let (clean_name, bytes) = crate::settings_logo::validate(&name, &data, max_pixels)
        .map_err(|m| SettingsError::field("app_logo", m))?;
    let media = state.config().media_root.clone();
    let stored = crate::settings_logo::store(&media, &clean_name, &bytes)
        .await
        .map_err(SettingsError::internal)?;
    remove_logo_file(&state).await;
    let changes = vec![(
        def.key.to_string(),
        Input::Json(Value::String(stored.clone())),
    )];
    let opts = ApplyOptions {
        expected_rev: None,
        env: None,
        if_unset: false,
    };
    match store::apply(
        state.db(),
        h.secret_box(),
        &changes,
        &opts,
        &actor(&p),
        Action::Set,
    )
    .await
    {
        Ok(rev) => finish_write(&state, &h, &access, &changes, rev).await,
        Err(e) => {
            if let Some(p) = crate::settings_logo::media_path(&media, &stored) {
                let _ = tokio::fs::remove_file(p).await;
            }
            Err(e.into())
        }
    }
}

/// Router part (relative to `/rs/api/v2`).
pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    use axum::routing::{get, post, put};
    r.route("/settings", get(list).patch(patch))
        .route("/settings/schema", get(schema))
        .route("/settings/history", get(history))
        .route("/settings/restart", post(restart))
        .route("/settings/app_logo", put(put_logo))
        .route("/settings/origin", get(crate::settings_migration::origin))
        .route("/settings/adopt", post(crate::settings_migration::adopt))
        .route("/settings/slim-env", get(crate::settings_migration::slim_env))
        .route("/settings/check-file", post(crate::settings_migration::check_file))
        .route("/settings/export", post(crate::settings_migration::export))
        .route("/settings/import", post(crate::settings_migration::import))
        .route("/settings/{key}", get(get_one).delete(reset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(su: bool, view: bool, change: bool, lo: bool) -> Access {
        Access {
            superuser: su,
            view_ac: view || su,
            change_ac: change || su,
            loopback: lo,
        }
    }

    #[test]
    fn permission_matrix() {
        let host = pl_settings::schema::find("PAPERLESS_EMAIL_HOST").unwrap();
        let lang = pl_settings::schema::find("PAPERLESS_OCR_LANGUAGE").unwrap();
        let desk = pl_settings::schema::find("desktop.server.lan_sharing").unwrap();
        let su = access(true, false, false, false);
        assert!(su.can_read(host) && su.can_write(host) && su.can_write(lang));
        assert!(!su.can_read(desk) && !su.can_write(desk));
        let su_lo = access(true, false, false, true);
        assert!(su_lo.can_read(desk) && su_lo.can_write(desk));
        let viewer = access(false, true, false, true);
        assert!(
            viewer.can_read(lang)
                && !viewer.can_write(lang)
                && !viewer.can_read(host)
                && !viewer.can_read(desk)
        );
        let changer = access(false, true, true, false);
        assert!(changer.can_write(lang) && !changer.can_write(host));
        let nobody = access(false, false, false, true);
        assert!(!nobody.any() && !nobody.can_read(lang));
    }

    #[test]
    fn store_errors_map_to_codes() {
        let e: SettingsError = StoreError::RevConflict { current: 4 }.into();
        assert_eq!((e.status, e.code), (StatusCode::CONFLICT, "rev_conflict"));
        let e: SettingsError =
            StoreError::LockedByEnvironment(vec!["PAPERLESS_EMAIL_HOST".into()]).into();
        assert_eq!(
            (e.status, e.code),
            (StatusCode::CONFLICT, "locked_by_environment")
        );
        let e: SettingsError = StoreError::Invalid(vec![pl_settings::value::ValueError {
            key: "K".into(),
            message: "bad".into(),
        }])
        .into();
        assert_eq!(
            (e.status, e.code),
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid")
        );
        assert_eq!(e.fields.unwrap()["K"][0], "bad");
    }

    #[test]
    fn secrets_never_in_items() {
        let env = pl_core::config::Env::from_pairs([
            ("PAPERLESS_EMAIL_HOST_PASSWORD", "hunter2"),
            ("PAPERLESS_AI_LLM_API_KEY", "sk-x"),
        ]);
        let s = Settings::build(&pl_settings::layer::Layers::new(env), 0, None);
        for k in ["PAPERLESS_EMAIL_HOST_PASSWORD", "PAPERLESS_AI_LLM_API_KEY"] {
            let v = item(&s, s.get(k).unwrap());
            assert!(
                !v.to_string().contains("hunter2") && !v.to_string().contains("sk-x"),
                "{v}"
            );
            assert_eq!(v["is_set"], true);
            assert_eq!(v["value"], Value::Null);
        }
        let v = item(&s, s.get("PAPERLESS_EMAIL_PORT").unwrap());
        assert_eq!(
            (v["value"].clone(), v["is_set"].clone(), v["source"].clone()),
            (json!(25), json!(false), json!("default"))
        );
    }
}
