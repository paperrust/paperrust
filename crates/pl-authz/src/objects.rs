//! Object visibility as sea-query building blocks: `permitted_object_ids`, `PermittedObjectsFilter`,
//! `restrict_queryset_to_visible`, `restrict_queryset_to_owner_visible`, `permitted_document_ids`.
//!
//! upstream: src/documents/permissions.py@v3.4.2 (permitted_object_ids, annotate_authorizing_fields,
//!           user_is_unrestricted, user_is_active_superuser, restrict_queryset_to_visible,
//!           restrict_queryset_to_owner_visible, permitted_document_ids),
//!           src/documents/filters.py@v3.4.2 (PermittedObjectsFilter).
//!
//! All functions are pure (no DB access) and return a [`SelectStatement`] of ids or a
//! [`Condition`] to attach to a list query. Grants are matched with one uncorrelated
//! `IN (… UNION ALL …)` against guardian's `object_pk` (TEXT), exactly like upstream, so
//! PostgreSQL builds the grant set once (no N+1, no per-row subquery).

use crate::models::{DOCUMENT, DOCUMENT_PARENT_FIELD, ObjectModel};
use crate::user::User;
use sea_query::{
    Alias, Cond, Condition, Expr, ExprTrait as _, Func, IntoCondition, JoinType, Query, SelectStatement, SimpleExpr, UnionType,
};

/// Alias of the base table inside generated subqueries (never clashes with callers' aliases).
const T: &str = "pl_authz_obj";
/// Alias of the joined parent row (`root_document`).
const P: &str = "pl_authz_parent";

fn a(s: &str) -> Alias {
    Alias::new(s)
}

fn col(table: &str, column: &str) -> SimpleExpr {
    Expr::col((a(table), a(column))).into()
}

/// `SELECT id FROM django_content_type WHERE app_label = .. AND model = ..` (scalar subquery).
pub fn content_type_id_subquery(model: &ObjectModel) -> SelectStatement {
    Query::select()
        .column(a("id"))
        .from(a("django_content_type"))
        .and_where(Expr::col(a("app_label")).eq(model.app_label))
        .and_where(Expr::col(a("model")).eq(model.model_name))
        .to_owned()
}

/// Strip an `app_label.` prefix: guardian stores bare codenames.
pub fn bare_codename(perm: &str) -> &str {
    perm.rsplit_once('.').map(|(_, c)| c).unwrap_or(perm)
}

/// The user's and their groups' grant keys (`object_pk`) for `perm` on `model`:
/// `user_keys.union(group_keys, all=True)` of upstream.
pub fn granted_keys_subquery(user: &User, model: &ObjectModel, perm: &str) -> SelectStatement {
    let codename = bare_codename(perm).to_string();
    let perm_ids = Query::select()
        .column(a("id"))
        .from(a("auth_permission"))
        .and_where(Expr::col(a("codename")).eq(codename))
        .and_where(Expr::col(a("content_type_id")).in_subquery(content_type_id_subquery(model)))
        .to_owned();
    let user_keys = Query::select()
        .column(a("object_pk"))
        .from(a("guardian_userobjectpermission"))
        .and_where(Expr::col(a("user_id")).eq(user.id))
        .and_where(Expr::col(a("permission_id")).in_subquery(perm_ids.clone()))
        .to_owned();
    let groups =
        Query::select().column(a("group_id")).from(a("auth_user_groups")).and_where(Expr::col(a("user_id")).eq(user.id)).to_owned();
    let group_keys = Query::select()
        .column(a("object_pk"))
        .from(a("guardian_groupobjectpermission"))
        .and_where(Expr::col(a("group_id")).in_subquery(groups))
        .and_where(Expr::col(a("permission_id")).in_subquery(perm_ids))
        .to_owned();
    let mut q = user_keys;
    q.union(UnionType::All, group_keys);
    q
}

/// Options of [`permitted_object_ids`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PermOpts {
    /// Use `global_objects` (include soft-deleted rows); only affects soft-delete models.
    pub include_deleted: bool,
    /// Self-referencing FK column whose target authorizes the row (`root_document_id`).
    pub parent_field: Option<&'static str>,
}

/// Owner and key expressions for the base row (`authorizing_owner`/`authorizing_id`).
fn owner_and_key(parent: Option<&str>) -> (SimpleExpr, SimpleExpr) {
    match parent {
        None => (col(T, "owner_id"), col(T, "id")),
        Some(pf) => {
            let owner: SimpleExpr = Expr::case(Expr::col((a(T), a(pf))).is_null(), col(T, "owner_id")).finally(col(P, "owner_id")).into();
            let key: SimpleExpr = Func::coalesce([col(T, pf), col(T, "id")]).into();
            (owner, key)
        }
    }
}

fn base_select(model: &ObjectModel, opts: &PermOpts, apply_manager: bool) -> SelectStatement {
    let mut q = Query::select();
    q.column((a(T), a("id"))).from_as(a(model.table), a(T));
    if let Some(pf) = opts.parent_field {
        q.join_as(JoinType::LeftJoin, a(model.table), a(P), Expr::col((a(P), a("id"))).equals((a(T), a(pf))));
    }
    if apply_manager && model.soft_delete && !opts.include_deleted {
        q.and_where(Expr::col((a(T), a("deleted_at"))).is_null());
    }
    q
}

/// `permitted_object_ids(user, model, perm, include_deleted=, parent_field=)`:
/// `SELECT id` of the rows `user` may access with `perm`.
///
/// * `None` (anonymous / unauthenticated) → unowned rows only.
/// * inactive user → nothing (even superusers).
/// * active superuser → every row (of the manager).
/// * otherwise → owned by user, unowned, or granted to the user or one of their groups.
pub fn permitted_object_ids(user: Option<&User>, model: &ObjectModel, perm: &str, opts: &PermOpts) -> SelectStatement {
    let mut q = base_select(model, opts, true);
    let (owner, key) = owner_and_key(opts.parent_field);
    let Some(user) = user else {
        q.and_where(owner.is_null());
        return q;
    };
    if !user.is_active {
        q.and_where(SimpleExpr::from(Expr::val(false)));
        return q;
    }
    if user.is_superuser {
        return q;
    }
    let granted = granted_key_cond(user, model, perm, key);
    q.cond_where(Cond::any().add(owner.clone().eq(user.id)).add(owner.is_null()).add(granted));
    q
}

/// `key` (integer expression of the row) is shared with `user` or one of their groups with `perm`:
/// `legacy` = `CAST(key AS varchar) IN (guardian object_pk …)` as upstream, `grants` (L5) =
/// `key IN (SELECT object_id FROM paperrust_grant …)` – same set (contract "L5").
pub fn granted_key_cond(user: &User, model: &ObjectModel, perm: &str, key: SimpleExpr) -> SimpleExpr {
    if crate::grants::use_grants() {
        key.in_subquery(crate::grants::granted_ids_subquery(user.id, model, perm))
    } else {
        key.cast_as(a("varchar")).in_subquery(granted_keys_subquery(user, model, perm))
    }
}

/// `permitted_document_ids(user, perm=, include_deleted=)`: documents incl. root-document logic.
pub fn permitted_document_ids(user: Option<&User>, perm: &str, include_deleted: bool) -> SelectStatement {
    permitted_object_ids(user, &DOCUMENT, perm, &PermOpts { include_deleted, parent_field: Some(DOCUMENT_PARENT_FIELD) })
}

/// `user_is_unrestricted(user)`: `None` or an active superuser.
pub fn user_is_unrestricted(user: Option<&User>) -> bool {
    match user {
        None => true,
        Some(u) => u.is_active && u.is_superuser,
    }
}

/// `user_is_active_superuser(user)`.
pub fn user_is_active_superuser(user: Option<&User>) -> bool {
    user.is_some_and(|u| u.is_active && u.is_superuser)
}

/// `<id_col> IN (subquery)`.
fn id_in(table_alias: &str, sub: SelectStatement) -> Condition {
    Expr::col((a(table_alias), a("id"))).in_subquery(sub).into_condition()
}

/// `restrict_queryset_to_visible(qs, user, perm)`; `None` = "no user filtering" (unrestricted).
/// `table_alias` is the alias/table name of the outer query's model table.
pub fn restrict_to_visible(user: Option<&User>, model: &ObjectModel, perm: &str, table_alias: &str) -> Condition {
    if user_is_unrestricted(user) {
        return Cond::all();
    }
    id_in(table_alias, permitted_object_ids(user, model, perm, &PermOpts::default()))
}

/// `restrict_queryset_to_owner_visible(qs, user, perm)`; `None` sees only unowned rows.
pub fn restrict_to_owner_visible(user: Option<&User>, model: &ObjectModel, perm: &str, table_alias: &str) -> Condition {
    if user_is_active_superuser(user) {
        return Cond::all();
    }
    id_in(table_alias, permitted_object_ids(user, model, perm, &PermOpts::default()))
}

/// Configuration of a `PermittedObjectsFilter` subclass.
#[derive(Debug, Clone, Copy)]
pub struct FilterOpts {
    /// `include_granted` (False for e.g. `TrashView`: owned/unowned only).
    pub include_granted: bool,
    /// `perm_codename` (default `view_<model_name>`).
    pub perm_codename: Option<&'static str>,
    /// `parent_field` (`Some(DOCUMENT_PARENT_FIELD)` for documents).
    pub parent_field: Option<&'static str>,
}

impl Default for FilterOpts {
    fn default() -> Self {
        Self { include_granted: true, perm_codename: None, parent_field: None }
    }
}

/// `PermittedObjectsFilter.filter_queryset` as a condition on the outer query.
///
/// `user` is `request.user` (`None` = AnonymousUser). `table_alias` names the outer model table.
pub fn permitted_objects_filter(user: Option<&User>, model: &ObjectModel, table_alias: &str, opts: &FilterOpts) -> Condition {
    if let Some(u) = user {
        if !u.is_active {
            return SimpleExpr::from(Expr::val(false)).into_condition();
        }
        if u.is_superuser {
            return Cond::all();
        }
    }
    if !opts.include_granted {
        // Q(owner=user.pk) | Q(owner__isnull=True) on the outer row (no manager filter).
        let uid = user.map(|u| u.id);
        return match opts.parent_field {
            None => {
                let owner = Expr::col((a(table_alias), a("owner_id")));
                let mut c = Cond::any().add(owner.clone().is_null());
                if let Some(uid) = uid {
                    c = c.add(Expr::col((a(table_alias), a("owner_id"))).eq(uid));
                }
                c
            }
            Some(pf) => {
                let popts = PermOpts { include_deleted: true, parent_field: Some(pf) };
                let mut q = base_select(model, &popts, false);
                let (owner, _) = owner_and_key(Some(pf));
                let mut c = Cond::any().add(owner.clone().is_null());
                if let Some(uid) = uid {
                    c = c.add(owner.eq(uid));
                }
                q.cond_where(c);
                id_in(table_alias, q)
            }
        };
    }
    let default_perm;
    let perm = match opts.perm_codename {
        Some(p) => p,
        None => {
            default_perm = model.codename("view");
            &default_perm
        }
    };
    let popts = PermOpts { include_deleted: false, parent_field: opts.parent_field };
    id_in(table_alias, permitted_object_ids(user, model, perm, &popts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TAG;
    use sea_query::PostgresQueryBuilder;

    fn user(id: i64) -> User {
        crate::user::test_user(id)
    }

    #[test]
    fn sql_shapes() {
        let u = user(7);
        let q = permitted_object_ids(Some(&u), &TAG, "documents.view_tag", &PermOpts::default());
        let s = q.to_string(PostgresQueryBuilder);
        assert!(s.starts_with(r#"SELECT "pl_authz_obj"."id" FROM "documents_tag" AS "pl_authz_obj" WHERE"#), "{s}");
        assert!(s.contains(r#""pl_authz_obj"."owner_id" = 7"#), "{s}");
        assert!(s.contains(r#""pl_authz_obj"."owner_id" IS NULL"#), "{s}");
        assert!(s.contains(r#"CAST("pl_authz_obj"."id" AS varchar) IN"#), "{s}");
        assert!(s.contains("UNION ALL"), "{s}");
        assert!(s.contains(r#""codename" = 'view_tag'"#), "{s}");
        assert!(!s.contains("deleted_at"), "{s}");

        let anon = permitted_object_ids(None, &TAG, "view_tag", &PermOpts::default());
        assert_eq!(
            anon.to_string(PostgresQueryBuilder),
            r#"SELECT "pl_authz_obj"."id" FROM "documents_tag" AS "pl_authz_obj" WHERE "pl_authz_obj"."owner_id" IS NULL"#
        );
    }

    #[test]
    fn documents_use_root() {
        let u = user(3);
        let s = permitted_document_ids(Some(&u), "view_document", false).to_string(PostgresQueryBuilder);
        assert!(s.contains(r#"LEFT JOIN "documents_document" AS "pl_authz_parent""#), "{s}");
        assert!(s.contains(r#""pl_authz_obj"."deleted_at" IS NULL"#), "{s}");
        assert!(s.contains(r#"COALESCE("pl_authz_obj"."root_document_id", "pl_authz_obj"."id")"#), "{s}");
        assert!(s.contains("CASE WHEN"), "{s}");
    }

    #[test]
    fn superuser_and_inactive() {
        let mut u = user(1);
        u.is_superuser = true;
        let c = permitted_objects_filter(Some(&u), &TAG, "t", &FilterOpts::default());
        let s = Query::select().column(a("id")).from(a("t")).cond_where(c).to_string(PostgresQueryBuilder);
        assert_eq!(s, r#"SELECT "id" FROM "t" WHERE TRUE"#);
        u.is_active = false;
        let c = permitted_objects_filter(Some(&u), &TAG, "t", &FilterOpts::default());
        let s = Query::select().column(a("id")).from(a("t")).cond_where(c).to_string(PostgresQueryBuilder);
        assert_eq!(s, r#"SELECT "id" FROM "t" WHERE FALSE"#);
        assert!(user_is_unrestricted(None));
        assert!(!user_is_active_superuser(None));
    }

    #[test]
    fn owner_only_filter() {
        let u = user(5);
        let c = permitted_objects_filter(Some(&u), &TAG, "t", &FilterOpts { include_granted: false, ..Default::default() });
        let s = Query::select().column(a("id")).from(a("t")).cond_where(c).to_string(PostgresQueryBuilder);
        assert_eq!(s, r#"SELECT "id" FROM "t" WHERE "t"."owner_id" IS NULL OR "t"."owner_id" = 5"#);
    }
}
