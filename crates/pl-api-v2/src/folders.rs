//! `/rs/api/v2/folders` – folder view (results/ideas/2026-11-22-papermerge-ordneransicht.md, stage 1;
//! contract docs/contract/pl-api-v2.md "Ordner"). New in PaperRust, no paperless-ngx counterpart.
//!
//! Data model (migration `pl_api_v2.0002_paperrust_folder`): `paperrust_folder` (tree as adjacency list,
//! names unique per level), `paperrust_document_folder` (at most one folder per document; no row =
//! "Unsorted"), `paperrust_folder_grant` (shares, same encoding as `paperrust_object_grant`).
//!
//! Permissions:
//! * using folders at all: active user with `documents.view_document`; creating, changing, deleting
//!   folders and moving documents additionally needs `documents.change_document`;
//! * a folder is visible to superusers, its owner, everyone if it has no owner, and users/groups with a
//!   view share (bit 1); change needs owner/ownerless/superuser or bit 2, delete bit 4;
//! * folder shares do **not** extend to the documents inside (document permissions stay per document);
//! * moving documents needs `change_document` on every document (object permission) and change on the
//!   target folder;
//! * deleting a folder that still has contents moves them, so it needs the same rights as moving them
//!   directly: delete on the folder, `change_document` on every document in it, change on every direct
//!   subfolder and (`contents=parent`) change on the parent folder. Otherwise 403 without naming the
//!   objects.
//!
//! Locking (READ COMMITTED): permission checks and writes of `move_documents`/`delete` run in one
//! transaction after the rows they depend on are locked - lock order caller's `auth_user` row (`FOR SHARE`)
//! -> caller's groups (`FOR SHARE`, id order; the caller's rights are re-read after these two, see
//! [`fresh_caller`]) -> tree lock -> folder being deleted
//! (`FOR UPDATE`) -> its subfolders (`FOR UPDATE`, id order) -> target/parent folder (`FOR SHARE`) ->
//! documents (`FOR SHARE`, id order). Folder shares only change under `FOR UPDATE` of the folder (`patch`); owner
//! changes, trash and deletion of documents update the document row; group membership, the superuser/active
//! flags and direct permissions change under the user row lock, group permissions under the group row lock
//! (pl-auth-api users/groups, roles apply). So none of them can slip in between the check and the write.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use pl_authz::User;
use pl_core::AppState;
use pl_domain::{DomainError, Kind, Principal};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::d2;
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, if_match, json, json_with_rev};

/// Migration (name, SQL) for `pl_cli::migrate::paperrust_migrations()`.
pub const MIGRATION: (&str, &str) = ("pl_api_v2.0002_paperrust_folder", include_str!("../migrations/0002_paperrust_folder.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

/// Longest folder name (column `varchar(128)`, like tag names).
pub const NAME_MAX: usize = 128;
/// Most documents per `move-documents` call.
pub const MOVE_MAX: usize = 50_000;

pub const PERM_CHANGE_DOCUMENT: &str = "documents.change_document";

/// Share bits (same as `paperrust_object_grant.perms`).
pub const BIT_VIEW: i16 = 1;
pub const BIT_CHANGE: i16 = 2;
pub const BIT_DELETE: i16 = 4;

static READY: AtomicBool = AtomicBool::new(false);

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/folders", get(list).post(create))
        .route("/folders/move-documents", post(move_documents))
        .route("/folders/{id}", get(one).patch(patch).delete(delete))
}

/// Create the tables (once per process; idempotent).
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

fn sql_state(e: &sqlx::Error) -> Option<String> {
    e.as_database_error().and_then(|d| d.code()).map(|c| c.to_string())
}

const DUPLICATE_NAME: &str = "A folder with this name already exists here.";

/// Unique name violated → field error; anything else internal.
fn write_err(e: sqlx::Error) -> V2Error {
    match sql_state(&e).as_deref() {
        Some("23505") => V2Error(DomainError::field("name", DUPLICATE_NAME)),
        Some("23503") => V2Error(DomainError::field("parent", "Folder does not exist.")),
        _ => db(e),
    }
}

// ------------------------------------------------------------------------- caller

/// Caller as seen by the folder rules.
#[derive(Debug, Clone)]
pub struct Access {
    pub user_id: i64,
    pub superuser: bool,
    /// `[user_id, -group_id, …]` (subjects as in `paperrust_folder_grant`)
    pub subjects: Vec<i64>,
    pub can_organize: bool,
}

impl Access {
    pub fn of(u: &User) -> V2Result<Access> {
        if !(u.is_active && u.has_perm(d2::PERM_VIEW_DOCUMENT)) {
            return Err(V2Error(DomainError::Forbidden("You do not have permission to perform this action.".into())));
        }
        let mut subjects = vec![u.id];
        subjects.extend(u.groups.iter().map(|g| -g));
        Ok(Access { user_id: u.id, superuser: u.is_superuser, subjects, can_organize: u.has_perm(PERM_CHANGE_DOCUMENT) })
    }

    fn require_organize(&self) -> V2Result<()> {
        if self.can_organize {
            Ok(())
        } else {
            Err(V2Error(DomainError::Forbidden("You do not have permission to perform this action.".into())))
        }
    }

    /// SQL condition "folder `alias` has `bit` for the caller"; `$1` = user id, `$2` = subjects.
    pub fn cond(&self, alias: &str, bit: i16) -> String {
        if self.superuser {
            return "TRUE".into();
        }
        format!(
            "({alias}.owner_id = $1 OR {alias}.owner_id IS NULL OR EXISTS (SELECT 1 FROM paperrust_folder_grant pfg \
             WHERE pfg.folder_id = {alias}.id AND pfg.subject = ANY($2) AND pfg.perms & {bit} <> 0))"
        )
    }

    /// May the caller change owner and shares of `f` (superuser, owner, or folder without owner)?
    fn may_set_owner(&self, f: &FolderRow) -> bool {
        self.superuser || f.owner_id.is_none_or(|o| i64::from(o) == self.user_id)
    }
}

fn access(p: &Principal) -> V2Result<Access> {
    Access::of(d2::user(p)?)
}

// ------------------------------------------------------------------------- rows

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FolderRow {
    pub id: i64,
    pub uuid: Uuid,
    pub rev: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub owner_id: Option<i32>,
    pub sort: i32,
    pub color: Option<String>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub can_change: bool,
    pub can_delete: bool,
}

/// Folder in API form (fields in this order).
#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    pub id: i64,
    pub uuid: Uuid,
    pub rev: i64,
    pub name: String,
    pub parent: Option<i64>,
    pub owner: Option<i64>,
    pub color: Option<String>,
    pub sort: i32,
    pub user_can_change: bool,
    pub user_can_delete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_count: Option<i64>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

impl From<&FolderRow> for Folder {
    fn from(r: &FolderRow) -> Self {
        Folder {
            id: r.id,
            uuid: r.uuid,
            rev: r.rev,
            name: r.name.clone(),
            parent: r.parent_id,
            owner: r.owner_id.map(i64::from),
            color: r.color.clone(),
            sort: r.sort,
            user_can_change: r.can_change,
            user_can_delete: r.can_delete,
            document_count: None,
            created: r.created,
            modified: r.modified,
        }
    }
}

fn select_sql(a: &Access) -> String {
    format!(
        "SELECT f.id, f.uuid, f.rev, f.name::text AS name, f.parent_id, f.owner_id, f.sort, f.color::text AS color, f.created, \
         f.modified, {} AS can_change, {} AS can_delete FROM paperrust_folder f WHERE {}",
        a.cond("f", BIT_CHANGE),
        a.cond("f", BIT_DELETE),
        a.cond("f", BIT_VIEW)
    )
}

async fn visible_folders(pool: &PgPool, a: &Access) -> V2Result<Vec<FolderRow>> {
    sqlx::query_as::<_, FolderRow>(&format!("{} ORDER BY f.sort, lower(f.name), f.id", select_sql(a)))
        .bind(a.user_id)
        .bind(&a.subjects)
        .fetch_all(pool)
        .await
        .map_err(db)
}

/// Row lock taken by [`visible_folder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowLock {
    No,
    /// `FOR SHARE`: the folder (and with it its shares) cannot change or vanish until commit
    Share,
    /// `FOR UPDATE`: the caller changes or deletes the folder
    Update,
}

async fn visible_folder<'e, E: sqlx::PgExecutor<'e>>(ex: E, a: &Access, id: i64, lock: RowLock) -> V2Result<FolderRow> {
    let lock = match lock {
        RowLock::No => "",
        RowLock::Share => " FOR SHARE OF f",
        RowLock::Update => " FOR UPDATE OF f",
    };
    sqlx::query_as::<_, FolderRow>(&format!("{} AND f.id = $3{lock}", select_sql(a)))
        .bind(a.user_id)
        .bind(&a.subjects)
        .bind(id)
        .fetch_optional(ex)
        .await
        .map_err(db)?
        .ok_or(V2Error(DomainError::NotFound))
}

/// Visible document ids (root documents outside the trash) as a subquery.
fn visible_docs_sql(p: &Principal) -> V2Result<String> {
    Ok(pl_store::visible_ids_sql(p, Kind::Document)?)
}

/// Base table of the documents for row locks: `paperrust_document` from data model stage 1 (then
/// `documents_document` is a view on which `FOR SHARE` does not work), otherwise the Django table.
async fn document_lock_table(tx: &mut sqlx::PgConnection) -> V2Result<&'static str> {
    pl_authz::grants::document_lock_table(tx).await.map_err(db)
}

/// Lock the document rows `ids` (`FOR SHARE`, id order): owner changes, trash and deletion of these
/// documents wait until the transaction ends, so a permission check made afterwards stays valid for
/// the write in the same transaction.
async fn lock_documents(tx: &mut sqlx::PgConnection, table: &str, ids: &[i32]) -> V2Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(&format!("SELECT id FROM {table} WHERE id = ANY($1::int4[]) ORDER BY id FOR SHARE"))
        .bind(ids)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    Ok(())
}

/// Number of existing documents among `ids` without `change_document` for `user` (0 for superusers).
/// Ids without a document row (removed for good) are not counted.
async fn count_unchangeable(tx: &mut sqlx::PgConnection, table: &str, user: &User, ids: &[i32], include_deleted: bool) -> V2Result<i64> {
    if user.is_superuser || ids.is_empty() {
        return Ok(0);
    }
    let change = pl_authz::objects::permitted_document_ids(Some(user), "change_document", include_deleted)
        .to_string(sea_query::PostgresQueryBuilder);
    sqlx::query_scalar(&format!(
        "SELECT count(*)::int8 FROM {table} d WHERE d.id = ANY($1::int4[]) \
         AND NOT EXISTS (SELECT 1 FROM ({change}) c(id) WHERE c.id = d.id)"
    ))
    .bind(ids)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)
}

/// Re-read the caller inside the transaction `tx` after locking what their rights depend on: the
/// `auth_user` row and their groups (`FOR SHARE`, id order). The request's user object can be older than
/// an admin change (membership, superuser/active flag, user or group permissions); with these locks such a
/// change either committed before (and is seen here) or waits until this transaction ends.
/// Gone or deactivated meanwhile -> 403 like a missing permission.
async fn fresh_caller(tx: &mut sqlx::PgConnection, id: i64) -> V2Result<(User, Access)> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id::int8 FROM auth_user WHERE id = $1 FOR SHARE").bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
    if found.is_none() {
        return Err(V2Error(DomainError::Forbidden("You do not have permission to perform this action.".into())));
    }
    sqlx::query(
        "SELECT g.id FROM auth_group g JOIN auth_user_groups ug ON ug.group_id = g.id          WHERE ug.user_id = $1 ORDER BY g.id FOR SHARE OF g",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    let user = pl_authz::user::load_user_by_id(&mut *tx, id)
        .await
        .map_err(db)?
        .ok_or(V2Error(DomainError::Forbidden("You do not have permission to perform this action.".into())))?;
    let a = Access::of(&user)?;
    a.require_organize()?;
    Ok((user, a))
}

/// Visible documents per folder (only direct contents) and the number of visible "Unsorted" documents.
async fn counts(pool: &PgPool, p: &Principal) -> V2Result<(HashMap<i64, i64>, i64)> {
    let vis = visible_docs_sql(p)?;
    let rows: Vec<(i64, i64)> = sqlx::query_as(&format!(
        "SELECT m.folder_id, count(*)::int8 FROM paperrust_document_folder m WHERE m.document_id IN ({vis}) GROUP BY m.folder_id"
    ))
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let unsorted: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*)::int8 FROM ({vis}) v(id) WHERE NOT EXISTS (SELECT 1 FROM paperrust_document_folder m WHERE m.document_id = v.id)"
    ))
    .fetch_one(pool)
    .await
    .map_err(db)?;
    Ok((rows.into_iter().collect(), unsorted))
}

// ------------------------------------------------------------------------- pure helpers

/// Trimmed, valid folder name.
pub fn clean_name(raw: &str) -> Result<String, &'static str> {
    let n = raw.trim();
    if n.is_empty() {
        return Err("This field may not be blank.");
    }
    if n.chars().count() > NAME_MAX {
        return Err("Ensure this field has no more than 128 characters.");
    }
    if n.chars().any(char::is_control) {
        return Err("Control characters are not allowed.");
    }
    Ok(n.to_string())
}

/// `#rrggbb` (lower case) or `None` for an empty value.
pub fn clean_color(raw: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(c) = raw.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(None) };
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
    if ok { Ok(Some(c.to_ascii_lowercase())) } else { Err("Expected a colour like #a1b2c3.") }
}

/// Path from the root to `id` over the visible folders (`parent` → id). Stops at a parent the caller
/// does not see and on cycles (cannot happen, defensive).
pub fn path_to(parents: &HashMap<i64, Option<i64>>, id: i64) -> Vec<i64> {
    let mut out = vec![id];
    let mut seen = HashSet::from([id]);
    let mut cur = parents.get(&id).copied().flatten();
    while let Some(p) = cur {
        if !parents.contains_key(&p) || !seen.insert(p) {
            break;
        }
        out.push(p);
        cur = parents.get(&p).copied().flatten();
    }
    out.reverse();
    out
}

/// Would `parent` as new parent of `id` create a cycle (`parent` is `id` or one of its descendants)?
/// `parents` covers **all** folders (not only the visible ones).
pub fn creates_cycle(parents: &HashMap<i64, Option<i64>>, id: i64, parent: i64) -> bool {
    let mut seen = HashSet::new();
    let mut cur = Some(parent);
    while let Some(p) = cur {
        if p == id {
            return true;
        }
        if !seen.insert(p) {
            return true;
        }
        cur = parents.get(&p).copied().flatten();
    }
    false
}

/// `Some(Some(x))` for a given value, `Some(None)` for `null`, `None` for a missing field.
fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Integer id or UUID string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Ref {
    Id(i64),
    Uuid(Uuid),
}

// ------------------------------------------------------------------------- shares

/// `{"view": {"users": [..], "groups": [..]}, "change": {..}, "delete": {..}}` like v1
/// `set_permissions` (plus `delete`, which v1 also knows).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PermSet {
    #[serde(default)]
    pub view: Subjects,
    #[serde(default)]
    pub change: Subjects,
    #[serde(default)]
    pub delete: Subjects,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Subjects {
    #[serde(default)]
    pub users: Vec<i64>,
    #[serde(default)]
    pub groups: Vec<i64>,
}

impl PermSet {
    /// Rows `(subject, perms)` (change/delete imply view, like the classic sharing dialog shows it).
    pub fn rows(&self) -> Vec<(i64, i16)> {
        let mut m: std::collections::BTreeMap<i64, i16> = std::collections::BTreeMap::new();
        for (s, bit) in [(&self.view, BIT_VIEW), (&self.change, BIT_CHANGE), (&self.delete, BIT_DELETE)] {
            for u in &s.users {
                *m.entry(*u).or_default() |= bit | BIT_VIEW;
            }
            for g in &s.groups {
                *m.entry(-*g).or_default() |= bit | BIT_VIEW;
            }
        }
        m.into_iter().collect()
    }

    pub fn from_rows(rows: &[(i64, i16)]) -> PermSet {
        let mut p = PermSet::default();
        for (subject, perms) in rows {
            for (bit, s) in [(BIT_VIEW, &mut p.view), (BIT_CHANGE, &mut p.change), (BIT_DELETE, &mut p.delete)] {
                if perms & bit != 0 {
                    if *subject > 0 { s.users.push(*subject) } else { s.groups.push(-*subject) }
                }
            }
        }
        p
    }
}

async fn check_users(tx: &mut sqlx::PgConnection, field: &str, ids: &[i64]) -> V2Result<()> {
    let want: BTreeSet<i64> = ids.iter().copied().collect();
    if want.is_empty() {
        return Ok(());
    }
    let list: Vec<i64> = want.iter().copied().collect();
    let found: Vec<i64> = sqlx::query_scalar("SELECT id::int8 FROM auth_user WHERE id = ANY($1::int8[])")
        .bind(&list)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(m) = want.iter().find(|x| !found.contains(x)) {
        return Err(V2Error(DomainError::field(field, format!("Invalid pk \"{m}\" - object does not exist."))));
    }
    Ok(())
}

async fn check_groups(tx: &mut sqlx::PgConnection, ids: &[i64]) -> V2Result<()> {
    let want: BTreeSet<i64> = ids.iter().copied().collect();
    if want.is_empty() {
        return Ok(());
    }
    let list: Vec<i64> = want.iter().copied().collect();
    let found: Vec<i64> = sqlx::query_scalar("SELECT id::int8 FROM auth_group WHERE id = ANY($1::int8[])")
        .bind(&list)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(m) = want.iter().find(|x| !found.contains(x)) {
        return Err(V2Error(DomainError::field("set_permissions", format!("Invalid pk \"{m}\" - object does not exist."))));
    }
    Ok(())
}

async fn write_grants(tx: &mut sqlx::PgConnection, folder: i64, p: &PermSet) -> V2Result<()> {
    let users: Vec<i64> = [&p.view, &p.change, &p.delete].iter().flat_map(|s| s.users.iter().copied()).collect();
    let groups: Vec<i64> = [&p.view, &p.change, &p.delete].iter().flat_map(|s| s.groups.iter().copied()).collect();
    check_users(tx, "set_permissions", &users).await?;
    check_groups(tx, &groups).await?;
    sqlx::query("DELETE FROM paperrust_folder_grant WHERE folder_id = $1").bind(folder).execute(&mut *tx).await.map_err(db)?;
    let rows = p.rows();
    if !rows.is_empty() {
        let subjects: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let perms: Vec<i16> = rows.iter().map(|r| r.1).collect();
        sqlx::query(
            "INSERT INTO paperrust_folder_grant (folder_id, subject, perms) SELECT $1, s, p FROM unnest($2::int8[], $3::int2[]) AS x(s, p)",
        )
        .bind(folder)
        .bind(&subjects)
        .bind(&perms)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    }
    Ok(())
}

async fn read_grants(pool: &PgPool, folder: i64) -> V2Result<PermSet> {
    let rows: Vec<(i64, i16)> = sqlx::query_as("SELECT subject, perms FROM paperrust_folder_grant WHERE folder_id = $1 ORDER BY subject")
        .bind(folder)
        .fetch_all(pool)
        .await
        .map_err(db)?;
    Ok(PermSet::from_rows(&rows))
}

// ------------------------------------------------------------------------- handlers

/// `GET /folders[?counts=0]` – all visible folders (flat, `parent` builds the tree), with
/// `document_count` per folder and `unsorted` (visible documents without a folder).
pub async fn list(State(state): State<AppState>, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let a = access(&p)?;
    let pool = state.db();
    ensure(pool).await?;
    let rows = visible_folders(pool, &a).await?;
    let with_counts = !matches!(q.get("counts").map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("0" | "false" | "no"));
    let (per, unsorted) = if with_counts { counts(pool, &p).await.map(|(m, u)| (Some(m), Some(u)))? } else { (None, None) };
    let items: Vec<Folder> = rows
        .iter()
        .map(|r| {
            let mut f = Folder::from(r);
            f.document_count = per.as_ref().map(|m| m.get(&r.id).copied().unwrap_or(0));
            f
        })
        .collect();
    Ok(json(&json!({ "items": items, "unsorted": unsorted, "can_organize": a.can_organize })))
}

/// `GET /folders/{id}` – one folder with `path` (visible ancestors from the root, incl. itself) and, for
/// owner/superuser, `permissions`.
pub async fn one(State(state): State<AppState>, Caller(p): Caller, Path(id): Path<i64>) -> V2Result<Response> {
    let a = access(&p)?;
    let pool = state.db();
    ensure(pool).await?;
    let f = visible_folder(pool, &a, id, RowLock::No).await?;
    detail(pool, &a, &f, StatusCode::OK).await
}

async fn detail(pool: &PgPool, a: &Access, f: &FolderRow, status: StatusCode) -> V2Result<Response> {
    let all = visible_folders(pool, a).await?;
    let parents: HashMap<i64, Option<i64>> = all.iter().map(|r| (r.id, r.parent_id)).collect();
    let names: HashMap<i64, &str> = all.iter().map(|r| (r.id, r.name.as_str())).collect();
    let path: Vec<Value> =
        path_to(&parents, f.id).into_iter().map(|i| json!({ "id": i, "name": names.get(&i).copied().unwrap_or_default() })).collect();
    let mut v = serde_json::to_value(Folder::from(f)).map_err(|e| V2Error(DomainError::internal(e)))?;
    v["path"] = Value::Array(path);
    if a.may_set_owner(f) {
        v["permissions"] = serde_json::to_value(read_grants(pool, f.id).await?).map_err(|e| V2Error(DomainError::internal(e)))?;
    }
    Ok(json_with_rev(status, &v, Some(f.rev)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderWrite {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub parent: Option<Option<i64>>,
    #[serde(default, deserialize_with = "nullable")]
    pub color: Option<Option<String>>,
    #[serde(default)]
    pub sort: Option<i32>,
    #[serde(default, deserialize_with = "nullable")]
    pub owner: Option<Option<i64>>,
    #[serde(default)]
    pub set_permissions: Option<PermSet>,
}

/// Parent must be visible and changeable for the caller.
async fn check_parent(tx: &mut sqlx::PgConnection, a: &Access, parent: i64) -> V2Result<()> {
    match visible_folder(&mut *tx, a, parent, RowLock::No).await {
        Ok(p) if p.can_change => Ok(()),
        Ok(_) => Err(V2Error(DomainError::Forbidden("You do not have permission to add folders here.".into()))),
        Err(V2Error(DomainError::NotFound)) => Err(V2Error(DomainError::field("parent", "Folder does not exist."))),
        Err(e) => Err(e),
    }
}

/// Serialises changes of the tree shape (parent changes, deletes) – rules out cycles from concurrent moves.
async fn tree_lock(tx: &mut sqlx::PgConnection) -> V2Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_folder_tree'))").execute(&mut *tx).await.map_err(db)?;
    Ok(())
}

/// `POST /folders` `{name, parent?, color?, sort?, owner?, set_permissions?}` → 201.
pub async fn create(State(state): State<AppState>, Caller(p): Caller, V2Json(w): V2Json<FolderWrite>) -> V2Result<Response> {
    let a = access(&p)?;
    a.require_organize()?;
    let pool = state.db();
    ensure(pool).await?;
    let name = clean_name(w.name.as_deref().unwrap_or("")).map_err(|m| V2Error(DomainError::field("name", m)))?;
    let color = clean_color(w.color.clone().flatten().as_deref()).map_err(|m| V2Error(DomainError::field("color", m)))?;
    let parent = w.parent.flatten();
    // owner: default the caller; someone else / nobody only for superusers
    let owner = match w.owner {
        None => Some(a.user_id),
        Some(o) if a.superuser || o == Some(a.user_id) => o,
        Some(_) => return Err(V2Error(DomainError::Forbidden("Only superusers can create folders for other users.".into()))),
    };
    let mut tx = pool.begin().await.map_err(db)?;
    if let Some(pid) = parent {
        tree_lock(&mut tx).await?;
        check_parent(&mut tx, &a, pid).await?;
    }
    if let Some(o) = owner {
        check_users(&mut tx, "owner", &[o]).await?;
    }
    let owner_i32 = owner.map(|o| i32::try_from(o).unwrap_or(i32::MAX));
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO paperrust_folder (name, parent_id, owner_id, sort, color) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&name)
    .bind(parent)
    .bind(owner_i32)
    .bind(w.sort.unwrap_or(0))
    .bind(&color)
    .fetch_one(&mut *tx)
    .await
    .map_err(write_err)?;
    if let Some(ps) = &w.set_permissions {
        write_grants(&mut tx, id, ps).await?;
    }
    tx.commit().await.map_err(db)?;
    let f = visible_folder(pool, &a, id, RowLock::No).await?;
    detail(pool, &a, &f, StatusCode::CREATED).await
}

/// `PATCH /folders/{id}` (optionally `If-Match: "<rev>"`).
pub async fn patch(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    V2Json(w): V2Json<FolderWrite>,
) -> V2Result<Response> {
    let a = access(&p)?;
    a.require_organize()?;
    let expected = if_match(&headers)?;
    let pool = state.db();
    ensure(pool).await?;
    let mut tx = pool.begin().await.map_err(db)?;
    let moves = matches!(w.parent, Some(Some(_)));
    if moves {
        tree_lock(&mut tx).await?;
    }
    let f = visible_folder(&mut *tx, &a, id, RowLock::Update).await?;
    if !f.can_change {
        return Err(V2Error(DomainError::Forbidden("You do not have permission to change this folder.".into())));
    }
    if let Some(r) = expected
        && r != f.rev
    {
        return Err(V2Error(DomainError::RevMismatch { current: f.rev }));
    }
    let name = match &w.name {
        Some(n) => clean_name(n).map_err(|m| V2Error(DomainError::field("name", m)))?,
        None => f.name.clone(),
    };
    let color = match &w.color {
        Some(c) => clean_color(c.as_deref()).map_err(|m| V2Error(DomainError::field("color", m)))?,
        None => f.color.clone(),
    };
    let parent = match w.parent {
        None => f.parent_id,
        Some(None) => None,
        Some(Some(pid)) => {
            if pid != f.parent_id.unwrap_or(0) {
                check_parent(&mut tx, &a, pid).await?;
                let all: Vec<(i64, Option<i64>)> =
                    sqlx::query_as("SELECT id, parent_id FROM paperrust_folder").fetch_all(&mut *tx).await.map_err(db)?;
                let parents: HashMap<i64, Option<i64>> = all.into_iter().collect();
                if creates_cycle(&parents, f.id, pid) {
                    return Err(V2Error(DomainError::field("parent", "A folder cannot be moved into itself or one of its subfolders.")));
                }
            }
            Some(pid)
        }
    };
    let owner = match w.owner {
        None => f.owner_id.map(i64::from),
        Some(o) => {
            if !a.may_set_owner(&f) {
                return Err(V2Error(DomainError::Forbidden("Only the owner or a superuser can change the owner.".into())));
            }
            if let Some(o) = o {
                check_users(&mut tx, "owner", &[o]).await?;
            }
            o
        }
    };
    if w.set_permissions.is_some() && !a.may_set_owner(&f) {
        return Err(V2Error(DomainError::Forbidden("Only the owner or a superuser can change the permissions.".into())));
    }
    let owner_i32 = owner.map(|o| i32::try_from(o).unwrap_or(i32::MAX));
    sqlx::query(
        "UPDATE paperrust_folder SET name = $2, parent_id = $3, owner_id = $4, sort = $5, color = $6, rev = rev + 1, modified = now() \
         WHERE id = $1",
    )
    .bind(f.id)
    .bind(&name)
    .bind(parent)
    .bind(owner_i32)
    .bind(w.sort.unwrap_or(f.sort))
    .bind(&color)
    .execute(&mut *tx)
    .await
    .map_err(write_err)?;
    if let Some(ps) = &w.set_permissions {
        write_grants(&mut tx, f.id, ps).await?;
    }
    tx.commit().await.map_err(db)?;
    // the caller may have given the folder away: then it is no longer visible → 200 with the old view is wrong
    match visible_folder(pool, &a, id, RowLock::No).await {
        Ok(nf) => detail(pool, &a, &nf, StatusCode::OK).await,
        Err(V2Error(DomainError::NotFound)) => Ok(json(&json!({ "id": id, "visible": false }))),
        Err(e) => Err(e),
    }
}

/// What happens with the contents of a deleted folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contents {
    /// documents and subfolders go to the parent folder (root/"Unsorted" for a top-level folder)
    Parent,
    /// documents become "Unsorted", subfolders move to the top level
    Unsorted,
}

impl Contents {
    pub fn parse(s: Option<&str>) -> Result<Contents, &'static str> {
        match s.map(str::trim) {
            None | Some("") | Some("parent") => Ok(Contents::Parent),
            Some("unsorted") => Ok(Contents::Unsorted),
            Some(_) => Err("Expected parent or unsorted."),
        }
    }
}

/// `DELETE /folders/{id}?contents=parent|unsorted` – documents are never deleted. → 200
/// `{"moved_documents": n, "moved_folders": m}`.
pub async fn delete(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> V2Result<Response> {
    let a = access(&p)?;
    a.require_organize()?;
    let contents = Contents::parse(q.get("contents").map(String::as_str)).map_err(|m| V2Error(DomainError::field("contents", m)))?;
    let expected = if_match(&headers)?;
    let pool = state.db();
    ensure(pool).await?;
    let mut tx = pool.begin().await.map_err(db)?;
    let (user, a) = fresh_caller(&mut tx, a.user_id).await?;
    tree_lock(&mut tx).await?;
    let f = visible_folder(&mut *tx, &a, id, RowLock::Update).await?;
    if !f.can_delete {
        return Err(V2Error(DomainError::Forbidden("You do not have permission to delete this folder.".into())));
    }
    if let Some(r) = expected
        && r != f.rev
    {
        return Err(V2Error(DomainError::RevMismatch { current: f.rev }));
    }
    let target = match contents {
        Contents::Parent => f.parent_id,
        Contents::Unsorted => None,
    };
    check_contents_movable(&mut tx, &a, &user, f.id, target).await?;
    let moved_folders = sqlx::query("UPDATE paperrust_folder SET parent_id = $2, rev = rev + 1, modified = now() WHERE parent_id = $1")
        .bind(f.id)
        .bind(target)
        .execute(&mut *tx)
        .await
        .map_err(|e| match sql_state(&e).as_deref() {
            Some("23505") => {
                V2Error(DomainError::Conflict("A subfolder has the same name as a folder at the target level. Rename it first.".into()))
            }
            _ => db(e),
        })?
        .rows_affected();
    let moved_documents = match target {
        Some(t) => {
            sqlx::query("UPDATE paperrust_document_folder SET folder_id = $2, moved = now() WHERE folder_id = $1").bind(f.id).bind(t)
        }
        None => sqlx::query("DELETE FROM paperrust_document_folder WHERE folder_id = $1").bind(f.id),
    }
    .execute(&mut *tx)
    .await
    .map_err(db)?
    .rows_affected();
    sqlx::query("DELETE FROM paperrust_folder WHERE id = $1").bind(f.id).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(json(&json!({ "moved_documents": moved_documents, "moved_folders": moved_folders })))
}

pub const DELETE_FORBIDDEN_PARENT: &str = "You do not have permission to move the contents of this folder into its parent folder.";
pub const DELETE_FORBIDDEN_FOLDERS: &str = "You do not have permission to change all subfolders of this folder.";
pub const DELETE_FORBIDDEN_DOCUMENTS: &str = "You do not have permission to change all documents in this folder.";

/// Deleting folder `id` moves its contents to `target` (`None` = "Unsorted"/top level). Same rights as
/// moving them directly (`move_documents`, `patch` with a new parent): change on the target folder,
/// change on every direct subfolder, `change_document` on every document (trashed ones included, they
/// move too). Locks what it checked (see the module comment); the caller holds the tree lock and the
/// folder `FOR UPDATE`, so no new contents can arrive. 403 messages name no objects.
async fn check_contents_movable(tx: &mut sqlx::PgConnection, a: &Access, user: &User, id: i64, target: Option<i64>) -> V2Result<()> {
    let subfolders: Vec<(i64, bool)> = sqlx::query_as(&format!(
        "SELECT f.id, {} FROM paperrust_folder f WHERE f.parent_id = $3 ORDER BY f.id FOR UPDATE OF f",
        a.cond("f", BIT_CHANGE)
    ))
    .bind(a.user_id)
    .bind(&a.subjects)
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    let docs: Vec<i32> = sqlx::query_scalar("SELECT document_id FROM paperrust_document_folder WHERE folder_id = $1 ORDER BY document_id")
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
    if subfolders.is_empty() && docs.is_empty() {
        return Ok(());
    }
    if let Some(t) = target {
        match visible_folder(&mut *tx, a, t, RowLock::Share).await {
            Ok(pf) if pf.can_change => {}
            Ok(_) | Err(V2Error(DomainError::NotFound)) => return Err(V2Error(DomainError::Forbidden(DELETE_FORBIDDEN_PARENT.into()))),
            Err(e) => return Err(e),
        }
    }
    if subfolders.iter().any(|(_, can_change)| !can_change) {
        return Err(V2Error(DomainError::Forbidden(DELETE_FORBIDDEN_FOLDERS.into())));
    }
    if !user.is_superuser && !docs.is_empty() {
        let table = document_lock_table(tx).await?;
        lock_documents(tx, table, &docs).await?;
        if count_unchangeable(tx, table, user, &docs, true).await? > 0 {
            return Err(V2Error(DomainError::Forbidden(DELETE_FORBIDDEN_DOCUMENTS.into())));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveBody {
    pub documents: Vec<Ref>,
    /// target folder; `null` = "Unsorted"
    pub folder: Option<Ref>,
}

/// Distinct ids in input order.
pub fn dedup(ids: &[i64]) -> Vec<i64> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|i| seen.insert(*i)).collect()
}

async fn resolve_documents(pool: &PgPool, refs: &[Ref]) -> V2Result<Vec<i64>> {
    let uuids: Vec<Uuid> = refs.iter().filter_map(|r| if let Ref::Uuid(u) = r { Some(*u) } else { None }).collect();
    let map: HashMap<Uuid, i64> = if uuids.is_empty() {
        HashMap::new()
    } else {
        let rows: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT uuid, local_id::int8 FROM paperrust_sync_object WHERE kind = 1 AND uuid = ANY($1::uuid[]) AND deleted_at IS NULL",
        )
        .bind(&uuids)
        .fetch_all(pool)
        .await
        .map_err(|e| match sql_state(&e).as_deref() {
            Some("42P01") => V2Error(DomainError::field("documents", "Document UUIDs need the sync capture; use ids.")),
            _ => db(e),
        })?;
        rows.into_iter().collect()
    };
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        match r {
            Ref::Id(i) => out.push(*i),
            Ref::Uuid(u) => match map.get(u) {
                Some(i) => out.push(*i),
                None => return Err(V2Error(DomainError::field("documents", "Some documents don't exist or were specified twice."))),
            },
        }
    }
    Ok(out)
}

async fn resolve_folder(pool: &PgPool, r: &Ref) -> V2Result<i64> {
    match r {
        Ref::Id(i) => Ok(*i),
        Ref::Uuid(u) => sqlx::query_scalar("SELECT id FROM paperrust_folder WHERE uuid = $1")
            .bind(u)
            .fetch_optional(pool)
            .await
            .map_err(db)?
            .ok_or(V2Error(DomainError::field("folder", "Folder does not exist."))),
    }
}

/// Validation message of the upload field `folder` for an unknown or invisible folder (DRF
/// `PrimaryKeyRelatedField` wording, like the other upload fields).
pub fn upload_folder_missing(id: impl std::fmt::Display) -> String {
    format!("Invalid pk \"{id}\" - object does not exist.")
}
/// Validation message of the upload field `folder` when the uploader may not file into it.
pub const UPLOAD_FOLDER_FORBIDDEN: &str = "You do not have permission to add documents to this folder.";

/// Stage 2: upload into a folder (PaperRust-only upload field `folder` of `POST /api/documents/post_document/`,
/// pl-server `upload.rs`). The same rules as moving a document there: the uploader needs
/// `documents.view_document` + `documents.change_document` and change permission on a visible folder (the new
/// document belongs to the uploader, so the document side is always satisfied). `Ok(Err(msg))` = validation
/// error for the field; no folder tables yet = the folder does not exist (no DDL from the upload path).
pub async fn check_upload_target(pool: &PgPool, u: &User, id: i64) -> V2Result<Result<(), String>> {
    let Ok(a) = Access::of(u) else {
        return Ok(Err(UPLOAD_FOLDER_FORBIDDEN.into()));
    };
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('paperrust_folder')::text").fetch_one(pool).await.map_err(db)?;
    if exists.is_none() {
        return Ok(Err(upload_folder_missing(id)));
    }
    match visible_folder(pool, &a, id, RowLock::No).await {
        Ok(f) if f.can_change && a.can_organize => Ok(Ok(())),
        Ok(_) => Ok(Err(UPLOAD_FOLDER_FORBIDDEN.into())),
        Err(V2Error(DomainError::NotFound)) => Ok(Err(upload_folder_missing(id))),
        Err(e) => Err(e),
    }
}

/// `POST /folders/move-documents` `{documents: [id|uuid…], folder: id|uuid|null}` → 200
/// `{"moved": n, "folder": id|null}` (`moved` = rows whose folder actually changed).
pub async fn move_documents(State(state): State<AppState>, Caller(p): Caller, V2Json(b): V2Json<MoveBody>) -> V2Result<Response> {
    let a = access(&p)?;
    a.require_organize()?;
    let pool = state.db();
    ensure(pool).await?;
    if b.documents.is_empty() {
        return Err(V2Error(DomainError::field("documents", "This list may not be empty.")));
    }
    if b.documents.len() > MOVE_MAX {
        return Err(V2Error(DomainError::field("documents", format!("At most {MOVE_MAX} documents per request."))));
    }
    let ids = dedup(&resolve_documents(pool, &b.documents).await?);
    let target = match &b.folder {
        None => None,
        Some(r) => Some(resolve_folder(pool, r).await?),
    };
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    // Checks and write in one transaction, after locking what the checks depend on (caller and groups,
    // target folder FOR SHARE, then the documents FOR SHARE): a rights/share/owner change committed
    // meanwhile is either seen by the checks or waits for this move.
    let mut tx = pool.begin().await.map_err(db)?;
    let (user, a) = fresh_caller(&mut tx, a.user_id).await?;
    let folder = match target {
        None => None,
        Some(fid) => match visible_folder(&mut *tx, &a, fid, RowLock::Share).await {
            Ok(f) if f.can_change => Some(f.id),
            Ok(_) => return Err(V2Error(DomainError::Forbidden("You do not have permission to change this folder.".into()))),
            Err(V2Error(DomainError::NotFound)) => return Err(V2Error(DomainError::field("folder", "Folder does not exist."))),
            Err(e) => return Err(e),
        },
    };
    let table = document_lock_table(&mut tx).await?;
    lock_documents(&mut tx, table, &ids32).await?;
    // like bulk_edit: unknown/invisible -> 400, visible but not changeable -> 403
    let vis = pl_store::visible_ids_sql_for(&user, Kind::Document);
    let visible: i64 = sqlx::query_scalar(&format!("SELECT count(*)::int8 FROM ({vis}) v(id) WHERE v.id = ANY($1::int8[])"))
        .bind(&ids)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
    if visible != ids.len() as i64 {
        return Err(V2Error(DomainError::field("documents", "Some documents don't exist or were specified twice.")));
    }
    if count_unchangeable(&mut tx, table, &user, &ids32, false).await? > 0 {
        return Err(V2Error(DomainError::Forbidden("Insufficient permissions.".into())));
    }
    let moved = match folder {
        Some(fid) => sqlx::query(
            "INSERT INTO paperrust_document_folder (document_id, folder_id) SELECT d, $2 FROM unnest($1::int4[]) AS d \
             ON CONFLICT (document_id) DO UPDATE SET folder_id = EXCLUDED.folder_id, moved = now() \
             WHERE paperrust_document_folder.folder_id IS DISTINCT FROM EXCLUDED.folder_id",
        )
        .bind(&ids32)
        .bind(fid),
        None => sqlx::query("DELETE FROM paperrust_document_folder WHERE document_id = ANY($1::int4[])").bind(&ids32),
    }
    .execute(&mut *tx)
    .await
    .map_err(|e| match sql_state(&e).as_deref() {
        Some("23503") => V2Error(DomainError::field("folder", "Folder does not exist.")),
        _ => db(e),
    })?
    .rows_affected();
    tx.commit().await.map_err(db)?;
    Ok(json(&json!({ "moved": moved, "folder": folder })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("  Rechnungen ").unwrap(), "Rechnungen");
        assert!(clean_name("   ").is_err());
        assert!(clean_name("a\u{7}b").is_err());
        assert!(clean_name(&"x".repeat(128)).is_ok());
        assert!(clean_name(&"ä".repeat(129)).is_err());
        assert_eq!(clean_name("A/B").unwrap(), "A/B");
    }

    #[test]
    fn colors() {
        assert_eq!(clean_color(None).unwrap(), None);
        assert_eq!(clean_color(Some(" ")).unwrap(), None);
        assert_eq!(clean_color(Some("#A1B2C3")).unwrap(), Some("#a1b2c3".into()));
        assert!(clean_color(Some("#abc")).is_err());
        assert!(clean_color(Some("red")).is_err());
        assert!(clean_color(Some("#gggggg")).is_err());
    }

    fn tree() -> HashMap<i64, Option<i64>> {
        // 1 ─ 2 ─ 3, 4 (root)
        HashMap::from([(1, None), (2, Some(1)), (3, Some(2)), (4, None)])
    }

    #[test]
    fn paths() {
        let t = tree();
        assert_eq!(path_to(&t, 3), vec![1, 2, 3]);
        assert_eq!(path_to(&t, 4), vec![4]);
        // invisible parent: path starts at the first visible folder
        let mut partial = t.clone();
        partial.remove(&1);
        assert_eq!(path_to(&partial, 3), vec![2, 3]);
    }

    #[test]
    fn cycles() {
        let t = tree();
        assert!(creates_cycle(&t, 1, 3), "into own descendant");
        assert!(creates_cycle(&t, 2, 2), "into itself");
        assert!(!creates_cycle(&t, 3, 4));
        assert!(!creates_cycle(&t, 2, 4));
        assert!(!creates_cycle(&t, 4, 3));
    }

    #[test]
    fn contents_param() {
        assert_eq!(Contents::parse(None).unwrap(), Contents::Parent);
        assert_eq!(Contents::parse(Some("parent")).unwrap(), Contents::Parent);
        assert_eq!(Contents::parse(Some("unsorted")).unwrap(), Contents::Unsorted);
        assert!(Contents::parse(Some("delete")).is_err());
    }

    #[test]
    fn perm_rows_roundtrip() {
        let p = PermSet {
            view: Subjects { users: vec![3], groups: vec![2] },
            change: Subjects { users: vec![5], groups: vec![] },
            delete: Subjects { users: vec![], groups: vec![2] },
        };
        let rows = p.rows();
        assert_eq!(rows, vec![(-2, BIT_VIEW | BIT_DELETE), (3, BIT_VIEW), (5, BIT_VIEW | BIT_CHANGE)]);
        let back = PermSet::from_rows(&rows);
        assert_eq!(back.view.users, vec![3, 5]);
        assert_eq!(back.view.groups, vec![2]);
        assert_eq!(back.change.users, vec![5]);
        assert_eq!(back.delete.groups, vec![2]);
    }

    #[test]
    fn write_body_nullable_fields() {
        let w: FolderWrite = serde_json::from_value(json!({"parent": null, "color": "#aabbcc"})).unwrap();
        assert_eq!(w.parent, Some(None));
        assert_eq!(w.color, Some(Some("#aabbcc".into())));
        assert_eq!(w.owner, None);
        let w: FolderWrite = serde_json::from_value(json!({"name": "x"})).unwrap();
        assert_eq!(w.parent, None);
        assert!(serde_json::from_value::<FolderWrite>(json!({"nme": "x"})).is_err());
    }

    #[test]
    fn move_body_refs() {
        let b: MoveBody =
            serde_json::from_value(json!({"documents": [1, "8a3c1e52-4a0e-4b8e-9d3e-0c2f6e3b9a11"], "folder": null})).unwrap();
        assert_eq!(b.documents[0], Ref::Id(1));
        assert!(matches!(b.documents[1], Ref::Uuid(_)));
        assert_eq!(b.folder, None);
        assert!(serde_json::from_value::<MoveBody>(json!({"documents": ["nope"], "folder": 1})).is_err());
        assert_eq!(dedup(&[3, 1, 3, 2, 1]), vec![3, 1, 2]);
    }

    #[test]
    fn access_conditions() {
        let a = Access { user_id: 7, superuser: false, subjects: vec![7, -2], can_organize: true };
        let c = a.cond("f", BIT_CHANGE);
        assert!(c.contains("f.owner_id = $1 OR f.owner_id IS NULL"), "{c}");
        assert!(c.contains("pfg.perms & 2 <> 0"), "{c}");
        let su = Access { superuser: true, ..a };
        assert_eq!(su.cond("f", BIT_VIEW), "TRUE");
    }

    #[test]
    fn migration_is_expand_only() {
        let sql: String =
            MIGRATION.1.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n").to_ascii_uppercase();
        for bad in ["DROP TABLE", "DROP COLUMN", "ALTER TABLE DOCUMENTS_DOCUMENT", "TRUNCATE"] {
            assert!(!sql.contains(bad), "{bad}");
        }
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS PAPERRUST_FOLDER "));
    }
}
