// upstream: django/contrib/auth/backends.py (ModelBackend.has_perm)
//
//! The user comes from `pl-authz` (authentication + model permissions).

use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sqlx::PgConnection;

use crate::error::ApiError;

/// The logged-in user.
pub type CurrentUser = pl_authz::User;

/// `user.has_perm("documents.<codename>")`
pub fn has_perm(user: &CurrentUser, codename: &str) -> bool {
    user.has_perm(&format!("documents.{codename}"))
}

/// Content type id of `documents.document`.
pub async fn document_content_type_id(conn: &mut PgConnection) -> Result<i32, ApiError> {
    let q = Query::select()
        .column(Alias::new("id"))
        .from(Alias::new("django_content_type"))
        .and_where(Expr::col(Alias::new("app_label")).eq("documents"))
        .and_where(Expr::col(Alias::new("model")).eq("document"))
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (id,): (i32,) = sqlx::query_as_with(&sql, values).fetch_one(&mut *conn).await?;
    Ok(id)
}

/// Who set `request.user` (like `pl_objects::drf::request_user_source`): Django middleware
/// (session, AutoLogin, RemoteUser) → auditlog actor set; DRF authentication (token,
/// basic, DebugAngular) → actor NULL (`AuditlogMiddleware` only sees the anonymous user).
pub fn request_user_source(method: Option<pl_authz::authn::AuthMethod>) -> pl_core::client::RequestUserSource {
    use pl_authz::authn::AuthMethod;
    use pl_core::client::RequestUserSource;
    match method {
        Some(AuthMethod::Session | AuthMethod::AutoLogin | AuthMethod::RemoteUser) => RequestUserSource::DjangoMiddleware,
        _ => RequestUserSource::DrfAuthentication,
    }
}

/// Does the middleware set the auditlog actor (`pl_core::client::audit_actor`)?
pub fn middleware_sets_actor(method: Option<pl_authz::authn::AuthMethod>) -> bool {
    pl_core::client::audit_actor(0, "", request_user_source(method)).0.is_some()
}
