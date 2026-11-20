//! `DocumentSelectionMixin._resolve_document_ids` for `all=true` (bulk_edit, delete, reprocess,
//! rotate, PDF methods, selection_data, bulk_download) – the document set behind "select all"
//! with an active filter (docs/STANDALONE-GAPS.md §4.1).
//!
//! upstream: src/documents/views.py@v3.4.2 (`DocumentSelectionMixin._resolve_document_ids`,
//! `_get_search_document_ids`, `_TANTIVY_SEARCH_PARAM_NAMES`), django-filter 25
//! (`FilterSet(data=dict).qs`: invalid fields are dropped from `cleaned_data`; widgets read
//! `data.get(name)`), django/forms (`CharField.to_python`: `str(value)`).
//!
//! * permitted roots: `Document.objects.filter(id__in=permitted_document_ids(user),
//!   root_document__isnull=True)` (default manager → not deleted)
//! * ORM filters: every key except the Tantivy ones, value as Python `str()` (`None` = absent);
//!   lenient [`pl_docfilter::filterset::apply_filterset_lenient`]; no `?search=`/`ordering`
//!   (those are filter backends, not FilterSet fields).
//! * Tantivy: `pl_search::views::selection_search_ids` (400 / 403 like upstream).
//! * `excluded_documents` removed; order of the (unordered upstream) result: `-created, -id`.

use pl_authz::User;
use pl_core::{ApiError, AppState, QueryParams};
use pl_docfilter::FilterContext;
use sea_query::{Expr, Order};
use serde_json::{Map, Value};

use crate::db::{ApiResult, a, id_in_list, id_list};

/// `_TANTIVY_SEARCH_PARAM_NAMES`.
pub const SEARCH_PARAM_NAMES: [&str; 4] = ["text", "title_search", "query", "more_like_id"];

const DOC: &str = pl_docfilter::DOC;

/// Python `str(value)` as the Django form field sees a dict value.
fn py_str(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.clone(),
        other => pl_core::pyrepr::repr_value(other),
    }
}

/// The ORM part of the selection filters as a `QueryDict`-like parameter list.
pub fn orm_params(filters: &Map<String, Value>) -> QueryParams {
    QueryParams::from_pairs(
        filters
            .iter()
            .filter(|(k, v)| !SEARCH_PARAM_NAMES.contains(&k.as_str()) && !v.is_null())
            .map(|(k, v)| (k.clone(), py_str(v))),
    )
}

/// Document ids of an `all=true` selection.
pub async fn resolve_all(
    state: &AppState,
    user: &User,
    filters: Option<&Map<String, Value>>,
    excluded: &[i64],
) -> ApiResult<Vec<i64>> {
    let mut q = resolve_all_query(state, user, filters, excluded).await?;
    q.order_by((a(DOC), a("created")), Order::Desc).order_by((a(DOC), a("id")), Order::Desc);
    id_list(state.db(), &q).await
}

/// The `all=true` selection as one unordered query (`SELECT "documents_document"."id" FROM
/// "documents_document" …`): same set as [`resolve_all`] – permitted, not deleted root documents
/// minus `excluded` – without shipping the ids over the wire (PERF-SCALE §12, `selection_data`
/// with "select all").
pub async fn resolve_all_query(
    state: &AppState,
    user: &User,
    filters: Option<&Map<String, Value>>,
    excluded: &[i64],
) -> ApiResult<sea_query::SelectStatement> {
    let db = state.db();
    let empty = Map::new();
    let filters = filters.unwrap_or(&empty);
    let params = orm_params(filters);
    let fctx = FilterContext::new(*state.tz());
    // L1: AST + one query, visibility directly on the row (pl_store::docsql)
    let mut q = pl_store::docsql::v1_selection(db, &fctx, user, &params).await?;
    let backend = pl_search::routes::backend(state);
    let ruser = pl_search::routes::request_user(user);
    match pl_search::views::selection_search_ids(db, backend, &ruser, filters).await {
        Ok(None) => {}
        Ok(Some(ids)) => {
            q.and_where(id_in_list(Expr::col((a(DOC), a("id"))).into(), &ids));
        }
        Err(pl_search::views::SelectionError::BadRequest(body)) => return Err(ApiError::validation(body)),
        Err(pl_search::views::SelectionError::Forbidden(msg)) => {
            return Err(ApiError::permission_denied_msg(msg));
        }
        Err(pl_search::views::SelectionError::Other(e)) => return Err(ApiError::internal(e)),
    }
    if !excluded.is_empty() {
        q.and_where(id_in_list(Expr::col((a(DOC), a("id"))).into(), excluded).not());
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_values_like_django_forms() {
        let f: Map<String, Value> = serde_json::from_str(
            r#"{"tags__id__all": "1,2", "is_tagged": 1, "has_custom_fields": true, "title_content": "x",
                "query": "q", "text": null, "correspondent__id": 2.0, "owner__id__none": null}"#,
        )
        .unwrap();
        let p = orm_params(&f);
        assert_eq!(p.get("tags__id__all"), Some("1,2"));
        assert_eq!(p.get("is_tagged"), Some("1"));
        assert_eq!(p.get("has_custom_fields"), Some("True"));
        assert_eq!(p.get("correspondent__id"), Some("2.0"));
        assert_eq!(p.get("query"), None);
        assert_eq!(p.get("owner__id__none"), None);
    }
}
