//! Single-object helpers: `get_object`, object permissions, children loading, deletion.
//!
//! upstream: rest_framework 3.18.3 generics.GenericAPIView.get_object,
//!           src/documents/permissions.py@v3.4.2 (PaperlessObjectPermissions),
//!           src/documents/serialisers.py@v3.4.2 (TagSerializer.get_children, OwnedObjectSerializer),
//!           upstream 8aa99a381 (#14612: adjacency-list tags), Django 5.2 deletion.Collector

use std::collections::{HashMap, HashSet};

use axum::http::StatusCode;
use pl_authz::owned::ObjectGrantees;
use pl_authz::perms::{ObjRef, guardian_has_perm};
use pl_authz::{AuthUser, User};
use pl_core::sqlbind::SqlxBinder;
use pl_core::{ApiError, RequestContext};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::audit;
use crate::drf::{self, AuditMeta};
use crate::listing::{self, ALIAS};
use crate::model::{Kind, ObjectRow, Relation};
use crate::perms;
use crate::serialize::{self, ChangeSource, SerCtx};
use crate::tagtree::TagTree;

/// Django's 404 page (URL resolver does not match, e.g. pk with a dot).
const NOT_FOUND_PAGE: &str = "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Not Found</title>\n</head>\n<body>\n  <h1>Not Found</h1><p>The requested resource was not found on this server.</p>\n</body>\n</html>\n";

/// `(IsAuthenticated, PaperlessObjectPermissions).has_permission`.
pub fn check_model_perm(auth: &AuthUser, kind: Kind) -> Result<&User, ApiError> {
    auth.require_model_perms(kind.model()).map_err(drf::auth)
}

/// Parse the `{pk}` URL segment like the router regex `[^/.]+` + `get_object_or_404`.
pub fn parse_pk(raw: &str) -> Result<Option<i64>, ApiError> {
    if raw.contains('.') || raw.contains('/') || raw.is_empty() {
        return Err(ApiError::django_text(StatusCode::NOT_FOUND, NOT_FOUND_PAGE));
    }
    match crate::text::py_int(raw) {
        None => Err(ApiError::not_found()),
        Some(i) => Ok(i32::try_from(i).ok().map(i64::from)),
    }
}

/// `self.get_object()` (filter_queryset incl. PermittedObjectsFilter on the annotated queryset).
pub async fn get_object(db: &PgPool, kind: Kind, user: &User, pk: Option<i64>, with_last_corr: bool) -> Result<ObjectRow, ApiError> {
    let Some(pk) = pk else { return Err(ApiError::not_found_model(kind.object_name())) };
    let counters = crate::counters::available(db).await;
    let mut q = listing::select_counted(kind, user, true, with_last_corr, Some(&[pk]), counters);
    q.and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).eq(pk));
    listing::restrict_visible(&mut q, kind, user, kind.codename("view"));
    let rows = listing::fetch(db, kind, &q, true, with_last_corr).await?;
    rows.into_iter().next().ok_or_else(|| ApiError::not_found_model(kind.object_name()))
}

/// `PaperlessObjectPermissions.has_object_permission`.
pub async fn check_object_perm(db: &PgPool, auth: &AuthUser, kind: Kind, row: &ObjectRow) -> Result<(), ApiError> {
    auth.check_object_perms(db, kind.model(), ObjRef { id: row.id, owner_id: row.owner_id }).await.map_err(drf::auth)
}

/// Children map for retrieve/create/update (`TagSerializer.get_children` without `children_map`):
/// visible tags below `root`, ordered by the request ordering, grouped by parent.
pub async fn children_of(
    db: &PgPool,
    user: &User,
    root: &ObjectRow,
    ctx: &RequestContext,
) -> Result<HashMap<i64, Vec<ObjectRow>>, ApiError> {
    let mut out: HashMap<i64, Vec<ObjectRow>> = HashMap::new();
    if root.tag.is_none() {
        return Ok(out);
    }
    let tree = TagTree::load_pool(db).await?;
    let mut desc: Vec<i64> = tree.descendants([root.id]).into_iter().collect();
    if desc.is_empty() {
        return Ok(out);
    }
    desc.sort_unstable();
    let counters = crate::counters::available(db).await;
    let mut q = listing::select_counted(Kind::Tag, user, true, false, Some(&desc), counters);
    q.and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).is_in(desc));
    q.cond_where(perms::restrict_visible_cond(user, Kind::Tag, "view_tag", ALIAS));
    let terms = listing::ordering_terms(Kind::Tag, drf::param(ctx, "ordering"), false);
    listing::apply_ordering(&mut q, &terms, false)?;
    let rows = listing::fetch(db, Kind::Tag, &q, true, false).await?;
    // Walk from the root through the parent edges, so only reachable (visible) nodes appear.
    let mut frontier = vec![root.id];
    while let Some(node) = frontier.pop() {
        let kids: Vec<ObjectRow> = rows.iter().filter(|r| r.tag.as_ref().and_then(|t| t.tn_parent_id) == Some(node)).cloned().collect();
        frontier.extend(kids.iter().map(|k| k.id));
        if !kids.is_empty() {
            out.insert(node, kids);
        }
    }
    Ok(out)
}

/// Serialize one object outside the list action (retrieve/create/update responses).
pub async fn serialize_single(
    db: &PgPool,
    kind: Kind,
    user: &User,
    ctx: &RequestContext,
    row: &ObjectRow,
) -> Result<serde_json::Value, ApiError> {
    let children = if kind == Kind::Tag { Some(children_of(db, user, row, ctx).await?) } else { None };
    let full_perms = drf::full_perms(ctx);
    let grantees: HashMap<i64, ObjectGrantees> = if full_perms {
        let pks = serialize::all_pks(std::slice::from_ref(row), children.as_ref());
        pl_authz::owned::get_permission_grantees(db, kind.model(), &pks).await.map_err(drf::auth)?
    } else {
        HashMap::new()
    };
    let mut can_change = HashSet::new();
    if !full_perms {
        for r in collect_rows(row, children.as_ref()) {
            if r.owner_id.is_some()
                && r.owner_id != Some(user.id)
                && !(user.is_active && user.is_superuser)
                && guardian_has_perm(db, user, kind.model(), r.id, kind.codename("change")).await.map_err(drf::auth)?
            {
                can_change.insert(r.id);
            }
        }
    }
    let sctx = SerCtx { user, full_perms, change: ChangeSource::PerObject(&can_change), grantees: &grantees, children: children.as_ref() };
    Ok(serialize::serialize(kind, row, &sctx))
}

fn collect_rows<'a>(root: &'a ObjectRow, children: Option<&'a HashMap<i64, Vec<ObjectRow>>>) -> Vec<&'a ObjectRow> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(k) = children.and_then(|c| c.get(&out[i].id)) {
            out.extend(k.iter());
        }
        i += 1;
    }
    out
}

/// Load rows (unannotated) inside a transaction, ordered by id.
pub async fn load_rows_tx(tx: &mut Transaction<'_, Postgres>, kind: Kind, user: &User, ids: &[i64]) -> Result<Vec<ObjectRow>, ApiError> {
    let mut q = listing::select(kind, user, false, false, false);
    q.and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).is_in(ids.to_vec()));
    q.order_by((Alias::new(ALIAS), Alias::new("id")), sea_query::Order::Asc);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(&mut **tx).await?;
    rows.iter().map(|r| listing::decode(kind, r, false, false).map_err(ApiError::from)).collect()
}

/// Django `QuerySet.delete()` for `ids` incl. Collector cascade (tags: all descendants) and audit entries.
/// Returns the deleted rows. Storage-path document renames are the caller's business.
/// `_with_tag_descendants` (upstream fix fa83a4c5d / #14687, after v3.4.2): the given tags plus
/// every descendant (through `tn_parent`), i.e. every tag a delete of `ids` removes.
pub async fn tag_ids_with_descendants(db: &PgPool, ids: &[i64]) -> Result<Vec<i64>, ApiError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = sqlx::query(
        "WITH RECURSIVE sub(id) AS (SELECT id FROM documents_tag WHERE id = ANY($1::bigint[])          UNION SELECT t.id FROM documents_tag t JOIN sub ON t.tn_parent_id = sub.id) SELECT id FROM sub ORDER BY id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(|r| r.try_get::<i32, _>(0).map(i64::from)).collect::<Result<_, _>>()?)
}

/// `TagViewSet.perform_destroy` (upstream fix fa83a4c5d / #14687, after v3.4.2): deleting a tag
/// also deletes its descendants, so every one of them must be in
/// `permitted_object_ids(user, Tag, "delete_tag")`; otherwise `PermissionDenied("Insufficient permissions.")`.
pub async fn check_tag_descendants_deletable(db: &PgPool, user: &User, tag_id: i64) -> Result<(), ApiError> {
    let ids = tag_ids_with_descendants(db, &[tag_id]).await?;
    let mut q = Query::select();
    q.expr(Expr::val(1))
        .from_as(Alias::new("documents_tag"), Alias::new(ALIAS))
        .and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).is_in(ids))
        .cond_where(sea_query::Cond::all().not().add(perms::permitted_ids_cond(user, Kind::Tag, "delete_tag", ALIAS)))
        .limit(1);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    if sqlx::query_with(&sql, values).fetch_optional(db).await?.is_some() {
        return Err(ApiError::permission_denied_msg(pl_authz::error::INSUFFICIENT_PERMISSIONS));
    }
    Ok(())
}

pub async fn delete_objects(
    tx: &mut Transaction<'_, Postgres>,
    meta: &AuditMeta,
    audit_on: bool,
    user: &User,
    kind: Kind,
    ids: &[i64],
) -> Result<Vec<ObjectRow>, ApiError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut all: Vec<i64> = ids.to_vec();
    if kind == Kind::Tag {
        // tn_parent on_delete=CASCADE: collect all descendants through the FK
        let rows = sqlx::query(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM documents_tag WHERE id = ANY($1::bigint[]) \
             UNION SELECT t.id FROM documents_tag t JOIN sub ON t.tn_parent_id = sub.id) SELECT id FROM sub",
        )
        .bind(ids)
        .fetch_all(&mut **tx)
        .await?;
        all = rows.iter().map(|r| r.try_get::<i32, _>(0).map(i64::from)).collect::<Result<_, _>>()?;
    }
    let deleted = load_rows_tx(tx, kind, user, &all).await?;
    // Raw SQL: table/column names are constants from `Kind::relations()`.
    for rel in kind.relations() {
        match rel {
            Relation::SetNull { table, column } => {
                sqlx::query(&format!("UPDATE {table} SET {column} = NULL WHERE {column} = ANY($1::bigint[])"))
                    .bind(&all)
                    .execute(&mut **tx)
                    .await?;
            }
            Relation::M2m { table, column } => {
                sqlx::query(&format!("DELETE FROM {table} WHERE {column} = ANY($1::bigint[])")).bind(&all).execute(&mut **tx).await?;
            }
        }
    }
    let (sql, values) = Query::delete()
        .from_table(Alias::new(kind.table()))
        .and_where(Expr::col(Alias::new("id")).is_in(all.clone()))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut **tx).await?;
    if audit_on {
        for r in &deleted {
            audit::log(tx, meta, kind, audit::DELETE, r, audit::diff(kind, Some(r), None)).await?;
        }
    }
    Ok(deleted)
}
