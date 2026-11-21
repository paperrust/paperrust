//! Permission subqueries that individual filters need (`has_duplicates`, `shared_by__id`).
//!
//! upstream: src/documents/permissions.py@v3.4.2 (permitted_document_ids),
//! src/documents/filters.py@v3.4.2 (SharedByUser, DocumentFilterSet.filter_has_duplicates)
//!
//! The visibility of the list itself is set by pl-documents via pl-authz. These building blocks
//! ensure that filters do not let invisible documents "shine through" (e.g. duplicates
//! the user must not see). The permission logic comes from pl-authz.

use pl_authz::models::DOCUMENT;
use pl_authz::objects::{content_type_id_subquery, permitted_document_ids};
use sea_query::{Alias, Expr, ExprTrait, Query, SelectStatement, SimpleExpr};

use crate::User;
use crate::sql::{DOC, col};

/// `permitted_document_ids(user, include_deleted=True)` (for `has_duplicates`).
pub fn visible_root_documents(user: &User) -> SelectStatement {
    permitted_document_ids(Some(user), "view_document", true)
}

/// `SharedByUser`: any user or group object permission on the document
/// (`content_type = Document`, `object_pk = CAST(id AS varchar)`).
pub fn document_is_shared() -> SimpleExpr {
    if pl_authz::grants::use_grants() {
        // L5: sharing index (row content type = document, any permission).
        return pl_authz::grants::any_grant_exists(&DOCUMENT, col(DOC, "id"));
    }
    let key: SimpleExpr = Expr::cust_with_exprs("CAST($1 AS varchar)", [col(DOC, "id")]);
    let mk = |table: &str, alias: &str| {
        Query::select()
            .expr(Expr::val(1))
            .from_as(Alias::new(table), Alias::new(alias))
            .and_where(col(alias, "content_type_id").in_subquery(content_type_id_subquery(&DOCUMENT)))
            .and_where(col(alias, "object_pk").eq(key.clone()))
            .to_owned()
    };
    Expr::exists(mk("guardian_userobjectpermission", "sbu")).or(Expr::exists(mk("guardian_groupobjectpermission", "sbg")))
}
