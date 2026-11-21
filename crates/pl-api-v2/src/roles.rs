//! `/rs/api/v2/roles` – roles (docs/DESIGN2-ENTWIRREN.md §1, contract docs/contract/pl-api-v2.md
//! "Rollen"). New in PaperRust, no upstream counterpart.
//!
//! Roles are a **layer above the Django permissions**: a role is a named set of
//! model permissions (`<app_label>.<codename>`); "assign role" writes exactly this set (∪
//! [`ALWAYS`]) to `auth_user_user_permissions` or `auth_group_permissions`. The source of truth remains
//! the Django permissions; the role of a user/group is recognised by exact set comparison
//! (otherwise "individual"). A switch back to paperless-ngx only loses the role names.
//!
//! Derivation of the permission lists from upstream (v3.4.2):
//! * Model permissions are checked by `documents/permissions.py::PaperlessObjectPermissions` (view/add/change/
//!   delete per model); notes `PaperlessNotePermissions` (POST additionally needs
//!   `documents.change_document` → included in "Edit");
//! * History: `documents/views.py::DocumentViewSet.history` requires `auditlog.view_logentry`;
//! * UI settings: `UiSettingsView` with `PaperlessObjectPermissions` (GET view, POST add)
//!   → [`ALWAYS`];
//! * Logs/system views: `PaperlessAdminPermissions` = `is_staff` (no model permission);
//! * Model list like upstream `src-ui/src/app/services/permissions.service.ts::PermissionType`
//!   (there workflow triggers/actions are not separate types; hence not here either).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use pl_core::AppState;
use pl_domain::{DomainError, Principal};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::d2;
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json, json_with_rev};

/// Migration (name, SQL) for `pl_cli::migrate::paperrust_migrations()`.
pub const MIGRATION: (&str, &str) = ("pl_api_v2.0001_paperrust_role", include_str!("../migrations/0001_paperrust_role.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

/// Fixed role "Read only".
pub const READ: &[&str] = &[
    "documents.view_correspondent",
    "documents.view_customfield",
    "documents.view_document",
    "documents.view_documenttype",
    "documents.view_note",
    "documents.view_savedview",
    "documents.view_sharelink",
    "documents.view_sharelinkbundle",
    "documents.view_storagepath",
    "documents.view_tag",
];

/// In addition to [`READ`] for "Edit".
pub const EDIT_EXTRA: &[&str] = &[
    "documents.add_document",
    "documents.change_document",
    "documents.add_note",
    "documents.change_note",
    "documents.add_savedview",
    "documents.change_savedview",
    "documents.add_sharelink",
    "documents.add_sharelinkbundle",
    "documents.change_sharelinkbundle",
];

/// In addition to "Edit" for "Manage".
pub const MANAGE_EXTRA: &[&str] = &[
    "documents.delete_document",
    "documents.delete_note",
    "documents.delete_sharelink",
    "documents.delete_sharelinkbundle",
    "documents.delete_savedview",
    "documents.add_tag",
    "documents.change_tag",
    "documents.delete_tag",
    "documents.add_correspondent",
    "documents.change_correspondent",
    "documents.delete_correspondent",
    "documents.add_documenttype",
    "documents.change_documenttype",
    "documents.delete_documenttype",
    "documents.add_storagepath",
    "documents.change_storagepath",
    "documents.delete_storagepath",
    "documents.add_customfield",
    "documents.change_customfield",
    "documents.delete_customfield",
    "documents.view_workflow",
    "documents.add_workflow",
    "documents.change_workflow",
    "documents.delete_workflow",
    "paperless_mail.view_mailaccount",
    "paperless_mail.add_mailaccount",
    "paperless_mail.change_mailaccount",
    "paperless_mail.delete_mailaccount",
    "paperless_mail.view_mailrule",
    "paperless_mail.add_mailrule",
    "paperless_mail.change_mailrule",
    "paperless_mail.delete_mailrule",
    "paperless_mail.view_processedmail",
    "paperless_mail.add_processedmail",
    "paperless_mail.change_processedmail",
    "paperless_mail.delete_processedmail",
    "documents.view_paperlesstask",
    "auditlog.view_logentry",
];

/// Always included (the UI does not work without them; `UiSettingsView`).
pub const ALWAYS: &[&str] = &["documents.view_uisettings", "documents.add_uisettings", "documents.change_uisettings"];

/// Topics of the fine-grained settings (key, models `<app_label>.<model>`).
pub const TOPICS: &[(&str, &[&str])] = &[
    ("documents", &["documents.document", "documents.note", "documents.sharelink", "documents.sharelinkbundle", "auditlog.logentry"]),
    ("organize", &["documents.tag", "documents.correspondent", "documents.documenttype", "documents.storagepath", "documents.customfield"]),
    ("views", &["documents.savedview"]),
    (
        "automation",
        &[
            "documents.workflow",
            "paperless_mail.mailaccount",
            "paperless_mail.mailrule",
            "paperless_mail.processedmail",
            "documents.paperlesstask",
        ],
    ),
    ("system", &["auth.user", "auth.group", "paperless.applicationconfiguration"]),
];

const ACTIONS: [&str; 4] = ["view", "add", "change", "delete"];

pub const MODE_CLASSIC: &str = "classic";
pub const MODE_CUSTOM: &str = "custom";

const MAX_IDS: usize = 1000;
const MAX_NAME: usize = 128;
const MAX_DESCRIPTION: usize = 2000;

const PERM_VIEW_USER: &str = "auth.view_user";
const PERM_VIEW_GROUP: &str = "auth.view_group";
const PERM_CHANGE_USER: &str = "auth.change_user";
const PERM_CHANGE_GROUP: &str = "auth.change_group";

const DENIED: &str = "You do not have permission to perform this action.";

// ---------------------------------------------------------------------------------------------
// Pure logic (without DB)
// ---------------------------------------------------------------------------------------------

/// Permission set of a fixed role (`None` = unknown; `admin` = empty, admin is the superuser flag).
pub fn builtin_set(key: &str) -> Option<BTreeSet<String>> {
    let parts: &[&[&str]] = match key {
        "read" => &[READ],
        "edit" => &[READ, EDIT_EXTRA],
        "manage" => &[READ, EDIT_EXTRA, MANAGE_EXTRA],
        "admin" => &[],
        _ => return None,
    };
    Some(parts.iter().flat_map(|p| p.iter()).map(|s| s.to_string()).collect())
}

/// All codenames a custom role may contain: view/add/change/delete of the topic models
/// (plus [`ALWAYS`], which are removed on save).
pub fn allowed_codenames() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, models) in TOPICS {
        for m in *models {
            if let Some((app, model)) = m.split_once('.') {
                for a in ACTIONS {
                    out.insert(format!("{app}.{a}_{model}"));
                }
            }
        }
    }
    out.extend(ALWAYS.iter().map(|s| s.to_string()));
    out
}

fn always_set() -> BTreeSet<String> {
    ALWAYS.iter().map(|s| s.to_string()).collect()
}

/// A role (response shape).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Role {
    pub id: i64,
    pub key: Option<String>,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub admin: bool,
    pub permissions: BTreeSet<String>,
}

impl Role {
    /// Permission set that is compared or written: without [`ALWAYS`], intersected with
    /// the permissions present in the database (`known`, older databases without e.g.
    /// `sharelinkbundle`).
    pub fn effective(&self, known: Option<&HashSet<String>>) -> BTreeSet<String> {
        self.permissions
            .iter()
            .filter(|p| !ALWAYS.contains(&p.as_str()))
            .filter(|p| known.is_none_or(|k| k.contains(p.as_str())))
            .cloned()
            .collect()
    }
}

/// Detected role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Role(i64),
    Individual,
    /// Superuser, but no administrator role in the table (should not happen).
    Admin,
}

impl Resolved {
    pub fn to_json(&self) -> Value {
        match self {
            Resolved::Role(id) => json!(id),
            Resolved::Individual => json!("individual"),
            Resolved::Admin => json!("admin"),
        }
    }
}

/// Role for a superuser flag and direct permissions: superuser → administrator role; otherwise the first
/// non-admin role (order of `roles`) whose set without [`ALWAYS`] is exactly equal.
pub fn resolve(is_superuser: bool, perms: &BTreeSet<String>, roles: &[Role], known: Option<&HashSet<String>>) -> Resolved {
    if is_superuser {
        return roles.iter().find(|r| r.admin).map(|r| Resolved::Role(r.id)).unwrap_or(Resolved::Admin);
    }
    let mine: BTreeSet<String> = perms.iter().filter(|p| !ALWAYS.contains(&p.as_str())).cloned().collect();
    roles.iter().filter(|r| !r.admin).find(|r| r.effective(known) == mine).map(|r| Resolved::Role(r.id)).unwrap_or(Resolved::Individual)
}

/// Who assigns (from `pl_authz::User`).
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: i64,
    pub is_active: bool,
    pub is_superuser: bool,
    pub permissions: HashSet<String>,
}

impl Actor {
    pub fn from_user(u: &pl_authz::User) -> Self {
        Actor { id: u.id, is_active: u.is_active, is_superuser: u.is_superuser, permissions: u.permissions.clone() }
    }

    pub fn has_perm(&self, p: &str) -> bool {
        self.is_active && (self.is_superuser || self.permissions.contains(p))
    }
}

/// Target user (before the assignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub id: i64,
    pub is_superuser: bool,
    pub is_staff: bool,
}

/// New state of a user after the assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserChange {
    pub id: i64,
    pub is_superuser: bool,
    pub is_staff: bool,
    /// Direct permissions (replaces `auth_user_user_permissions` completely).
    pub permissions: BTreeSet<String>,
}

fn forbidden(m: &str) -> V2Error {
    V2Error(DomainError::Forbidden(m.into()))
}

/// May `actor` assign this role to these kinds of targets? (before any DB access to targets)
pub fn check_assign(actor: &Actor, role: &Role, mode: &str, has_users: bool, has_groups: bool) -> V2Result<()> {
    if !actor.is_active {
        return Err(forbidden(DENIED));
    }
    if !role.builtin && mode != MODE_CUSTOM {
        return Err(V2Error(DomainError::invalid("Custom roles require the custom role mode.")));
    }
    if !actor.is_superuser {
        if (has_users && !actor.has_perm(PERM_CHANGE_USER)) || (has_groups && !actor.has_perm(PERM_CHANGE_GROUP)) {
            return Err(forbidden(DENIED));
        }
        if role.admin {
            return Err(forbidden("Superuser status can only be granted by a superuser"));
        }
        // No privilege escalation: only roles whose permissions the assigner has themselves.
        if role.permissions.iter().any(|p| !ALWAYS.contains(&p.as_str()) && !actor.permissions.contains(p)) {
            return Err(forbidden("You cannot assign a role with permissions you do not have."));
        }
    }
    if role.admin && has_groups {
        return Err(V2Error(DomainError::field("groups", "The administrator role cannot be assigned to a group.")));
    }
    Ok(())
}

/// Permissions that an assignment writes: role set ∪ [`ALWAYS`] (only existing permissions).
pub fn assigned_set(role: &Role, known: Option<&HashSet<String>>) -> BTreeSet<String> {
    let mut s = if role.admin { BTreeSet::new() } else { role.effective(known) };
    s.extend(always_set().into_iter().filter(|p| known.is_none_or(|k| k.contains(p.as_str()))));
    s
}

/// New state per target user. Administrator role: `is_superuser`/`is_staff` = true, direct
/// permissions = only [`ALWAYS`]. Other role for a superuser: both flags false (never for oneself,
/// only by a superuser). Other role for a non-superuser: `is_staff` stays.
pub fn plan_users(actor: &Actor, role: &Role, targets: &[Target], known: Option<&HashSet<String>>) -> V2Result<Vec<UserChange>> {
    let perms = assigned_set(role, known);
    let mut out = Vec::with_capacity(targets.len());
    for t in targets {
        let (su, staff) = if role.admin {
            (true, true)
        } else if t.is_superuser {
            if !actor.is_superuser {
                return Err(forbidden("Superusers can only be modified by other superusers"));
            }
            if t.id == actor.id {
                return Err(V2Error(DomainError::field("users", "You cannot remove your own administrator role")));
            }
            (false, false)
        } else {
            (false, t.is_staff)
        };
        out.push(UserChange { id: t.id, is_superuser: su, is_staff: staff, permissions: perms.clone() });
    }
    Ok(out)
}

/// Check the codenames of a custom role (allowed + present in the DB); [`ALWAYS`] is removed.
pub fn validate_permissions(list: &[String], known: Option<&HashSet<String>>) -> V2Result<BTreeSet<String>> {
    let allowed = allowed_codenames();
    let mut bad = Vec::new();
    let mut out = BTreeSet::new();
    for p in list {
        let p = p.trim();
        if !allowed.contains(p) || known.is_some_and(|k| !k.contains(p)) {
            bad.push(Value::String(format!("Unknown permission \"{p}\".")));
            continue;
        }
        if !ALWAYS.contains(&p) {
            out.insert(p.to_string());
        }
    }
    if !bad.is_empty() {
        let mut fields = Map::new();
        fields.insert("permissions".into(), Value::Array(bad));
        return Err(V2Error(DomainError::Invalid { message: "Unknown permissions.".into(), fields }));
    }
    Ok(out)
}

fn clean_name(s: &str) -> V2Result<String> {
    let n = s.trim();
    if n.is_empty() {
        return Err(V2Error(DomainError::field("name", "This field may not be blank.")));
    }
    if n.chars().count() > MAX_NAME {
        return Err(V2Error(DomainError::field("name", format!("Ensure this field has no more than {MAX_NAME} characters."))));
    }
    Ok(n.to_string())
}

fn clean_description(s: &str) -> V2Result<String> {
    if s.chars().count() > MAX_DESCRIPTION {
        return Err(V2Error(DomainError::field(
            "description",
            format!("Ensure this field has no more than {MAX_DESCRIPTION} characters."),
        )));
    }
    Ok(s.trim().to_string())
}

/// `1,2,3` → Ids (leer → leer).
pub fn parse_ids(field: &str, v: Option<&String>) -> V2Result<Vec<i64>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for part in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match part.parse::<i64>() {
            Ok(n) if n > 0 => out.push(n),
            _ => return Err(V2Error(DomainError::field(field, "Expected a comma-separated list of ids."))),
        }
    }
    if out.len() > MAX_IDS {
        return Err(V2Error(DomainError::field(field, format!("At most {MAX_IDS} ids."))));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn check_mode(m: &str) -> V2Result<&'static str> {
    match m {
        MODE_CLASSIC => Ok(MODE_CLASSIC),
        MODE_CUSTOM => Ok(MODE_CUSTOM),
        _ => Err(V2Error(DomainError::field("mode", "Expected \"classic\" or \"custom\"."))),
    }
}

/// Roles that apply in the mode (classic: only the fixed ones).
pub fn active_roles(all: Vec<Role>, mode: &str) -> Vec<Role> {
    if mode == MODE_CUSTOM { all } else { all.into_iter().filter(|r| r.builtin).collect() }
}

// ---------------------------------------------------------------------------------------------
// Datenbank
// ---------------------------------------------------------------------------------------------

static READY: AtomicBool = AtomicBool::new(false);

/// Create tables and reconcile fixed roles (once per process; the same SQL as the migration).
pub async fn ensure(pool: &PgPool) -> V2Result<()> {
    if !READY.load(Ordering::Relaxed) {
        sqlx::raw_sql(MIGRATION.1).execute(pool).await.map_err(db)?;
        READY.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn db(e: sqlx::Error) -> V2Error {
    V2Error(DomainError::internal(e))
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).is_some_and(|c| c == "23505")
}

type RoleRow = (i64, Option<String>, String, String, bool, bool, Vec<String>);

fn role_from(r: RoleRow) -> Role {
    let (id, key, name, description, builtin, admin, perms) = r;
    // Fixed roles: the permissions come from the code (source of truth), not from the row.
    let permissions = match (builtin, key.as_deref().and_then(builtin_set)) {
        (true, Some(s)) => s,
        _ => perms.into_iter().collect(),
    };
    Role { id, key, name, description, builtin, admin, permissions }
}

const ROLE_COLS: &str = "id, key::text, name::text, description, builtin, is_admin, permissions";

async fn load_roles<'e, E: sqlx::PgExecutor<'e>>(ex: E) -> V2Result<Vec<Role>> {
    let rows: Vec<RoleRow> =
        sqlx::query_as(&format!("SELECT {ROLE_COLS} FROM paperrust_role ORDER BY builtin DESC, id")).fetch_all(ex).await.map_err(db)?;
    Ok(rows.into_iter().map(role_from).collect())
}

async fn load_role<'e, E: sqlx::PgExecutor<'e>>(ex: E, id: i64) -> V2Result<Role> {
    let row: Option<RoleRow> =
        sqlx::query_as(&format!("SELECT {ROLE_COLS} FROM paperrust_role WHERE id = $1")).bind(id).fetch_optional(ex).await.map_err(db)?;
    row.map(role_from).ok_or(V2Error(DomainError::NotFound))
}

async fn load_mode<'e, E: sqlx::PgExecutor<'e>>(ex: E) -> V2Result<String> {
    let m: Option<String> =
        sqlx::query_scalar("SELECT mode::text FROM paperrust_role_config WHERE id = 1").fetch_optional(ex).await.map_err(db)?;
    Ok(m.unwrap_or_else(|| MODE_CLASSIC.to_string()))
}

/// All model permissions of the database: `<app_label>.<codename>` → `auth_permission.id`.
async fn known_perms<'e, E: sqlx::PgExecutor<'e>>(ex: E) -> V2Result<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT ct.app_label || '.' || p.codename, p.id::bigint
         FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id",
    )
    .fetch_all(ex)
    .await
    .map_err(db)?;
    Ok(rows.into_iter().collect())
}

fn ids_of(set: &BTreeSet<String>, known: &HashMap<String, i64>) -> Vec<i64> {
    set.iter().filter_map(|p| known.get(p).copied()).collect()
}

fn invalid_pks(field: &str, wanted: &[i64], found: &HashSet<i64>) -> V2Result<()> {
    let missing: Vec<Value> = wanted
        .iter()
        .filter(|i| !found.contains(i))
        .map(|i| Value::String(format!("Invalid pk \"{i}\" - object does not exist.")))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut fields = Map::new();
    fields.insert(field.into(), Value::Array(missing));
    Err(V2Error(DomainError::Invalid { message: "Invalid pk.".into(), fields }))
}

// ---------------------------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------------------------

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/roles", get(list).post(create))
        .route("/roles/mode", put(set_mode))
        .route("/roles/resolve", get(resolve_handler))
        .route("/roles/{id}", axum::routing::patch(patch).delete(delete))
        .route("/roles/{id}/assign", post(assign))
}

/// Read: active and (superuser, staff or `auth.view_user`).
fn may_read(p: &Principal) -> V2Result<&pl_authz::User> {
    let u = d2::user(p)?;
    if u.is_active && (u.is_superuser || u.is_staff || u.has_perm(PERM_VIEW_USER)) { Ok(u) } else { Err(forbidden(DENIED)) }
}

fn require_superuser(p: &Principal) -> V2Result<&pl_authz::User> {
    let u = d2::user(p)?;
    if u.is_active && u.is_superuser { Ok(u) } else { Err(forbidden(DENIED)) }
}

fn parse_id(raw: &str) -> V2Result<i64> {
    raw.trim().parse::<i64>().map_err(|_| V2Error(DomainError::NotFound))
}

async fn overview(pool: &PgPool) -> V2Result<Value> {
    let mode = load_mode(pool).await?;
    let roles = active_roles(load_roles(pool).await?, &mode);
    let topics: Vec<Value> = TOPICS.iter().map(|(k, m)| json!({ "key": k, "models": m })).collect();
    Ok(json!({ "mode": mode, "roles": roles, "topics": topics, "always": ALWAYS }))
}

/// `GET /roles`
pub async fn list(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    may_read(&p)?;
    ensure(state.db()).await?;
    Ok(json(&overview(state.db()).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeBody {
    mode: String,
}

/// `PUT /roles/mode` (superuser only). Custom roles are kept when switching to "classic".
pub async fn set_mode(State(state): State<AppState>, Caller(p): Caller, V2Json(b): V2Json<ModeBody>) -> V2Result<Response> {
    let u = require_superuser(&p)?;
    let mode = check_mode(b.mode.trim())?;
    ensure(state.db()).await?;
    sqlx::query(
        "INSERT INTO paperrust_role_config (id, mode, modified) VALUES (1, $1, now())
         ON CONFLICT (id) DO UPDATE SET mode = EXCLUDED.mode, modified = now()",
    )
    .bind(mode)
    .execute(state.db())
    .await
    .map_err(db)?;
    tracing::info!(target: "paperrust.roles", user = u.id, mode, "role mode changed");
    Ok(json(&overview(state.db()).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleWrite {
    name: Option<String>,
    description: Option<String>,
    permissions: Option<Vec<String>>,
}

async fn custom_mode_or_400(pool: &PgPool) -> V2Result<()> {
    if load_mode(pool).await? != MODE_CUSTOM {
        return Err(V2Error(DomainError::invalid("Custom roles require the custom role mode.")));
    }
    Ok(())
}

fn name_taken(e: sqlx::Error) -> V2Error {
    if is_unique_violation(&e) { V2Error(DomainError::field("name", "A role with this name already exists.")) } else { db(e) }
}

/// `POST /roles` (superuser only, only in "custom" mode) → 201.
pub async fn create(State(state): State<AppState>, Caller(p): Caller, V2Json(w): V2Json<RoleWrite>) -> V2Result<Response> {
    let u = require_superuser(&p)?;
    let pool = state.db();
    ensure(pool).await?;
    custom_mode_or_400(pool).await?;
    let name = clean_name(w.name.as_deref().ok_or_else(|| V2Error(DomainError::field("name", "This field is required.")))?)?;
    let description = clean_description(w.description.as_deref().unwrap_or(""))?;
    let known: HashSet<String> = known_perms(pool).await?.into_keys().collect();
    let perms = validate_permissions(w.permissions.as_deref().unwrap_or(&[]), Some(&known))?;
    let perms: Vec<String> = perms.into_iter().collect();
    let row: RoleRow = sqlx::query_as(&format!(
        "INSERT INTO paperrust_role (name, description, builtin, is_admin, permissions)
         VALUES ($1, $2, false, false, $3) RETURNING {ROLE_COLS}"
    ))
    .bind(&name)
    .bind(&description)
    .bind(&perms)
    .fetch_one(pool)
    .await
    .map_err(name_taken)?;
    let role = role_from(row);
    tracing::info!(target: "paperrust.roles", user = u.id, role = role.id, "role created");
    Ok(json_with_rev(StatusCode::CREATED, &role, None))
}

async fn editable(pool: &PgPool, id: i64, what: &str) -> V2Result<Role> {
    let role = load_role(pool, id).await?;
    if role.builtin {
        return Err(V2Error(DomainError::invalid(format!("Built-in roles cannot be {what}."))));
    }
    custom_mode_or_400(pool).await?;
    Ok(role)
}

/// `PATCH /roles/{id}` (superuser only, not for fixed roles).
pub async fn patch(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(raw): Path<String>,
    V2Json(w): V2Json<RoleWrite>,
) -> V2Result<Response> {
    let u = require_superuser(&p)?;
    let id = parse_id(&raw)?;
    let pool = state.db();
    ensure(pool).await?;
    let role = editable(pool, id, "changed").await?;
    let name = match &w.name {
        Some(n) => clean_name(n)?,
        None => role.name.clone(),
    };
    let description = match &w.description {
        Some(d) => clean_description(d)?,
        None => role.description.clone(),
    };
    let perms: Vec<String> = match &w.permissions {
        Some(l) => {
            let known: HashSet<String> = known_perms(pool).await?.into_keys().collect();
            validate_permissions(l, Some(&known))?.into_iter().collect()
        }
        None => role.permissions.iter().cloned().collect(),
    };
    let row: RoleRow = sqlx::query_as(&format!(
        "UPDATE paperrust_role SET name = $2, description = $3, permissions = $4, modified = now()
         WHERE id = $1 AND NOT builtin RETURNING {ROLE_COLS}"
    ))
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(&perms)
    .fetch_optional(pool)
    .await
    .map_err(name_taken)?
    .ok_or(V2Error(DomainError::NotFound))?;
    tracing::info!(target: "paperrust.roles", user = u.id, role = id, "role changed");
    Ok(json(&role_from(row)))
}

/// `DELETE /roles/{id}` (superuser only, not for fixed roles) → 204. Django permissions stay.
pub async fn delete(State(state): State<AppState>, Caller(p): Caller, Path(raw): Path<String>) -> V2Result<Response> {
    let u = require_superuser(&p)?;
    let id = parse_id(&raw)?;
    let pool = state.db();
    ensure(pool).await?;
    editable(pool, id, "deleted").await?;
    sqlx::query("DELETE FROM paperrust_role WHERE id = $1 AND NOT builtin").bind(id).execute(pool).await.map_err(db)?;
    tracing::info!(target: "paperrust.roles", user = u.id, role = id, "role deleted");
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignBody {
    #[serde(default)]
    users: Vec<i64>,
    #[serde(default)]
    groups: Vec<i64>,
}

fn dedup(mut v: Vec<i64>, field: &str) -> V2Result<Vec<i64>> {
    if v.len() > MAX_IDS {
        return Err(V2Error(DomainError::field(field, format!("At most {MAX_IDS} ids."))));
    }
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// `POST /roles/{id}/assign` – replaces the direct Django permissions of the users/groups with the
/// role set ∪ `always` (one transaction, all or nothing).
pub async fn assign(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(raw): Path<String>,
    V2Json(b): V2Json<AssignBody>,
) -> V2Result<Response> {
    let actor = Actor::from_user(d2::user(&p)?);
    let id = parse_id(&raw)?;
    let users = dedup(b.users, "users")?;
    let groups = dedup(b.groups, "groups")?;
    let pool = state.db();
    ensure(pool).await?;
    let mut tx = pool.begin().await.map_err(db)?;
    let role = load_role(&mut *tx, id).await?;
    let mode = load_mode(&mut *tx).await?;
    check_assign(&actor, &role, &mode, !users.is_empty(), !groups.is_empty())?;
    let known = known_perms(&mut *tx).await?;
    let known_names: HashSet<String> = known.keys().cloned().collect();

    let rows: Vec<(i64, bool, bool)> =
        sqlx::query_as("SELECT id::bigint, is_superuser, is_staff FROM auth_user WHERE id = ANY($1) ORDER BY id FOR UPDATE")
            .bind(&users)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
    invalid_pks("users", &users, &rows.iter().map(|r| r.0).collect())?;
    let group_rows: Vec<i64> = sqlx::query_scalar("SELECT id::bigint FROM auth_group WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(&groups)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
    invalid_pks("groups", &groups, &group_rows.iter().copied().collect())?;

    let targets: Vec<Target> = rows.iter().map(|&(id, s, st)| Target { id, is_superuser: s, is_staff: st }).collect();
    let changes = plan_users(&actor, &role, &targets, Some(&known_names))?;
    for c in &changes {
        let ids = ids_of(&c.permissions, &known);
        sqlx::query("UPDATE auth_user SET is_superuser = $2, is_staff = $3 WHERE id = $1")
            .bind(c.id)
            .bind(c.is_superuser)
            .bind(c.is_staff)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        replace_perms(&mut tx, "auth_user_user_permissions", "user_id", c.id, &ids).await?;
        record(&mut tx, "user", c.id, role.id, actor.id).await?;
    }
    let group_ids = ids_of(&assigned_set(&role, Some(&known_names)), &known);
    for g in &group_rows {
        replace_perms(&mut tx, "auth_group_permissions", "group_id", *g, &group_ids).await?;
        record(&mut tx, "group", *g, role.id, actor.id).await?;
    }
    tx.commit().await.map_err(db)?;
    tracing::info!(
        target: "paperrust.roles",
        user = actor.id,
        role = role.id,
        users = ?users,
        groups = ?groups,
        "role assigned"
    );
    let users_out: Vec<Value> = changes.iter().map(|c| json!({ "id": c.id, "role_id": role.id })).collect();
    let groups_out: Vec<Value> = group_rows.iter().map(|g| json!({ "id": g, "role_id": role.id })).collect();
    Ok(json(&json!({ "users": users_out, "groups": groups_out })))
}

/// Set the direct permissions of a user/group to exactly `ids` (`table`/`owner` are constants).
async fn replace_perms(tx: &mut sqlx::PgConnection, table: &str, owner: &str, owner_id: i64, ids: &[i64]) -> V2Result<()> {
    sqlx::query(&format!("DELETE FROM {table} WHERE {owner} = $1 AND NOT (permission_id = ANY($2))"))
        .bind(owner_id)
        .bind(ids)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query(&format!(
        "INSERT INTO {table} ({owner}, permission_id) SELECT $1::int, x::int FROM unnest($2::bigint[]) AS x ON CONFLICT DO NOTHING"
    ))
    .bind(owner_id)
    .bind(ids)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    Ok(())
}

async fn record(tx: &mut sqlx::PgConnection, subject: &str, subject_id: i64, role: i64, by: i64) -> V2Result<()> {
    sqlx::query(
        "INSERT INTO paperrust_role_assignment (subject, subject_id, role_id, assigned_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (subject, subject_id) DO UPDATE SET role_id = EXCLUDED.role_id, assigned = now(),
           assigned_by = EXCLUDED.assigned_by",
    )
    .bind(subject)
    .bind(subject_id)
    .bind(role)
    .bind(by)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    Ok(())
}

/// `GET /roles/resolve?users=1,2&groups=3` – unknown ids are missing from the response.
pub async fn resolve_handler(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
) -> V2Result<Response> {
    let u = may_read(&p)?;
    let users = parse_ids("users", q.get("users"))?;
    let groups = parse_ids("groups", q.get("groups"))?;
    if !groups.is_empty() && !(u.is_superuser || u.is_staff || u.has_perm(PERM_VIEW_GROUP)) {
        return Err(forbidden(DENIED));
    }
    let pool = state.db();
    ensure(pool).await?;
    let mode = load_mode(pool).await?;
    let roles = active_roles(load_roles(pool).await?, &mode);
    let known: HashSet<String> = known_perms(pool).await?.into_keys().collect();
    const PERMS: &str = "ARRAY(SELECT ct.app_label || '.' || p.codename FROM {t} x
        JOIN auth_permission p ON p.id = x.permission_id JOIN django_content_type ct ON ct.id = p.content_type_id
        WHERE x.{o} = s.id)";
    let user_rows: Vec<(i64, bool, Vec<String>)> = sqlx::query_as(&format!(
        "SELECT s.id::bigint, s.is_superuser, {} FROM auth_user s WHERE s.id = ANY($1)",
        PERMS.replace("{t}", "auth_user_user_permissions").replace("{o}", "user_id")
    ))
    .bind(&users)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let group_rows: Vec<(i64, Vec<String>)> = sqlx::query_as(&format!(
        "SELECT s.id::bigint, {} FROM auth_group s WHERE s.id = ANY($1)",
        PERMS.replace("{t}", "auth_group_permissions").replace("{o}", "group_id")
    ))
    .bind(&groups)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let mut uo = BTreeMap::new();
    for (id, su, perms) in user_rows {
        let set: BTreeSet<String> = perms.into_iter().collect();
        uo.insert(id, resolve(su, &set, &roles, Some(&known)).to_json());
    }
    let mut go = BTreeMap::new();
    for (id, perms) in group_rows {
        let set: BTreeSet<String> = perms.into_iter().collect();
        go.insert(id, resolve(false, &set, &roles, Some(&known)).to_json());
    }
    let to_obj = |m: BTreeMap<i64, Value>| -> Map<String, Value> { m.into_iter().map(|(k, v)| (k.to_string(), v)).collect() };
    Ok(json(&json!({ "users": to_obj(uo), "groups": to_obj(go) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(id: i64, key: &str) -> Role {
        Role {
            id,
            key: Some(key.into()),
            name: key.into(),
            description: String::new(),
            builtin: true,
            admin: key == "admin",
            permissions: builtin_set(key).unwrap(),
        }
    }

    fn roles() -> Vec<Role> {
        vec![builtin(1, "read"), builtin(2, "edit"), builtin(3, "manage"), builtin(4, "admin")]
    }

    fn actor(id: i64, su: bool, perms: &[&str]) -> Actor {
        Actor { id, is_active: true, is_superuser: su, permissions: perms.iter().map(|s| s.to_string()).collect() }
    }

    fn known_all() -> HashSet<String> {
        allowed_codenames().into_iter().collect()
    }

    /// Every fixed role → assigned set → detection yields the same role again.
    #[test]
    fn round_trip_builtin() {
        let rs = roles();
        let known = known_all();
        let su = actor(99, true, &[]);
        for r in &rs {
            let t = Target { id: 10, is_superuser: false, is_staff: false };
            let ch = plan_users(&su, r, &[t], Some(&known)).unwrap().remove(0);
            for a in ALWAYS {
                assert!(ch.permissions.contains(*a), "{a} fehlt bei {:?}", r.key);
            }
            assert_eq!(resolve(ch.is_superuser, &ch.permissions, &rs, Some(&known)), Resolved::Role(r.id), "{:?}", r.key);
            // Groups (without admin): same set, detection without superuser flag
            if !r.admin {
                let g = assigned_set(r, Some(&known));
                assert_eq!(resolve(false, &g, &rs, Some(&known)), Resolved::Role(r.id));
            }
        }
    }

    #[test]
    fn resolve_ignores_always_and_detects_individual() {
        let rs = roles();
        let mut s = builtin_set("read").unwrap();
        assert_eq!(resolve(false, &s, &rs, None), Resolved::Role(1));
        s.insert("documents.add_document".into());
        assert_eq!(resolve(false, &s, &rs, None), Resolved::Individual);
        assert_eq!(resolve(false, &BTreeSet::new(), &rs, None), Resolved::Individual);
        assert_eq!(resolve(true, &BTreeSet::new(), &rs, None), Resolved::Role(4));
        assert_eq!(resolve(true, &BTreeSet::new(), &rs[..3], None), Resolved::Admin);
    }

    /// Older database without `sharelinkbundle`: compare and write with the existing permissions.
    #[test]
    fn resolve_with_missing_permissions_in_db() {
        let rs = roles();
        let known: HashSet<String> = known_all().into_iter().filter(|p| !p.ends_with("_sharelinkbundle")).collect();
        let set = assigned_set(&rs[1], Some(&known));
        assert!(!set.iter().any(|p| p.ends_with("_sharelinkbundle")));
        assert_eq!(resolve(false, &set, &rs, Some(&known)), Resolved::Role(2));
    }

    #[test]
    fn builtin_sets_nested_and_allowed() {
        let allowed = allowed_codenames();
        let r = builtin_set("read").unwrap();
        let e = builtin_set("edit").unwrap();
        let m = builtin_set("manage").unwrap();
        assert!(r.is_subset(&e) && e.is_subset(&m) && r.len() < e.len() && e.len() < m.len());
        assert!(m.is_subset(&allowed));
        assert!(builtin_set("admin").unwrap().is_empty());
        assert!(builtin_set("x").is_none());
        for a in ALWAYS {
            assert!(!m.contains(*a));
        }
        // no system permissions in the fixed roles
        assert!(!m.iter().any(|p| p.starts_with("auth.") || p.starts_with("paperless.")));
    }

    /// The seeds in the migration match the constants.
    #[test]
    fn seed_matches_constants() {
        let sql = MIGRATION.1;
        for key in ["read", "edit", "manage", "admin"] {
            let start = sql.find(&format!("('{key}',")).unwrap_or_else(|| panic!("seed {key}"));
            let rest = &sql[start..];
            let line = &rest[..rest.find('\n').unwrap()];
            let a = line.rfind("'{").unwrap() + 2;
            let b = line.rfind("}'").unwrap();
            let seeded: BTreeSet<String> = line[a..b].split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
            assert_eq!(seeded, builtin_set(key).unwrap(), "{key}");
            assert_eq!(line.contains("true, true"), key == "admin");
        }
    }

    #[test]
    fn assign_admin_sets_flags_and_refuses_groups() {
        let rs = roles();
        let su = actor(1, true, &[]);
        let ch = plan_users(&su, &rs[3], &[Target { id: 5, is_superuser: false, is_staff: false }], None).unwrap();
        assert!(ch[0].is_superuser && ch[0].is_staff);
        assert_eq!(ch[0].permissions, always_set());
        let e = check_assign(&su, &rs[3], MODE_CLASSIC, false, true).unwrap_err();
        assert!(matches!(e.0, DomainError::Invalid { .. }));
        assert!(check_assign(&su, &rs[3], MODE_CLASSIC, true, false).is_ok());
    }

    #[test]
    fn assign_demotes_superuser_but_not_self() {
        let rs = roles();
        let su = actor(1, true, &[]);
        let other = Target { id: 2, is_superuser: true, is_staff: true };
        let ch = plan_users(&su, &rs[0], &[other], None).unwrap();
        assert!(!ch[0].is_superuser && !ch[0].is_staff);
        let me = Target { id: 1, is_superuser: true, is_staff: true };
        let e = plan_users(&su, &rs[0], &[other, me], None).unwrap_err();
        assert_eq!(e.0.to_string(), "You cannot remove your own administrator role");
        // Admin role for oneself: still allowed unchanged
        assert!(plan_users(&su, &rs[3], &[me], None).is_ok());
        // non-superuser target: is_staff stays
        let staff = Target { id: 3, is_superuser: false, is_staff: true };
        assert!(plan_users(&su, &rs[1], &[staff], None).unwrap()[0].is_staff);
    }

    #[test]
    fn non_superuser_assign_rules() {
        let rs = roles();
        let mut perms: Vec<&str> = vec![PERM_CHANGE_USER];
        perms.extend(READ);
        let a = actor(7, false, &perms);
        assert!(check_assign(&a, &rs[0], MODE_CLASSIC, true, false).is_ok());
        // groups without change_group
        assert!(matches!(check_assign(&a, &rs[0], MODE_CLASSIC, false, true).unwrap_err().0, DomainError::Forbidden(_)));
        // privilege escalation
        assert!(matches!(check_assign(&a, &rs[1], MODE_CLASSIC, true, false).unwrap_err().0, DomainError::Forbidden(_)));
        // admin role only by a superuser
        assert!(matches!(check_assign(&a, &rs[3], MODE_CLASSIC, true, false).unwrap_err().0, DomainError::Forbidden(_)));
        // superuser target only by a superuser
        let t = Target { id: 2, is_superuser: true, is_staff: true };
        assert!(matches!(plan_users(&a, &rs[0], &[t], None).unwrap_err().0, DomainError::Forbidden(_)));
        // without change_user
        let b = actor(8, false, READ);
        assert!(matches!(check_assign(&b, &rs[0], MODE_CLASSIC, true, false).unwrap_err().0, DomainError::Forbidden(_)));
    }

    #[test]
    fn custom_mode_guard() {
        let su = actor(1, true, &[]);
        let custom = Role {
            id: 10,
            key: None,
            name: "Buchhaltung".into(),
            description: String::new(),
            builtin: false,
            admin: false,
            permissions: ["documents.view_document".to_string()].into(),
        };
        assert!(check_assign(&su, &custom, MODE_CLASSIC, true, false).is_err());
        assert!(check_assign(&su, &custom, MODE_CUSTOM, true, false).is_ok());
        let all = vec![builtin(1, "read"), custom.clone()];
        assert_eq!(active_roles(all.clone(), MODE_CLASSIC).len(), 1);
        assert_eq!(active_roles(all.clone(), MODE_CUSTOM).len(), 2);
        // Detection in custom mode: a custom role is recognised
        let set = assigned_set(&custom, None);
        assert_eq!(resolve(false, &set, &active_roles(all.clone(), MODE_CUSTOM), None), Resolved::Role(10));
        assert_eq!(resolve(false, &set, &active_roles(all, MODE_CLASSIC), None), Resolved::Individual);
        assert!(check_mode("custom").is_ok() && check_mode("other").is_err());
    }

    #[test]
    fn validate_permission_lists() {
        let known = known_all();
        let ok = validate_permissions(
            &["documents.view_document".into(), "documents.view_uisettings".into(), "auth.view_user".into()],
            Some(&known),
        )
        .unwrap();
        assert_eq!(ok.len(), 2, "always wird entfernt");
        assert!(validate_permissions(&["documents.view_workflowtrigger".into()], Some(&known)).is_err());
        assert!(validate_permissions(&["nonsense".into()], None).is_err());
        let partial: HashSet<String> = ["documents.view_tag".to_string()].into();
        assert!(validate_permissions(&["documents.view_document".into()], Some(&partial)).is_err());
    }

    #[test]
    fn id_lists() {
        assert_eq!(parse_ids("users", Some(&"3, 1,3".to_string())).unwrap(), vec![1, 3]);
        assert!(parse_ids("users", Some(&"a".to_string())).is_err());
        assert!(parse_ids("users", Some(&"0".to_string())).is_err());
        assert!(parse_ids("users", None).unwrap().is_empty());
    }

    #[test]
    fn topics_cover_builtin_models() {
        let models: HashSet<&str> = TOPICS.iter().flat_map(|(_, m)| m.iter().copied()).collect();
        for p in builtin_set("manage").unwrap() {
            let (app, code) = p.split_once('.').unwrap();
            let model = code.split_once('_').unwrap().1;
            assert!(models.contains(format!("{app}.{model}").as_str()), "{p}");
        }
        assert_eq!(TOPICS.iter().map(|t| t.0).collect::<Vec<_>>(), ["documents", "organize", "views", "automation", "system"]);
    }
}
