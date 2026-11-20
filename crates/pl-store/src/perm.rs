//! Permissions: model permissions, object permissions (guardian, `owner`) → `Access`/`Sharing`. The decision
//! is made by `pl-authz` (same rules as v1); here it is only evaluated page by page.

use std::collections::{HashMap, HashSet};

use pl_authz::models::ObjectModel;
use pl_authz::User;
use pl_domain::model::{Access, GroupRef, Grantees, Kind, Sharing, UserRef};
use pl_domain::{DomainError, DomainResult};
use sqlx::PgPool;

use crate::db_err;
use crate::ident;

/// Model description per kind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spec {
    pub kind: Kind,
    pub table: &'static str,
    /// Model with owner/guardian (None = no object permissions, all visible).
    pub model: Option<ObjectModel>,
    /// Model permission for reading (`app.codename`).
    pub view_perm: &'static str,
    pub name_col: &'static str,
}

pub(crate) fn spec(kind: Kind) -> Spec {
    use pl_authz::models as m;
    let (table, model, view_perm, name_col) = match kind {
        Kind::Document => ("documents_document", Some(m::DOCUMENT), "documents.view_document", "title"),
        Kind::Tag => ("documents_tag", Some(m::TAG), "documents.view_tag", "name"),
        Kind::Correspondent => ("documents_correspondent", Some(m::CORRESPONDENT), "documents.view_correspondent", "name"),
        Kind::DocumentType => ("documents_documenttype", Some(m::DOCUMENT_TYPE), "documents.view_documenttype", "name"),
        Kind::StoragePath => ("documents_storagepath", Some(m::STORAGE_PATH), "documents.view_storagepath", "name"),
        Kind::CustomField => ("documents_customfield", None, "documents.view_customfield", "name"),
        Kind::SavedView => ("documents_savedview", Some(m::SAVED_VIEW), "documents.view_savedview", "name"),
        Kind::User => ("auth_user", None, "auth.view_user", "username"),
        Kind::Group => ("auth_group", None, "auth.view_group", "name"),
    };
    Spec { kind, table, model, view_perm, name_col }
}

/// Model permission like v1 (`PaperlessObjectPermissions` or `IsAuthenticated + DjangoModelPermissions`).
pub(crate) fn require(user: &User, perm: &str) -> DomainResult<()> {
    if user.is_active && user.has_perm(perm) {
        Ok(())
    } else {
        Err(DomainError::Forbidden("You do not have permission to perform this action.".into()))
    }
}

/// Object permissions (bare codenames) of the user on `ids` (direct + via groups).
pub(crate) async fn grants(
    db: &PgPool,
    user: &User,
    model: &ObjectModel,
    ids: &[i64],
    actions: &[&str],
) -> DomainResult<HashMap<i64, HashSet<String>>> {
    let mut out: HashMap<i64, HashSet<String>> = HashMap::new();
    if ids.is_empty() || user.is_superuser {
        return Ok(out);
    }
    let pks: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    let codes: Vec<String> = actions.iter().map(|a| model.codename(a)).collect();
    let sql = if pl_authz::grants::use_grants() {
        // L5: Grant index (same filters: row content type, codename).
        "WITH ct AS (SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2)
         SELECT g.object_id::text, p.codename FROM paperrust_grant g
           JOIN auth_permission p ON p.id = g.perm_id
          WHERE g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug WHERE ug.user_id = $3::int4)
                                || ($3)::int8)
            AND g.ct IN (SELECT id FROM ct)
            AND g.object_id = ANY($4::text[]::int8[]) AND p.codename = ANY($5::text[])"
    } else {
        LEGACY_GRANTS_SQL
    };
    let rows: Vec<(String, String)> = sqlx::query_as(sql)
    .bind(model.app_label)
    .bind(model.model_name)
    .bind(user.id)
    .bind(&pks)
    .bind(&codes)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    for (pk, code) in rows {
        if let Ok(id) = pk.parse::<i64>() {
            out.entry(id).or_default().insert(code);
        }
    }
    Ok(out)
}

const LEGACY_GRANTS_SQL: &str =
        "WITH ct AS (SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2)
         SELECT g.object_pk, p.codename FROM guardian_userobjectpermission g
           JOIN auth_permission p ON p.id = g.permission_id
          WHERE g.user_id = $3::int4 AND g.content_type_id IN (SELECT id FROM ct)
            AND g.object_pk = ANY($4::text[]) AND p.codename = ANY($5::text[])
         UNION ALL
         SELECT g.object_pk, p.codename FROM guardian_groupobjectpermission g
           JOIN auth_permission p ON p.id = g.permission_id
          WHERE g.group_id IN (SELECT group_id FROM auth_user_groups WHERE user_id = $3::int4)
            AND g.content_type_id IN (SELECT id FROM ct)
            AND g.object_pk = ANY($4::text[]) AND p.codename = ANY($5::text[])";

/// `Access` of a visible object: change/delete = model permission **and** (superuser,
/// without owner, own object or guardian permission) – like `check_object_perms` in v1.
pub(crate) fn access(
    user: &User,
    model: &ObjectModel,
    id: i64,
    owner_id: Option<i64>,
    grants: &HashMap<i64, HashSet<String>>,
) -> Access {
    let obj = |action: &str| -> bool {
        if !user.has_perm(&model.perm(action)) {
            return false;
        }
        if user.is_superuser || owner_id.is_none() || owner_id == Some(user.id) {
            return true;
        }
        grants.get(&id).is_some_and(|s| s.contains(&model.codename(action)))
    };
    Access { can_view: true, can_change: obj("change"), can_delete: obj("delete") }
}

/// Access for kinds without object permissions (custom fields, users, groups).
pub(crate) fn model_access(user: &User, app: &str, model: &str) -> Access {
    Access {
        can_view: true,
        can_change: user.has_perm(&format!("{app}.change_{model}")),
        can_delete: user.has_perm(&format!("{app}.delete_{model}")),
    }
}

/// User references (UUID + name).
pub(crate) async fn user_refs(db: &PgPool, ids: &[i64]) -> DomainResult<HashMap<i64, UserRef>> {
    let mut ids: Vec<i64> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let idents = ident::idents(db, Kind::User, &ids).await?;
    let rows: Vec<(i32, String)> = sqlx::query_as("SELECT id, username FROM auth_user WHERE id = ANY($1::int4[])")
        .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, username)| {
            let id = i64::from(id);
            idents.get(&id).map(|i| (id, UserRef { uuid: i.uuid, username }))
        })
        .collect())
}

pub(crate) async fn group_refs(db: &PgPool, ids: &[i64]) -> DomainResult<HashMap<i64, GroupRef>> {
    let mut ids: Vec<i64> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let idents = ident::idents(db, Kind::Group, &ids).await?;
    let rows: Vec<(i32, String)> = sqlx::query_as("SELECT id, name FROM auth_group WHERE id = ANY($1::int4[])")
        .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, name)| {
            let id = i64::from(id);
            idents.get(&id).map(|i| (id, GroupRef { uuid: i.uuid, name }))
        })
        .collect())
}

/// Shares (`Sharing`) for the objects the user owns (or all for superusers) –
/// like v1 `permissions` visible only with `full_perms` for owners.
pub(crate) async fn sharing(
    db: &PgPool,
    user: &User,
    model: &ObjectModel,
    objs: &[(i64, Option<i64>)],
) -> DomainResult<HashMap<i64, Sharing>> {
    let pks: Vec<i64> = objs
        .iter()
        .filter(|(_, owner)| user.is_superuser || *owner == Some(user.id))
        .map(|(id, _)| *id)
        .collect();
    if pks.is_empty() {
        return Ok(HashMap::new());
    }
    let g = pl_authz::owned::get_permission_grantees(db, model, &pks)
        .await
        .map_err(|e| DomainError::Internal(e.detail()))?;
    let mut uids = Vec::new();
    let mut gids = Vec::new();
    for og in g.values() {
        uids.extend(og.view.users.iter().chain(og.change.users.iter()).copied());
        gids.extend(og.view.groups.iter().chain(og.change.groups.iter()).copied());
    }
    let users = user_refs(db, &uids).await?;
    let groups = group_refs(db, &gids).await?;
    let conv = |gr: &pl_authz::owned::Grantees| Grantees {
        users: gr.users.iter().filter_map(|u| users.get(u).cloned()).collect(),
        groups: gr.groups.iter().filter_map(|x| groups.get(x).cloned()).collect(),
    };
    Ok(g.iter().map(|(id, og)| (*id, Sharing { view: conv(&og.view), change: conv(&og.change) })).collect())
}
