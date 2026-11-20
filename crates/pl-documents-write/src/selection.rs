// upstream: src/documents/serialisers.py@v3.4.2 (DocumentListSerializer, DocumentSelectionSerializer)
//           src/documents/views.py@v3.4.2 (DocumentSelectionMixin._resolve_document_ids)
//
//! Document selection for bulk_edit and the operation routes.

use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value};
use sqlx::PgConnection;

use crate::drf::{self, Errors};
use crate::error::ApiError;
use crate::user::CurrentUser;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub documents: Option<Vec<i64>>,
    pub all: bool,
    pub filters: Option<Map<String, Value>>,
    pub excluded_documents: Vec<i64>,
}

/// `Document.objects.filter(id__in=ids).count() == len(ids)`
pub async fn all_documents_exist(conn: &mut PgConnection, ids: &[i64]) -> Result<bool, ApiError> {
    if ids.is_empty() {
        return Ok(true);
    }
    let q = Query::select()
        .expr(Expr::col(a("id")).count())
        .from(a("documents_document"))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let (n,): (i64,) = sqlx::query_as_with(&sql, values).fetch_one(&mut *conn).await?;
    Ok(usize::try_from(n).ok() == Some(ids.len()))
}

fn missing_docs_msg(name: &str) -> String {
    format!("Some documents in {name} don't exist or were specified twice.")
}

/// Field validation of `documents`, `all`, `filters`, `excluded_documents` (in this order).
/// `documents_required` = `DocumentListSerializer` (instead of selection).
pub async fn parse_fields(
    conn: &mut PgConnection,
    body: &Map<String, Value>,
    documents_required: bool,
    selection_fields: bool,
    errors: &mut Errors,
) -> Result<Selection, ApiError> {
    let mut sel = Selection::default();
    match body.get("documents") {
        None if documents_required => errors.add("documents", drf::msg(drf::REQUIRED)),
        None => {}
        Some(v) => match drf::int_list(v) {
            Err(e) => errors.add("documents", e),
            Ok(ids) => {
                // validate_documents → _validate_document_id_list
                if all_documents_exist(conn, &ids).await? {
                    sel.documents = Some(ids);
                } else {
                    errors.add("documents", drf::msg(missing_docs_msg("documents")));
                }
            }
        },
    }
    if !selection_fields {
        return Ok(sel);
    }
    if let Some(v) = body.get("all") {
        match drf::boolean(v) {
            Ok(b) => sel.all = b,
            Err(e) => errors.add("all", e),
        }
    }
    if let Some(v) = body.get("filters") {
        match drf::dict(v) {
            Ok(m) => sel.filters = Some(m),
            Err(e) => errors.add("filters", e),
        }
    }
    if let Some(v) = body.get("excluded_documents") {
        match drf::int_list(v) {
            Ok(ids) => sel.excluded_documents = ids,
            Err(e) => errors.add("excluded_documents", e),
        }
    }
    Ok(sel)
}

/// `DocumentSelectionSerializer.validate` → `non_field_errors`-Text.
pub async fn validate(conn: &mut PgConnection, sel: &mut Selection) -> Result<Result<(), String>, ApiError> {
    if sel.all {
        if sel.documents.is_none() {
            sel.documents = Some(Vec::new());
        }
        return Ok(Ok(()));
    }
    if !sel.excluded_documents.is_empty() {
        return Ok(Err("excluded_documents is only supported when all is true.".into()));
    }
    let Some(docs) = &sel.documents else {
        return Ok(Err("documents is required unless all is true.".into()));
    };
    if !all_documents_exist(conn, docs).await? {
        return Ok(Err(missing_docs_msg("documents")));
    }
    Ok(Ok(()))
}

/// `_resolve_document_ids`: explicit ids, or for `all=true` the filter-dict document set
/// (`pl_documents::selection_filter`, docs/STANDALONE-GAPS.md §4.1).
///
/// FINDINGS B2: `resolve_all` fetches its own pool connections via `state.db()`. So that a request never
/// holds a connection while waiting for a second one (pool empty → everyone waits until the time limit), this
/// function returns the held connection `conn` to the pool beforehand for `all=true` and fetches
/// a new one afterwards. Returns: (connection, ids).
pub async fn resolve(
    state: &pl_core::AppState,
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    user: &CurrentUser,
    sel: &Selection,
) -> Result<(sqlx::pool::PoolConnection<sqlx::Postgres>, Vec<i64>), ApiError> {
    if !sel.all {
        return Ok((conn, sel.documents.clone().unwrap_or_default()));
    }
    drop(conn);
    let ids = pl_documents::selection_filter::resolve_all(state, user, sel.filters.as_ref(), &sel.excluded_documents).await?;
    let conn = state.db().acquire().await?;
    Ok((conn, ids))
}
