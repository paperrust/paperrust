//! Permissions – thin wrapper around `pl-authz`.
//!
//! upstream: src/documents/permissions.py@v3.4.2 (PaperlessObjectPermissions, permitted_object_ids,
//! permitted_document_ids, get_permission_grantees, set_permissions_for_objects,
//! has_perms_owner_aware), src/documents/filters.py@v3.4.2 (PermittedObjectsFilter),
//! src/documents/serialisers.py@v3.4.2 (OwnedObjectSerializer, SetPermissionsMixin).

use std::collections::HashMap;

use pl_authz::models::{DOCUMENT, ObjectModel};
use pl_authz::objects::{FilterOpts, granted_key_cond, permitted_document_ids, permitted_objects_filter};
use pl_authz::owned::{self, ObjectGrantees, PermissionsSpec};
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use sea_query::{Alias, Condition, Expr, PostgresQueryBuilder, Query};
use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::shim::{ApiError, ApiResult, Ctx};

/// `PermittedObjectsFilter` as a condition on `alias`.
pub fn visible(ctx: &Ctx, model: &ObjectModel, alias: &str) -> Condition {
    permitted_objects_filter(Some(ctx.authz_user()), model, alias, &FilterOpts::default())
}

/// `permitted_document_ids(user)` as SQL text (values safely inlined by sea-query), for
/// embedding into `Expr::cust` fragments.
pub fn permitted_document_ids_sql(ctx: &Ctx) -> String {
    permitted_document_ids(Some(ctx.authz_user()), "view_document", false).to_string(PostgresQueryBuilder)
}

/// `alias.id IN permitted_document_ids(user)` as a condition on the document row itself (alias of
/// `documents_document`), without the self-JOIN of the general form (PERF-SCALE #3c / Q7):
/// root documents check owner/sharing of their own row, versions via `EXISTS` those of their
/// root document (`authorizing_owner`/`authorizing_id` as upstream). Superuser: all non-
/// deleted ones, inactive users: none. Sharing check via `pl_authz::objects::granted_key_cond`
/// (L5 swap point). Same shape as `pl_objects::perms::permitted_doc_row`.
pub fn permitted_doc_row(ctx: &Ctx, alias: &str) -> Condition {
    permitted_doc_row_for(ctx.authz_user(), alias)
}

/// [`permitted_doc_row`] for a user (without request context).
pub fn permitted_doc_row_for(user: &pl_authz::User, alias: &str) -> Condition {
    let al = |s: &str| Alias::new(s);
    let col = |c: &str| Expr::col((al(alias), al(c)));
    let base = Condition::all().add(col("deleted_at").is_null());
    if !user.is_active {
        return base.add(Expr::cust("FALSE"));
    }
    if user.is_superuser {
        return base;
    }
    let owner_vis = |t: &str| {
        let owner = Expr::col((al(t), al("owner_id")));
        Condition::any().add(owner.clone().eq(user.id)).add(owner.is_null()).add(granted_key_cond(
            user,
            &DOCUMENT,
            "view_document",
            Expr::col((al(t), al("id"))).into(),
        ))
    };
    let mut root = Query::select();
    root.expr(Expr::cust("1"))
        .from_as(al("documents_document"), al("pl_pr"))
        .and_where(Expr::col((al("pl_pr"), al("id"))).equals((al(alias), al("root_document_id"))))
        .cond_where(owner_vis("pl_pr"));
    base.add(
        Condition::any()
            .add(Condition::all().add(col("root_document_id").is_null()).add(owner_vis(alias)))
            .add(Condition::all().add(col("root_document_id").is_not_null()).add(Expr::exists(root))),
    )
}

/// `IsAuthenticated` + `PaperlessObjectPermissions.has_permission` (request method).
pub fn require_model_perm(ctx: &Ctx, model: &ObjectModel) -> ApiResult<()> {
    ctx.auth.require_model_perms(model)?;
    Ok(())
}

/// `PaperlessObjectPermissions.has_object_permission` (request method).
pub async fn check_object_perm(db: &PgPool, ctx: &Ctx, model: &ObjectModel, id: i64, owner_id: Option<i64>) -> ApiResult<()> {
    ctx.auth.check_object_perms(db, model, ObjRef { id, owner_id }).await?;
    Ok(())
}

pub async fn grantees(db: &PgPool, model: &ObjectModel, ids: &[i64]) -> ApiResult<HashMap<i64, ObjectGrantees>> {
    Ok(owned::get_permission_grantees(db, model, ids).await?)
}

pub fn user_can_change(ctx: &Ctx, owner_id: Option<i64>, g: Option<&ObjectGrantees>) -> bool {
    owned::user_can_change(Some(ctx.authz_user()), owner_id, g)
}

/// `SetPermissionsSerializer` + `validate_set_permissions`; `Err(v)` = field error value.
pub async fn parse_set_permissions(db: &PgPool, v: &Value) -> ApiResult<Result<PermissionsSpec, Value>> {
    let raw = match owned::parse_set_permissions(v) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e.0)),
    };
    Ok(owned::validate_set_permissions(db, &raw).await?.map_err(|e| e.0))
}

/// `set_permissions_for_objects(permissions, model, [pk], merge=False)`
pub async fn apply_set_permissions(db: &PgPool, model: &ObjectModel, id: i64, spec: &PermissionsSpec) -> ApiResult<()> {
    let mut conn = db.acquire().await?;
    owned::set_permissions_for_objects(&mut conn, spec, model, &[id], false).await?;
    Ok(())
}

/// `PrimaryKeyRelatedField(queryset=User.objects.all(), allow_null=True)`
pub async fn parse_owner(db: &PgPool, v: &Value) -> ApiResult<Result<Option<i64>, Value>> {
    Ok(owned::parse_owner(db, v).await?.map_err(|e| e.0))
}

/// `user.has_perm("documents.view_document") and has_perms_owner_aware(user, "view_document",
/// doc)` – versions are authorised via their root.
pub async fn can_share_document(db: &PgPool, ctx: &Ctx, doc_id: i64) -> ApiResult<bool> {
    if !ctx.user.has_perm("documents.view_document") {
        return Ok(false);
    }
    let row = sqlx::query(
        "SELECT COALESCE(d.root_document_id, d.id)::int8 AS auth_id, \
                (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END)::int8 AS owner \
         FROM documents_document d LEFT OUTER JOIN documents_document r ON r.id = d.root_document_id \
         WHERE d.id = $1::int8",
    )
    .bind(doc_id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::ServerError);
    };
    let obj = ObjRef { id: row.try_get("auth_id")?, owner_id: row.try_get("owner")? };
    Ok(has_perms_owner_aware(db, Some(ctx.authz_user()), "view_document", &DOCUMENT, obj).await?)
}
