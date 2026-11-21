// upstream: src/documents/permissions.py@v3.4.2 (has_perms_owner_aware, permitted_document_ids)
//
// Thin wrappers around pl-authz for the file routes.

use pl_authz::models::DOCUMENT;
use pl_authz::perms::ObjRef;
use pl_authz::{AuthError, User};
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sqlx::PgPool;

/// `has_perms_owner_aware(user, "view_document", root)` for an already resolved root document.
pub async fn has_view_on_root(db: &PgPool, user: &User, root_id: i64, root_owner: Option<i64>) -> Result<bool, AuthError> {
    pl_authz::perms::has_perms_owner_aware(db, Some(user), "view_document", &DOCUMENT, ObjRef { id: root_id, owner_id: root_owner }).await
}

/// `documents.exclude(id__in=permitted_document_ids(user)).exists()` with
/// `documents = Document.objects.filter(pk__in=ids)`.
pub async fn any_not_permitted(db: &PgPool, user: &User, ids: &[i64]) -> Result<bool, sqlx::Error> {
    if ids.is_empty() {
        return Ok(false);
    }
    let a = Alias::new;
    let permitted = pl_authz::objects::permitted_document_ids(Some(user), "view_document", false);
    let inner = Query::select()
        .expr(Expr::val(1))
        .from(a("documents_document"))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .and_where(Expr::col(a("id")).not_in_subquery(permitted))
        .to_owned();
    let q = Query::select().expr_as(Expr::exists(inner), a("denied")).to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (b,): (bool,) = sqlx::query_as_with(&sql, values).fetch_one(db).await?;
    Ok(b)
}
