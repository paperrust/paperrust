// upstream: src/documents/matching.py@v3.4.2 (existing_document_matches_workflow,
//           prefilter_documents_by_workflowtrigger: `CustomFieldQueryParser("filter_custom_field_query")`,
//           `except serializers.ValidationError` → no matches)
//
//! Evaluate `filter_custom_field_query` of the workflow triggers against the database
//! (security review final K-N1: previously never executed without an evaluator).

use std::collections::HashSet;

use chrono_tz::Tz;
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, ExprTrait, PostgresQueryBuilder, Query};
use sqlx::{PgConnection, Row};

use crate::db::ConnFilterDb;
use crate::sql::{DOC, doc_col};
use crate::{FilterContext, FilterError, cfquery};

/// Result of [`custom_field_query_matching_ids`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomFieldQueryMatch {
    /// `serializers.ValidationError` while parsing (upstream: `documents.none()` or
    /// "Invalid custom field query configuration").
    Invalid,
    /// The documents from the input that the query matches.
    Ids(HashSet<i64>),
}

/// Ids per query (bound the `IN` list).
const CHUNK: usize = 1000;

/// `Document.objects.filter(id__in=ids).annotate(**annotations).filter(custom_field_q)` over the
/// connection `conn` (thus sees an open transaction). Empty query → all `ids`.
pub async fn custom_field_query_matching_ids(
    conn: &mut PgConnection,
    query: &str,
    ids: &[i64],
    tz: Tz,
) -> Result<CustomFieldQueryMatch, FilterError> {
    let ctx = FilterContext::new(tz);
    let db = ConnFilterDb::new(conn);
    let cond = match cfquery::custom_field_query_cond(query, "filter_custom_field_query", &ctx, &db).await {
        Ok(c) => c,
        Err(FilterError::Validation(_)) => return Ok(CustomFieldQueryMatch::Invalid),
        Err(e) => return Err(e),
    };
    let conn = db.into_inner();
    let mut out = HashSet::new();
    for chunk in ids.chunks(CHUNK) {
        let mut q = Query::select()
            .column((Alias::new(DOC), Alias::new("id")))
            .from(Alias::new(DOC))
            .and_where(doc_col("id").is_in(chunk.iter().copied()))
            .to_owned();
        cond.clone().apply_to(&mut q);
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        for r in sqlx::query_with(&sql, values).fetch_all(&mut *conn).await? {
            let id = match r.try_get::<i32, _>(0) {
                Ok(v) => i64::from(v),
                Err(_) => r.try_get::<i64, _>(0)?,
            };
            out.insert(id);
        }
    }
    Ok(CustomFieldQueryMatch::Ids(out))
}
