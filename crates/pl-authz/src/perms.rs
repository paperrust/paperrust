//! DRF permission classes of paperless and guardian object checks.
//!
//! upstream: src/documents/permissions.py@v3.4.2 (PaperlessObjectPermissions, PaperlessAdminPermissions,
//!           has_global_statistics_permission, has_system_status_permission, has_perms_owner_aware,
//!           GlobalPermissionsByMethod + ViewDocumentsPermissions, TrashPermissions,
//!           PaperlessNotePermissions, AcknowledgeTasksPermissions),
//!           rest_framework/permissions.py@3.16 (IsAuthenticated, DjangoModelPermissions,
//!           DjangoObjectPermissions.has_object_permission: 404 vs 403),
//!           guardian/core.py@3.5 (ObjectPermissionChecker.has_perm/get_perms),
//!           guardian/backends.py@3.5 (ObjectPermissionBackend.has_perm).

use crate::authn::AuthContext;
use crate::error::AuthError;
use crate::models::ObjectModel;
use crate::objects::{bare_codename, content_type_id_subquery};
use crate::user::User;
use axum::http::Method;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use sqlx::PgPool;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// `PaperlessObjectPermissions.perms_map` action for a method (`None` → `MethodNotAllowed`).
pub fn paperless_object_action(method: &Method) -> Option<&'static str> {
    match method.as_str() {
        "GET" | "OPTIONS" | "HEAD" => Some("view"),
        "POST" => Some("add"),
        "PUT" | "PATCH" => Some("change"),
        "DELETE" => Some("delete"),
        _ => None,
    }
}

/// A `GlobalPermissionsByMethod.perms_map` (method → required global permissions).
pub type PermsMap = &'static [(&'static str, &'static [&'static str])];

/// `ViewDocumentsPermissions.perms_map`.
pub const VIEW_DOCUMENTS_PERMISSIONS: PermsMap = &[
    ("OPTIONS", &["documents.view_document"]),
    ("GET", &["documents.view_document"]),
    ("POST", &["documents.view_document"]),
];

/// `TrashPermissions.perms_map`.
pub const TRASH_PERMISSIONS: PermsMap = &[
    ("OPTIONS", &["documents.view_document"]),
    ("GET", &["documents.view_document"]),
    ("POST", &["documents.delete_document"]),
];

/// `PaperlessNotePermissions.perms_map`.
pub const NOTE_PERMISSIONS: PermsMap = &[
    ("OPTIONS", &["documents.view_note", "documents.view_document"]),
    ("GET", &["documents.view_note", "documents.view_document"]),
    (
        "POST",
        &["documents.add_note", "documents.view_document", "documents.change_document"],
    ),
    (
        "DELETE",
        &["documents.delete_note", "documents.view_document", "documents.change_document"],
    ),
];

/// `AcknowledgeTasksPermissions.perms_map`.
pub const ACKNOWLEDGE_TASKS_PERMISSIONS: PermsMap = &[
    ("OPTIONS", &["documents.change_paperlesstask"]),
    ("POST", &["documents.change_paperlesstask"]),
];

/// `GlobalPermissionsByMethod.has_permission` (HEAD is checked as GET; unknown method → False).
pub fn global_perms_by_method(user: Option<&User>, method: &Method, map: PermsMap) -> bool {
    let Some(user) = user else { return false };
    let m = if method == Method::HEAD { "GET" } else { method.as_str() };
    map.iter()
        .find(|(k, _)| *k == m)
        .is_some_and(|(_, perms)| user.has_perms(perms))
}

/// `has_global_statistics_permission(user)`.
pub fn has_global_statistics_permission(user: Option<&User>) -> bool {
    user.is_some_and(|u| {
        u.is_active && (u.is_superuser || u.has_perm("paperless.view_global_statistics"))
    })
}

/// `has_system_status_permission(user)`.
pub fn has_system_status_permission(user: Option<&User>) -> bool {
    user.is_some_and(|u| {
        u.is_active
            && (u.is_superuser || u.is_staff || u.has_perm("paperless.view_system_monitoring"))
    })
}

/// A loaded object as the permission system needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjRef {
    pub id: i64,
    pub owner_id: Option<i64>,
}

/// guardian `ObjectPermissionChecker(user).has_perm(perm, obj)`:
/// inactive → false, superuser → true, else a direct or group grant of `perm` on the object.
pub async fn guardian_has_perm(
    db: &PgPool,
    user: &User,
    model: &ObjectModel,
    obj_id: i64,
    perm: &str,
) -> Result<bool, AuthError> {
    if !user.is_active {
        return Ok(false);
    }
    if user.is_superuser {
        return Ok(true);
    }
    if crate::grants::use_grants() {
        // L5: one index query (row content type = model, like guardian's checker).
        let sql = format!("SELECT {}", crate::grants::checker_exists_sql("$1", model, perm, "$2"));
        let (b,): (bool,) = sqlx::query_as(&sql).bind(user.id).bind(obj_id).fetch_one(db).await?;
        return Ok(b);
    }
    let codename = bare_codename(perm).to_string();
    let perm_ids = Query::select()
        .column(a("id"))
        .from(a("auth_permission"))
        .and_where(Expr::col(a("codename")).eq(codename))
        .and_where(Expr::col(a("content_type_id")).in_subquery(content_type_id_subquery(model)))
        .to_owned();
    let pk = obj_id.to_string();
    let user_grant = Query::select()
        .expr(Expr::val(1))
        .from(a("guardian_userobjectpermission"))
        .and_where(Expr::col(a("user_id")).eq(user.id))
        .and_where(Expr::col(a("content_type_id")).in_subquery(content_type_id_subquery(model)))
        .and_where(Expr::col(a("object_pk")).eq(pk.clone()))
        .and_where(Expr::col(a("permission_id")).in_subquery(perm_ids.clone()))
        .to_owned();
    let groups = Query::select()
        .column(a("group_id"))
        .from(a("auth_user_groups"))
        .and_where(Expr::col(a("user_id")).eq(user.id))
        .to_owned();
    let group_grant = Query::select()
        .expr(Expr::val(1))
        .from(a("guardian_groupobjectpermission"))
        .and_where(Expr::col(a("group_id")).in_subquery(groups))
        .and_where(Expr::col(a("content_type_id")).in_subquery(content_type_id_subquery(model)))
        .and_where(Expr::col(a("object_pk")).eq(pk))
        .and_where(Expr::col(a("permission_id")).in_subquery(perm_ids))
        .to_owned();
    let q = Query::select()
        .expr_as(
            Expr::exists(user_grant).or(Expr::exists(group_grant)),
            a("granted"),
        )
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (b,): (bool,) = sqlx::query_as_with(&sql, values).fetch_one(db).await?;
    Ok(b)
}

/// `has_perms_owner_aware(user, perm, obj)`: unowned, owned by `user`, or guardian grant.
///
/// For documents pass the **root** document (`get_root_document`): id and owner of
/// `root_document` when set. `user = None` means AnonymousUser (no grants considered).
pub async fn has_perms_owner_aware(
    db: &PgPool,
    user: Option<&User>,
    perm: &str,
    model: &ObjectModel,
    obj: ObjRef,
) -> Result<bool, AuthError> {
    if obj.owner_id.is_none() {
        return Ok(true);
    }
    let Some(user) = user else { return Ok(false) };
    if obj.owner_id == Some(user.id) {
        return Ok(true);
    }
    guardian_has_perm(db, user, model, obj.id, perm).await
}

impl AuthContext {
    /// `(IsAuthenticated, PaperlessObjectPermissions).has_permission` for the request method:
    /// 401/403 if anonymous, 405 for an unmapped method, 403 without the model permission.
    pub fn require_model_perms(&self, model: &ObjectModel) -> Result<&User, AuthError> {
        let user = self.require_authenticated()?;
        let Some(action) = paperless_object_action(&self.http_method) else {
            return Err(AuthError::MethodNotAllowed { method: self.http_method.to_string() });
        };
        if user.has_perm(&model.perm(action)) { Ok(user) } else { Err(self.deny()) }
    }

    /// `(IsAuthenticated, <GlobalPermissionsByMethod subclass>)`.
    pub fn require_perms_by_method(&self, map: PermsMap) -> Result<&User, AuthError> {
        let user = self.require_authenticated()?;
        if global_perms_by_method(Some(user), &self.http_method, map) { Ok(user) } else { Err(self.deny()) }
    }

    /// A `GlobalPermissionsByMethod` subclass used **without** `IsAuthenticated`
    /// (e.g. `PaperlessNotePermissions` on the notes action).
    pub fn check_perms_by_method(&self, map: PermsMap) -> Result<(), AuthError> {
        if global_perms_by_method(self.user(), &self.http_method, map) { Ok(()) } else { Err(self.deny()) }
    }

    /// `(IsAuthenticated, PaperlessAdminPermissions)`: active staff.
    pub fn require_admin(&self) -> Result<&User, AuthError> {
        let user = self.require_authenticated()?;
        if user.is_active && user.is_staff { Ok(user) } else { Err(self.deny()) }
    }

    /// `PaperlessObjectPermissions.has_object_permission` (after `get_object`):
    /// unowned or own → ok; else guardian object perm for the method (GET→view, PUT/PATCH→change,
    /// DELETE→delete, POST→add); without it: 404 for safe methods or without view grant,
    /// otherwise 403.
    pub async fn check_object_perms(
        &self,
        db: &PgPool,
        model: &ObjectModel,
        obj: ObjRef,
    ) -> Result<(), AuthError> {
        let Some(owner) = obj.owner_id else { return Ok(()) };
        let Some(user) = self.user() else {
            // AnonymousUser never equals an owner; guardian anonymous grants are not supported.
            return Err(AuthError::not_found());
        };
        if owner == user.id {
            return Ok(());
        }
        let Some(action) = paperless_object_action(&self.http_method) else {
            return Err(AuthError::MethodNotAllowed { method: self.http_method.to_string() });
        };
        if guardian_has_perm(db, user, model, obj.id, &model.codename(action)).await? {
            return Ok(());
        }
        let safe = matches!(self.http_method.as_str(), "GET" | "HEAD" | "OPTIONS");
        if safe || !guardian_has_perm(db, user, model, obj.id, &model.codename("view")).await? {
            return Err(AuthError::not_found());
        }
        Err(self.deny())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authn::AuthMethod;
    use crate::models::TAG;

    fn ctx(user: Option<User>, m: Method) -> AuthContext {
        match user {
            Some(u) => AuthContext::for_user(u, AuthMethod::Token, m),
            None => AuthContext::anonymous(m, Some("Token".into())),
        }
    }

    #[test]
    fn model_perms() {
        let mut u = crate::user::test_user(1);
        u.permissions.insert("documents.view_tag".into());
        assert!(ctx(Some(u.clone()), Method::GET).require_model_perms(&TAG).is_ok());
        assert!(ctx(Some(u.clone()), Method::HEAD).require_model_perms(&TAG).is_ok());
        let e = ctx(Some(u.clone()), Method::PATCH).require_model_perms(&TAG).unwrap_err();
        assert_eq!(e.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(e.detail(), "You do not have permission to perform this action.");
        let e = ctx(Some(u), Method::TRACE).require_model_perms(&TAG).unwrap_err();
        assert_eq!(e.status(), axum::http::StatusCode::METHOD_NOT_ALLOWED);
        let e = ctx(None, Method::GET).require_model_perms(&TAG).unwrap_err();
        assert_eq!(e.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn by_method() {
        let mut u = crate::user::test_user(1);
        u.permissions.insert("documents.view_document".into());
        let c = ctx(Some(u.clone()), Method::HEAD);
        assert!(c.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS).is_ok());
        let c = ctx(Some(u.clone()), Method::DELETE);
        assert!(c.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS).is_err());
        let c = ctx(Some(u), Method::POST);
        assert!(c.require_perms_by_method(TRASH_PERMISSIONS).is_err());
        let c = ctx(None, Method::GET);
        assert_eq!(
            c.check_perms_by_method(NOTE_PERMISSIONS).unwrap_err().status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn helpers() {
        let mut u = crate::user::test_user(1);
        assert!(!has_system_status_permission(Some(&u)));
        u.is_staff = true;
        assert!(has_system_status_permission(Some(&u)));
        assert!(!has_global_statistics_permission(Some(&u)));
        u.permissions.insert("paperless.view_global_statistics".into());
        assert!(has_global_statistics_permission(Some(&u)));
        assert!(!has_system_status_permission(None));
        let c = ctx(Some(u.clone()), Method::GET);
        assert!(c.require_admin().is_ok());
        u.is_staff = false;
        assert!(ctx(Some(u), Method::GET).require_admin().is_err());
    }
}
