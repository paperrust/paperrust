// upstream: src/documents/permissions.py@v3.4.2 (permitted_object_ids, permitted_document_ids,
//           annotate_authorizing_fields, set_permissions_for_objects, _apply_bulk_permission_entry)
//           src/documents/views.py@v3.4.2 (DocumentOperationPermissionMixin._has_document_permissions)
//
//! Document object permissions. `permitted_document_ids` comes from pl-authz; `set_permissions_for_objects`
//! is local here (CORE-REQUEST to pl-authz, if wanted there jointly).

use sea_query::{Alias, Expr, JoinType, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use pl_core::sqlbind::SqlxBinder;
use sqlx::PgConnection;

use crate::error::ApiError;
use crate::user::{CurrentUser, document_content_type_id, has_perm};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// `authorizing_owner` for table alias `d` with LEFT JOIN `r` (root).
fn authorizing_owner() -> SimpleExpr {
    Expr::case(
        Expr::col((a("d"), a("root_document_id"))).is_null(),
        Expr::col((a("d"), a("owner_id"))),
    )
    .finally(Expr::col((a("r"), a("owner_id"))))
    .into()
}

/// `FROM documents_document d LEFT JOIN documents_document r ON r.id = d.root_document_id`
fn base_select() -> SelectStatement {
    Query::select()
        .from_as(a("documents_document"), a("d"))
        .join_as(
            JoinType::LeftJoin,
            a("documents_document"),
            a("r"),
            Expr::col((a("r"), a("id"))).equals((a("d"), a("root_document_id"))),
        )
        .to_owned()
}

/// `permitted_document_ids(user, perm=…, include_deleted=…)` (pl-authz) as a `SELECT id` subquery.
pub fn permitted_ids_subquery(user: &CurrentUser, perm: &str, include_deleted: bool) -> SelectStatement {
    pl_authz::objects::permitted_document_ids(Some(user), perm, include_deleted)
}

/// `Document.global_objects.filter(pk__in=ids).exclude(pk__in=permitted_document_ids(user, perm)).exists()`
pub async fn any_not_permitted(
    conn: &mut PgConnection,
    user: &CurrentUser,
    ids: &[i64],
    perm: &str,
    include_deleted: bool,
) -> Result<bool, ApiError> {
    let sub = permitted_ids_subquery(user, perm, include_deleted);
    let inner = Query::select()
        .expr(Expr::val(1))
        .from(a("documents_document"))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(a("id")).not_in_subquery(sub))
        .to_owned();
    let q = Query::select().expr(Expr::exists(inner)).to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (b,): (bool,) = sqlx::query_as_with(&sql, values).fetch_one(&mut *conn).await?;
    Ok(b)
}

/// All root documents of the (non-deleted) ids belong to `user` or to nobody.
pub async fn owns_all_roots(conn: &mut PgConnection, user: &CurrentUser, ids: &[i64]) -> Result<bool, ApiError> {
    let mut inner = base_select();
    inner
        .expr(Expr::val(1))
        .and_where(Expr::col((a("d"), a("id"))).is_in(ids.iter().copied()))
        .and_where(Expr::col((a("d"), a("deleted_at"))).is_null())
        .and_where(Expr::expr(authorizing_owner()).is_not_null())
        .and_where(Expr::expr(authorizing_owner()).ne(user.id));
    let q = Query::select().expr(Expr::exists(inner)).to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (foreign,): (bool,) = sqlx::query_as_with(&sql, values).fetch_one(&mut *conn).await?;
    Ok(!foreign)
}

/// Which conditions an operation places on the permissions (`_has_document_permissions`).
#[derive(Clone, Copy, Debug, Default)]
pub struct OperationNeeds {
    pub ownership: bool,
    pub add_document: bool,
    pub delete_document: bool,
}

/// `DocumentOperationPermissionMixin._has_document_permissions`
pub async fn has_document_permissions(
    conn: &mut PgConnection,
    user: &CurrentUser,
    documents: &[i64],
    needs: OperationNeeds,
) -> Result<bool, ApiError> {
    if user.is_superuser {
        // upstream does not check is_active here (has_perm below does); an inactive superuser
        // would not be logged in at all.
        return Ok(true);
    }
    let mut ok = has_perm(user, "change_document")
        && !any_not_permitted(conn, user, documents, "change_document", false).await?;
    if ok && needs.ownership {
        ok = owns_all_roots(conn, user, documents).await?;
    }
    if ok && needs.add_document && !has_perm(user, "add_document") {
        ok = false;
    }
    if ok && needs.delete_document && !has_perm(user, "delete_document") {
        ok = false;
    }
    Ok(ok)
}

/// Entry of an action in `PermissionsSpec`: `None` = key missing (leave unchanged).
#[derive(Clone, Debug, Default)]
pub struct GranteesUpdate {
    pub users: Option<Vec<i64>>,
    pub groups: Option<Vec<i64>>,
}

/// `PermissionsSpec` in insertion order (`view`, `change`).
pub type PermissionsSpec = Vec<(&'static str, GranteesUpdate)>;

async fn existing_ids(conn: &mut PgConnection, table: &str, ids: &[i64]) -> Result<Vec<i64>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let q = Query::select()
        .column(a("id"))
        .from(a(table))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?;
    Ok(rows.into_iter().map(|(i,)| i64::from(i)).collect())
}

/// Number of existing users/groups (for `_validate_user_ids` / `_validate_group_ids`).
pub async fn count_existing(conn: &mut PgConnection, table: &str, ids: &[i64]) -> Result<usize, ApiError> {
    Ok(existing_ids(conn, table, ids).await?.len())
}

async fn permission_ids(conn: &mut PgConnection, ctype: i32, codenames: &[String]) -> Result<Vec<(i32, String)>, ApiError> {
    let q = Query::select()
        .columns([a("id"), a("codename")])
        .from(a("auth_permission"))
        .and_where(Expr::col(a("content_type_id")).eq(ctype))
        .and_where(Expr::col(a("codename")).is_in(codenames.iter().cloned()))
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_as_with(&sql, values).fetch_all(&mut *conn).await?)
}

/// `set_permissions_for_objects(permissions, Document, pks, merge=merge)`
pub async fn set_permissions_for_documents(
    conn: &mut PgConnection,
    permissions: &PermissionsSpec,
    pks: &[i64],
    merge: bool,
) -> Result<(), ApiError> {
    let object_pks: Vec<String> = pks.iter().map(|p| p.to_string()).collect();
    if object_pks.is_empty() {
        return Ok(());
    }
    let ctype = document_content_type_id(conn).await?;

    // First resolve all actions (Permission.DoesNotExist → exception).
    let mut perms_by_action: Vec<(&'static str, Vec<i32>)> = Vec::new();
    for (action, entry) in permissions {
        if entry.users.is_none() && entry.groups.is_none() {
            continue;
        }
        let mut codenames = vec![format!("{action}_document")];
        if *action == "change" {
            codenames.push("view_document".into());
        }
        let found = permission_ids(conn, ctype, &codenames).await?;
        if found.len() != codenames.len() {
            return Err(ApiError::Uncaught("Permission matching query does not exist".into()));
        }
        perms_by_action.push((action, found.into_iter().map(|(id, _)| id).collect()));
    }

    for (action, entry) in permissions {
        let codename = format!("{action}_document");
        let permission_objs = perms_by_action
            .iter()
            .find(|(a, _)| a == action)
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        for (ids, table, identity, identity_table) in [
            (&entry.users, "guardian_userobjectpermission", "user_id", "auth_user"),
            (&entry.groups, "guardian_groupobjectpermission", "group_id", "auth_group"),
        ] {
            let Some(ids) = ids else { continue };
            apply_entry(conn, table, identity, identity_table, ids, &codename, &permission_objs, ctype, &object_pks, merge)
                .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_entry(
    conn: &mut PgConnection,
    table: &str,
    identity_field: &str,
    identity_table: &str,
    ids: &[i64],
    codename: &str,
    permission_objs: &[i32],
    ctype: i32,
    object_pks: &[String],
    merge: bool,
) -> Result<(), ApiError> {
    let add_ids = existing_ids(conn, identity_table, ids).await?;

    if !merge {
        let perm_with_codename = Query::select()
            .column(a("id"))
            .from(a("auth_permission"))
            .and_where(Expr::col(a("codename")).eq(codename))
            .to_owned();
        let mut del = Query::delete();
        del.from_table(a(table))
            .and_where(Expr::col(a("content_type_id")).eq(ctype))
            .and_where(Expr::col(a("object_pk")).is_in(object_pks.iter().cloned()))
            .and_where(Expr::col(a("permission_id")).in_subquery(perm_with_codename));
        if !add_ids.is_empty() {
            del.and_where(Expr::col(a(identity_field)).is_not_in(add_ids.iter().copied()));
        }
        let (sql, values) = del.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    }

    if add_ids.is_empty() {
        return Ok(());
    }

    let mut rows: Vec<[sea_query::SimpleExpr; 4]> = Vec::new();
    for perm in permission_objs {
        for pk in object_pks {
            for ident in &add_ids {
                rows.push([ctype.into(), pk.clone().into(), (*perm).into(), (*ident).into()]);
            }
        }
    }
    // bulk_create(rows, ignore_conflicts=True, batch_size=1000)
    for chunk in rows.chunks(1000) {
        let mut ins = Query::insert();
        ins.into_table(a(table))
            .columns([a("content_type_id"), a("object_pk"), a("permission_id"), a(identity_field)]);
        for row in chunk {
            ins.values(row.clone())
                .map_err(|e| ApiError::Uncaught(format!("sea-query: {e}")))?;
        }
        ins.on_conflict(sea_query::OnConflict::new().do_nothing().to_owned());
        let (sql, values) = ins.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    }
    Ok(())
}
