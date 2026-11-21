//! Permissions: Django ModelBackend (global permissions), guardian (object permissions), visibility,
//! `permissions`/`user_can_change`, `set_permissions`.
//!
//! upstream: src/documents/permissions.py@v3.4.2 (PaperlessObjectPermissions, permitted_object_ids,
//! has_perms_owner_aware, set_permissions_for_objects, get_permission_grantees),
//! src/documents/serialisers.py@v3.4.2 (OwnedObjectSerializer.get_user_can_change,
//! SetPermissionsMixin, SetPermissionsSerializer), guardian.core.ObjectPermissionChecker.
//! Local until pl-authz publishes these helpers (CORE-REQUEST).

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::http::Method;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Transaction};

use crate::ctx::MailUser;
use crate::drf::{self, FieldError};
use crate::error::{ApiError, ApiResult};
use crate::pyval::{self, PyInt};

pub const APP_LABEL: &str = "paperless_mail";

/// Model metadata for permission checks.
#[derive(Debug, Clone, Copy)]
pub struct Model {
    /// Django `model_name` (lower case), e.g. `mailaccount`.
    pub name: &'static str,
    /// Django `object_name`, e.g. `MailAccount` (404 text).
    pub object_name: &'static str,
    pub table: &'static str,
}

pub const MAIL_ACCOUNT: Model = Model { name: "mailaccount", object_name: "MailAccount", table: "paperless_mail_mailaccount" };
pub const MAIL_RULE: Model = Model { name: "mailrule", object_name: "MailRule", table: "paperless_mail_mailrule" };
pub const PROCESSED_MAIL: Model = Model { name: "processedmail", object_name: "ProcessedMail", table: "paperless_mail_processedmail" };

/// `django_content_type.id` for a model of this app.
pub async fn content_type_id(db: &PgPool, model: Model) -> ApiResult<i32> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2")
        .bind(APP_LABEL)
        .bind(model.name)
        .fetch_optional(db)
        .await?;
    row.map(|r| r.0).ok_or_else(|| ApiError::internal(format!("content type {} missing", model.name)))
}

/// `user.has_perm("paperless_mail.<codename>")` (ModelBackend).
pub async fn has_global_perm(db: &PgPool, user: &MailUser, codename: &str) -> ApiResult<bool> {
    if !user.is_active {
        return Ok(false);
    }
    if user.is_superuser {
        return Ok(true);
    }
    if let Some(p) = &user.perms {
        return Ok(p.contains(&format!("{APP_LABEL}.{codename}")));
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM auth_permission p \
         JOIN django_content_type ct ON ct.id = p.content_type_id \
         WHERE ct.app_label = $1 AND p.codename = $2 AND ( \
           p.id IN (SELECT up.permission_id FROM auth_user_user_permissions up WHERE up.user_id = $3) \
           OR p.id IN (SELECT gp.permission_id FROM auth_group_permissions gp \
                       JOIN auth_user_groups ug ON ug.group_id = gp.group_id WHERE ug.user_id = $3)) \
         LIMIT 1",
    )
    .bind(APP_LABEL)
    .bind(codename)
    .bind(user.id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// DRF `perms_map` of `PaperlessObjectPermissions`.
pub fn action_for_method(method: &Method) -> &'static str {
    match *method {
        Method::POST => "add",
        Method::PUT | Method::PATCH => "change",
        Method::DELETE => "delete",
        _ => "view",
    }
}

/// `PaperlessObjectPermissions.has_permission` → 403 on missing global permission.
pub async fn require_model_perm(db: &PgPool, user: &MailUser, method: &Method, model: Model) -> ApiResult<()> {
    let codename = format!("{}_{}", action_for_method(method), model.name);
    if has_global_perm(db, user, &codename).await? { Ok(()) } else { Err(ApiError::forbidden()) }
}

/// guardian `ObjectPermissionChecker(user).has_perm(codename, obj)`.
pub async fn guardian_has_perm(db: &PgPool, user: &MailUser, codename: &str, ct: i32, pk: i32) -> ApiResult<bool> {
    if !user.is_active {
        return Ok(false);
    }
    if user.is_superuser {
        return Ok(true);
    }
    if pl_authz::grants::use_grants() {
        // L5: share index (row content type = model like guardian's checker).
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM auth_permission p JOIN paperrust_grant g ON g.perm_id = p.id               WHERE p.content_type_id = $1 AND p.codename = $2 AND g.ct = $1 AND g.object_id = $4                 AND g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug                                           WHERE ug.user_id = $3) || ($3)::int8) LIMIT 1",
        )
        .bind(ct)
        .bind(codename)
        .bind(user.id)
        .bind(i64::from(pk))
        .fetch_optional(db)
        .await?;
        return Ok(row.is_some());
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM auth_permission p WHERE p.content_type_id = $1 AND p.codename = $2 AND ( \
           EXISTS (SELECT 1 FROM guardian_userobjectpermission u WHERE u.permission_id = p.id \
                   AND u.user_id = $3 AND u.content_type_id = $1 AND u.object_pk = $4) \
           OR EXISTS (SELECT 1 FROM guardian_groupobjectpermission g \
                   JOIN auth_user_groups ug ON ug.group_id = g.group_id \
                   WHERE g.permission_id = p.id AND ug.user_id = $3 \
                   AND g.content_type_id = $1 AND g.object_pk = $4)) LIMIT 1",
    )
    .bind(ct)
    .bind(codename)
    .bind(user.id)
    .bind(pk.to_string())
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// `has_perms_owner_aware(user, codename, obj)`.
pub async fn has_perms_owner_aware(db: &PgPool, user: &MailUser, codename: &str, ct: i32, pk: i32, owner: Option<i32>) -> ApiResult<bool> {
    match owner {
        None => Ok(true),
        Some(o) if o == user.id => Ok(true),
        Some(_) => guardian_has_perm(db, user, codename, ct, pk).await,
    }
}

/// `PaperlessObjectPermissions.has_object_permission` incl. DRF 404 rule.
pub async fn check_object_permission(
    db: &PgPool,
    user: &MailUser,
    method: &Method,
    model: Model,
    ct: i32,
    pk: i32,
    owner: Option<i32>,
) -> ApiResult<()> {
    match owner {
        None => return Ok(()),
        Some(o) if o == user.id => return Ok(()),
        _ => {}
    }
    let action = action_for_method(method);
    if guardian_has_perm(db, user, &format!("{action}_{}", model.name), ct, pk).await? {
        return Ok(());
    }
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if safe {
        return Err(ApiError::not_found());
    }
    if !guardian_has_perm(db, user, &format!("view_{}", model.name), ct, pk).await? {
        return Err(ApiError::not_found());
    }
    Err(ApiError::forbidden())
}

/// Visibility condition (`permitted_object_ids`) as SQL fragment over `<alias>.id`/`<alias>.owner_id`.
/// `None` = no restriction (active superuser). Placeholders from `$first`.
/// Returns: (SQL, bind values in order: user_id, codename, ct).
pub fn visible_sql(user: &MailUser, alias: &str, first: usize) -> Option<String> {
    if !user.is_active {
        return Some("FALSE".to_string());
    }
    if user.is_superuser {
        return None;
    }
    let (u, c, t) = (first, first + 1, first + 2);
    if pl_authz::grants::use_grants() {
        // L5: same set via the share index (bigint instead of text comparison).
        return Some(format!(
            "{alias}.id IN (SELECT v.id FROM {{table}} v WHERE v.owner_id = ${u} OR v.owner_id IS NULL              OR v.id IN (                SELECT g.object_id FROM paperrust_grant g                 WHERE g.perm_id = ANY(ARRAY(SELECT p.id FROM auth_permission p                                             WHERE p.codename = ${c} AND p.content_type_id = ${t}))                   AND g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug                                             WHERE ug.user_id = ${u}) || (${u})::int8)))"
        ));
    }
    Some(format!(
        "{alias}.id IN (SELECT v.id FROM {{table}} v WHERE v.owner_id = ${u} OR v.owner_id IS NULL \
         OR CAST(v.id AS TEXT) IN ( \
           SELECT uop.object_pk FROM guardian_userobjectpermission uop \
             JOIN auth_permission p ON p.id = uop.permission_id \
             WHERE uop.user_id = ${u} AND p.codename = ${c} AND p.content_type_id = ${t} \
           UNION ALL \
           SELECT gop.object_pk FROM guardian_groupobjectpermission gop \
             JOIN auth_permission p ON p.id = gop.permission_id \
             WHERE gop.group_id IN (SELECT ug.group_id FROM auth_user_groups ug WHERE ug.user_id = ${u}) \
               AND p.codename = ${c} AND p.content_type_id = ${t}))"
    ))
}

/// List of the IDs from `ids` visible for `codename` (for bulk_delete / AccountField).
pub async fn permitted_among(db: &PgPool, user: &MailUser, model: Model, codename: &str, ids: &[i32]) -> ApiResult<HashSet<i32>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let ct = content_type_id(db, model).await?;
    let base = format!("SELECT t.id FROM {} t WHERE t.id = ANY($1)", model.table);
    let rows: Vec<(i32,)> = match visible_sql(user, "t", 2) {
        None => sqlx::query_as(&base).bind(ids).fetch_all(db).await?,
        Some(cond) => {
            let cond = cond.replace("{table}", model.table);
            let sql = format!("{base} AND {cond}");
            sqlx::query_as(&sql).bind(ids).bind(user.id).bind(codename).bind(ct).fetch_all(db).await?
        }
    };
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// `get_permission_grantees(model, pks)` → `{"view":{"users":[],"groups":[]},"change":{...}}`.
pub async fn grantees(db: &PgPool, model: Model, ct: i32, pks: &[i32]) -> ApiResult<HashMap<i32, Value>> {
    type UsersGroups = (Vec<i32>, Vec<i32>);
    let mut acc: HashMap<i32, BTreeMap<&'static str, UsersGroups>> = HashMap::new();
    for pk in pks {
        let mut m = BTreeMap::new();
        m.insert("view", (Vec::new(), Vec::new()));
        m.insert("change", (Vec::new(), Vec::new()));
        acc.insert(*pk, m);
    }
    if !pks.is_empty() {
        let view = format!("view_{}", model.name);
        let change = format!("change_{}", model.name);
        let pk_strs: Vec<String> = pks.iter().map(|p| p.to_string()).collect();
        let grants = pl_authz::grants::use_grants();
        let pk_ids: Vec<i64> = pks.iter().map(|p| i64::from(*p)).collect();
        for (table, col, is_user) in
            [("guardian_userobjectpermission", "user_id", true), ("guardian_groupobjectpermission", "group_id", false)]
        {
            let sql = format!(
                "SELECT x.object_pk, p.codename, x.{col} FROM {table} x \
                 JOIN auth_permission p ON p.id = x.permission_id \
                 WHERE x.content_type_id = $1 AND p.content_type_id = $1 \
                   AND p.codename = ANY($2) AND x.object_pk = ANY($3) ORDER BY x.{col}"
            );
            let rows: Vec<(String, String, i32)> = if grants {
                // L5: share index, users (subject > 0) or groups (subject < 0).
                let sign = if is_user { "> 0" } else { "< 0" };
                let sql = format!(
                    "SELECT x.object_id::text, p.codename, abs(x.subject)::int4 FROM paperrust_grant x                      JOIN auth_permission p ON p.id = x.perm_id                      WHERE x.ct = $1 AND p.content_type_id = $1 AND x.subject {sign}                        AND p.codename = ANY($2) AND x.object_id = ANY($3) ORDER BY abs(x.subject)"
                );
                sqlx::query_as(&sql).bind(ct).bind(vec![view.clone(), change.clone()]).bind(&pk_ids).fetch_all(db).await?
            } else {
                sqlx::query_as(&sql).bind(ct).bind(vec![view.clone(), change.clone()]).bind(&pk_strs).fetch_all(db).await?
            };
            for (opk, codename, ident) in rows {
                let Ok(pk) = opk.parse::<i32>() else { continue };
                let action = if codename == view { "view" } else { "change" };
                if let Some(entry) = acc.get_mut(&pk).and_then(|m| m.get_mut(action)) {
                    if is_user {
                        entry.0.push(ident);
                    } else {
                        entry.1.push(ident);
                    }
                }
            }
        }
    }
    Ok(acc
        .into_iter()
        .map(|(pk, m)| {
            let v = |k: &str| {
                let (u, g) = &m[k];
                json!({ "users": u, "groups": g })
            };
            (pk, json!({ "view": v("view"), "change": v("change") }))
        })
        .collect())
}

/// `user_can_change` for several objects (pk, owner). Same semantics as the guardian checker.
pub async fn user_can_change(
    db: &PgPool,
    user: &MailUser,
    model: Model,
    ct: i32,
    objs: &[(i32, Option<i32>)],
) -> ApiResult<HashMap<i32, bool>> {
    let mut out = HashMap::new();
    let mut need = Vec::new();
    for (pk, owner) in objs {
        match owner {
            None => {
                out.insert(*pk, true);
            }
            Some(o) if *o == user.id => {
                out.insert(*pk, true);
            }
            _ if user.is_active_superuser() => {
                out.insert(*pk, true);
            }
            _ if !user.is_active => {
                out.insert(*pk, false);
            }
            _ => need.push(pk.to_string()),
        }
    }
    if need.is_empty() {
        return Ok(out);
    }
    let codename = format!("change_{}", model.name);
    if pl_authz::grants::use_grants() {
        let ids: Vec<i64> = need.iter().filter_map(|s| s.parse::<i64>().ok()).collect();
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT g.object_id::text FROM paperrust_grant g JOIN auth_permission p ON p.id = g.perm_id               WHERE g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug                                           WHERE ug.user_id = $1) || ($1)::int8)                 AND g.ct = $2 AND p.content_type_id = $2 AND p.codename = $3 AND g.object_id = ANY($4)",
        )
        .bind(user.id)
        .bind(ct)
        .bind(&codename)
        .bind(&ids)
        .fetch_all(db)
        .await?;
        let granted: HashSet<String> = rows.into_iter().map(|r| r.0).collect();
        for s in need {
            if let Ok(pk) = s.parse::<i32>() {
                out.insert(pk, granted.contains(&s));
            }
        }
        return Ok(out);
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT u.object_pk FROM guardian_userobjectpermission u \
           JOIN auth_permission p ON p.id = u.permission_id \
           WHERE u.user_id = $1 AND u.content_type_id = $2 AND p.content_type_id = $2 \
             AND p.codename = $3 AND u.object_pk = ANY($4) \
         UNION \
         SELECT g.object_pk FROM guardian_groupobjectpermission g \
           JOIN auth_permission p ON p.id = g.permission_id \
           JOIN auth_user_groups ug ON ug.group_id = g.group_id \
           WHERE ug.user_id = $1 AND g.content_type_id = $2 AND p.content_type_id = $2 \
             AND p.codename = $3 AND g.object_pk = ANY($4)",
    )
    .bind(user.id)
    .bind(ct)
    .bind(&codename)
    .bind(&need)
    .fetch_all(db)
    .await?;
    let granted: HashSet<String> = rows.into_iter().map(|r| r.0).collect();
    for s in need {
        if let Ok(pk) = s.parse::<i32>() {
            out.insert(pk, granted.contains(&s));
        }
    }
    Ok(out)
}

/// Inserts `user_can_change` or `permissions` after `owner` (serializer order).
pub struct PermFields {
    pub full_perms: bool,
    pub can_change: HashMap<i32, bool>,
    pub grantees: HashMap<i32, Value>,
}

impl PermFields {
    pub async fn load(db: &PgPool, user: &MailUser, model: Model, full_perms: bool, objs: &[(i32, Option<i32>)]) -> ApiResult<Self> {
        let ct = content_type_id(db, model).await?;
        let pks: Vec<i32> = objs.iter().map(|o| o.0).collect();
        if full_perms {
            Ok(PermFields { full_perms, can_change: HashMap::new(), grantees: grantees(db, model, ct, &pks).await? })
        } else {
            Ok(PermFields { full_perms, can_change: user_can_change(db, user, model, ct, objs).await?, grantees: HashMap::new() })
        }
    }

    pub fn insert(&self, map: &mut Map<String, Value>, pk: i32) {
        if self.full_perms {
            let empty = json!({"view":{"users":[],"groups":[]},"change":{"users":[],"groups":[]}});
            map.insert("permissions".into(), self.grantees.get(&pk).cloned().unwrap_or(empty));
        } else {
            map.insert("user_can_change".into(), Value::Bool(self.can_change.get(&pk).copied().unwrap_or(false)));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// set_permissions

/// Entry `users`/`groups`: missing (`None`) or list (`null` ≙ empty list).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PermSet {
    pub users: Option<Vec<i128>>,
    pub groups: Option<Vec<i128>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetPermissions {
    pub view: Option<PermSet>,
    pub change: Option<PermSet>,
}

fn parse_int_list(v: &Value) -> Result<Option<Vec<i128>>, FieldError> {
    match v {
        Value::Null => Ok(None),
        Value::Array(items) => {
            let mut errs = Map::new();
            let mut out = Vec::new();
            for (i, it) in items.iter().enumerate() {
                if it.is_null() {
                    errs.insert(i.to_string(), drf::err(drf::MSG_NULL));
                    continue;
                }
                match drf::integer_field(it, None, None) {
                    Ok(n) => out.push(n),
                    Err(e) => {
                        errs.insert(i.to_string(), e);
                    }
                }
            }
            if errs.is_empty() { Ok(Some(out)) } else { Err(Value::Object(errs)) }
        }
        other => Err(drf::err(format!("Expected a list of items but got type \"{}\".", pyval::type_name(other)))),
    }
}

fn parse_perm_set(v: &Value) -> Result<PermSet, FieldError> {
    let Value::Object(o) = v else {
        return Err(drf::non_dict_error(v));
    };
    let mut errs = Map::new();
    let mut ps = PermSet::default();
    for key in ["users", "groups"] {
        if let Some(val) = o.get(key) {
            match parse_int_list(val) {
                Ok(list) => {
                    let list = Some(list.unwrap_or_default());
                    if key == "users" {
                        ps.users = list;
                    } else {
                        ps.groups = list;
                    }
                }
                Err(e) => {
                    errs.insert(key.into(), e);
                }
            }
        }
    }
    if errs.is_empty() { Ok(ps) } else { Err(Value::Object(errs)) }
}

/// `SetPermissionsSerializer.to_internal_value` (without DB check).
pub fn parse_set_permissions(v: &Value) -> Result<SetPermissions, FieldError> {
    let Value::Object(o) = v else {
        return Err(drf::non_dict_error(v));
    };
    let mut unknown: Vec<&String> = o.keys().filter(|k| *k != "view" && *k != "change").collect();
    if !unknown.is_empty() {
        unknown.sort();
        let mut m = Map::new();
        for k in unknown {
            m.insert(k.clone(), Value::String("Unknown permission action.".into()));
        }
        return Err(Value::Object(m));
    }
    let mut errs = Map::new();
    let mut sp = SetPermissions::default();
    for key in ["view", "change"] {
        match o.get(key) {
            None => {}
            Some(Value::Null) => {
                errs.insert(key.into(), drf::err(drf::MSG_NULL));
            }
            Some(val) => match parse_perm_set(val) {
                Ok(ps) => {
                    if key == "view" {
                        sp.view = Some(ps);
                    } else {
                        sp.change = Some(ps);
                    }
                }
                Err(e) => {
                    errs.insert(key.into(), e);
                }
            },
        }
    }
    if errs.is_empty() { Ok(sp) } else { Err(Value::Object(errs)) }
}

/// Validated IDs (all existing, without duplicates).
#[derive(Debug, Clone, Default)]
pub struct ValidPermSet {
    pub users: Option<Vec<i32>>,
    pub groups: Option<Vec<i32>>,
}

#[derive(Debug, Clone, Default)]
pub struct ValidSetPermissions {
    pub view: Option<ValidPermSet>,
    pub change: Option<ValidPermSet>,
}

async fn validate_ids(db: &PgPool, table: &str, ids: &[i128], msg: &str) -> Result<Vec<i32>, FieldError> {
    let in_range: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let sql = format!("SELECT id FROM {table} WHERE id = ANY($1) ORDER BY id");
    let rows: Vec<(i32,)> = sqlx::query_as(&sql).bind(&in_range).fetch_all(db).await.map_err(|e| {
        tracing::error!(target: "pl_mail", "set_permissions lookup failed: {e}");
        drf::err(msg)
    })?;
    if rows.len() != ids.len() {
        return Err(drf::err(msg));
    }
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// `SetPermissionsMixin.validate_set_permissions`.
pub async fn validate_set_permissions(db: &PgPool, sp: &SetPermissions) -> Result<ValidSetPermissions, FieldError> {
    const U: &str = "Some users in don't exist or were specified twice.";
    const G: &str = "Some groups in don't exist or were specified twice.";
    let mut out = ValidSetPermissions::default();
    for (src, dst) in [(&sp.view, &mut out.view), (&sp.change, &mut out.change)] {
        if let Some(ps) = src {
            let mut v = ValidPermSet::default();
            if let Some(u) = &ps.users {
                v.users = Some(validate_ids(db, "auth_user", u, U).await?);
            }
            if let Some(g) = &ps.groups {
                v.groups = Some(validate_ids(db, "auth_group", g, G).await?);
            }
            *dst = Some(v);
        }
    }
    Ok(out)
}

/// `set_permissions_for_objects(permissions, model, [pk], merge=False)`.
pub async fn apply_set_permissions(tx: &mut Transaction<'_, Postgres>, model: Model, pk: i32, sp: &ValidSetPermissions) -> ApiResult<()> {
    let ct: (i32,) = sqlx::query_as("SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2")
        .bind(APP_LABEL)
        .bind(model.name)
        .fetch_one(&mut **tx)
        .await?;
    let ct = ct.0;
    let object_pk = pk.to_string();
    for (action, entry) in [("view", &sp.view), ("change", &sp.change)] {
        let Some(entry) = entry else { continue };
        if entry.users.is_none() && entry.groups.is_none() {
            continue;
        }
        let codename = format!("{action}_{}", model.name);
        let mut implied = vec![codename.clone()];
        if action == "change" {
            implied.push(format!("view_{}", model.name));
        }
        let perm_ids: Vec<(i32,)> = sqlx::query_as("SELECT id FROM auth_permission WHERE content_type_id = $1 AND codename = ANY($2)")
            .bind(ct)
            .bind(&implied)
            .fetch_all(&mut **tx)
            .await?;
        if perm_ids.len() != implied.len() {
            return Err(ApiError::internal(format!("permission rows missing for {codename}")));
        }
        for (table, col, ids) in
            [("guardian_userobjectpermission", "user_id", &entry.users), ("guardian_groupobjectpermission", "group_id", &entry.groups)]
        {
            let Some(ids) = ids else { continue };
            let del = format!(
                "DELETE FROM {table} WHERE content_type_id = $1 AND object_pk = $2 \
                 AND permission_id IN (SELECT id FROM auth_permission WHERE codename = $3) \
                 AND NOT ({col} = ANY($4))"
            );
            sqlx::query(&del).bind(ct).bind(&object_pk).bind(&codename).bind(ids).execute(&mut **tx).await?;
            if ids.is_empty() {
                continue;
            }
            let ins = format!(
                "INSERT INTO {table} (content_type_id, object_pk, permission_id, {col}) \
                 SELECT $1, $2, p, i FROM UNNEST($3::int[]) AS p CROSS JOIN UNNEST($4::int[]) AS i \
                 ON CONFLICT DO NOTHING"
            );
            let pids: Vec<i32> = perm_ids.iter().map(|r| r.0).collect();
            sqlx::query(&ins).bind(ct).bind(&object_pk).bind(&pids).bind(ids).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

/// `PrimaryKeyRelatedField(queryset=User.objects.all())` for `owner`.
pub async fn resolve_user_pk(db: &PgPool, v: &Value) -> Result<i32, FieldError> {
    resolve_pk(db, "auth_user", v).await
}

/// Generic PK lookup against a table without restriction.
pub async fn resolve_pk(db: &PgPool, table: &str, v: &Value) -> Result<i32, FieldError> {
    match drf::pk_lookup(v)? {
        drf::PkLookup::OutOfRange => Err(drf::pk_does_not_exist(v)),
        drf::PkLookup::Id(id) => {
            let sql = format!("SELECT id FROM {table} WHERE id = $1");
            let row: Option<(i32,)> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await.map_err(|_| drf::pk_does_not_exist(v))?;
            row.map(|r| r.0).ok_or_else(|| drf::pk_does_not_exist(v))
        }
    }
}

/// `ManyRelatedField.to_internal_value` with a `PrimaryKeyRelatedField` child: the same errors in
/// the same order as the single queries (first faulty entry wins), but only one
/// query (review 2 N29).
pub async fn resolve_pks(db: &PgPool, table: &str, items: &[Value]) -> Result<Vec<i32>, FieldError> {
    let lookups: Vec<Result<drf::PkLookup, FieldError>> =
        items.iter().map(|it| if it.is_null() { Err(drf::pk_does_not_exist(it)) } else { drf::pk_lookup(it) }).collect();
    let candidates: Vec<i32> = lookups
        .iter()
        .map_while(|l| l.as_ref().ok())
        .filter_map(|l| match l {
            drf::PkLookup::Id(i) => Some(*i),
            drf::PkLookup::OutOfRange => None,
        })
        .collect();
    let existing: std::collections::HashSet<i32> = if candidates.is_empty() {
        Default::default()
    } else {
        let sql = format!("SELECT id FROM {table} WHERE id = ANY($1)");
        let rows: Vec<(i32,)> =
            sqlx::query_as(&sql).bind(&candidates).fetch_all(db).await.map_err(|_| drf::pk_does_not_exist(&items[0]))?;
        rows.into_iter().map(|r| r.0).collect()
    };
    resolve_in_order(items, lookups, &existing)
}

/// Pure part of [`resolve_pks`]: results in input order, first error aborts.
fn resolve_in_order(
    items: &[Value],
    lookups: Vec<Result<drf::PkLookup, FieldError>>,
    existing: &std::collections::HashSet<i32>,
) -> Result<Vec<i32>, FieldError> {
    let mut out = Vec::with_capacity(items.len());
    for (it, l) in items.iter().zip(lookups) {
        match l? {
            drf::PkLookup::Id(id) if existing.contains(&id) => out.push(id),
            _ => return Err(drf::pk_does_not_exist(it)),
        }
    }
    Ok(out)
}

/// Python `int(x)` → i32 for `Model.objects.get(pk=x)`; `None` = TypeError/ValueError/out of range.
pub fn pk_from_raw(v: &Value) -> Option<i32> {
    match pyval::py_int(v) {
        PyInt::Ok(i) => i32::try_from(i).ok(),
        PyInt::Invalid => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 2 N29: batch resolution reports the first faulty entry like the single queries.
    #[test]
    fn resolve_in_order_reports_first_failure() {
        use serde_json::json;
        let existing: std::collections::HashSet<i32> = [1, 2, 3].into_iter().collect();
        let run = |items: Vec<Value>| {
            let l = items.iter().map(|it| if it.is_null() { Err(drf::pk_does_not_exist(it)) } else { drf::pk_lookup(it) }).collect();
            resolve_in_order(&items, l, &existing)
        };
        assert_eq!(run(vec![json!(3), json!("1")]).unwrap(), vec![3, 1]);
        assert_eq!(run(vec![json!(1), json!(9), json!("x")]).unwrap_err(), json!(["Invalid pk \"9\" - object does not exist."]));
        assert_eq!(run(vec![json!(1), json!("x"), json!(9)]).unwrap_err(), json!(["Incorrect type. Expected pk value, received str."]));
        assert_eq!(run(vec![json!(null)]).unwrap_err(), json!(["Invalid pk \"None\" - object does not exist."]));
    }
    use serde_json::json;

    #[test]
    fn set_permissions_parsing() {
        assert_eq!(
            parse_set_permissions(&json!({"foo": {}, "bar": 1})).unwrap_err(),
            json!({"bar": "Unknown permission action.", "foo": "Unknown permission action."})
        );
        assert_eq!(
            parse_set_permissions(&json!([1])).unwrap_err(),
            json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]})
        );
        assert_eq!(
            parse_set_permissions(&json!({"view": {"users": ["x", 2]}})).unwrap_err(),
            json!({"view": {"users": {"0": ["A valid integer is required."]}}})
        );
        assert_eq!(
            parse_set_permissions(&json!({"view": {"users": "1"}})).unwrap_err(),
            json!({"view": {"users": ["Expected a list of items but got type \"str\"."]}})
        );
        let sp = parse_set_permissions(&json!({"change": {"users": null, "groups": [3]}})).unwrap();
        assert_eq!(sp.view, None);
        assert_eq!(sp.change, Some(PermSet { users: Some(vec![]), groups: Some(vec![3]) }));
    }

    #[test]
    fn visibility_sql_shapes() {
        let su = MailUser { id: 1, is_active: true, is_superuser: true, perms: None };
        assert!(visible_sql(&su, "t", 1).is_none());
        let inactive = MailUser { id: 1, is_active: false, is_superuser: true, perms: None };
        assert_eq!(visible_sql(&inactive, "t", 1).as_deref(), Some("FALSE"));
        let u = MailUser { id: 2, is_active: true, is_superuser: false, perms: None };
        let s = visible_sql(&u, "t", 3).unwrap();
        assert!(s.contains("$3") && s.contains("$4") && s.contains("$5"));
    }
}
