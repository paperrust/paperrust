//! The filtered document set (`filter_queryset(get_queryset())`) and the full-text branch of the
//! list.
//!
//! upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.get_queryset, filter_backends =
//! (DjangoFilterBackend, SearchFilter, DocumentsOrderingFilter, PermittedObjectsFilter),
//! UnifiedSearchViewSet.list – search branch), src/documents/serialisers.py (SearchResultSerializer),
//! src/paperless/views.py (StandardPagination.get_all_result_ids).
//!
//! Order as upstream: base condition → `pl_docfilter::apply` (DjangoFilterBackend +
//! SearchFilter) → `pl_docfilter::apply_ordering` → visibility (`PermittedObjectsFilter`,
//! pl-authz) → pagination (pl-core).

use pl_authz::User;
use pl_core::pagination::PageRequest;
use pl_core::{ApiError, AppState, QueryParams, RequestContext};
use pl_docfilter::FilterContext;
use pl_search::views::{BoxFut, DocumentSearchOutcome, FilteredDocuments, ViewError, ViewResponse};
use sea_query::{Expr, SelectStatement};
use serde_json::Value;

use crate::db::{ApiResult, a, id_in_list, id_list};
use crate::serializer::{SerializeAction, SerializeOpts, serialize_documents_db};

/// Document table of the outer query (without alias, as pl-docfilter requires).
pub const DOC: &str = pl_docfilter::DOC;

/// `self.filter_queryset(self.get_queryset())` as `SELECT documents_document.id …` with
/// full ordering. Filter errors (400/500) as upstream.
pub async fn filtered_documents(
    db: &sqlx::PgPool,
    tz: chrono_tz::Tz,
    user: &User,
    params: &QueryParams,
) -> ApiResult<SelectStatement> {
    Ok(filtered_documents_ext(db, tz, user, params).await?.0)
}

/// Like [`filtered_documents`]; second value: a substring search over the content is active
/// (then page + count in one scan, `pl_core::pagination::fetch_page_windowed`).
///
/// L1 (ARCHITECTURE-V2 §5): parameters → AST (`pl_docfilter::ast`) → one query
/// (`pl_store::docsql`, visibility directly on the row). The former path via
/// `pl_docfilter::apply` has been only a comparison baseline in the differential test
/// `crates/pl-store/tests/docquery_diff.rs` since data model stage 7.
pub async fn filtered_documents_ext(
    db: &sqlx::PgPool,
    tz: chrono_tz::Tz,
    user: &User,
    params: &QueryParams,
) -> ApiResult<(SelectStatement, bool)> {
    let fctx = FilterContext::new(tz);
    Ok(pl_store::docsql::v1_list(db, &fctx, user, params).await?)
}


/// Convert a pl-search response (400/403) into an `ApiError` with an identical body.
pub fn view_response(r: ViewResponse) -> ApiError {
    match r {
        ViewResponse::Json(status, v) => ApiError::json(status, v),
        ViewResponse::Text(status, s) => ApiError::django_text(status, s),
    }
}

fn view_error(e: ViewError) -> ApiError {
    ApiError::internal(e)
}

/// `filtered_qs` for pl-search (intersection index ∩ ORM).
struct Filtered<'a> {
    db: &'a sqlx::PgPool,
    q: &'a SelectStatement,
}

impl FilteredDocuments for Filtered<'_> {
    fn ids_in<'a>(&'a self, ids: &'a [i64]) -> BoxFut<'a, Result<Vec<i64>, ViewError>> {
        Box::pin(async move {
            // filtered_qs.filter(pk__in=ids) – in ORM order
            let mut q = self.q.clone();
            q.and_where(id_in_list(Expr::col((a(DOC), a("id"))).into(), ids));
            id_list(self.db, &q)
                .await
                .map_err(|e| ViewError::Internal(e.to_string()))
        })
    }

    fn all_ids(&self) -> BoxFut<'_, Result<Vec<i64>, ViewError>> {
        Box::pin(async move {
            let mut q = self.q.clone();
            q.clear_order_by();
            id_list(self.db, &q)
                .await
                .map_err(|e| ViewError::Internal(e.to_string()))
        })
    }
}

/// Search branch of `UnifiedSearchViewSet.list`. `q` = filtered set (already built, so that
/// filter errors come before query errors as upstream).
pub async fn search_list(
    state: &AppState,
    ctx: &RequestContext,
    user: &User,
    q: SelectStatement,
) -> ApiResult<Value> {
    // `more_like_id` (docs/STANDALONE-GAPS.md §4.2): in Rust since 2026-11-10. The order
    // of equally weighted MLT terms is already random upstream (rule
    // `more-like-this-upstream-nondeterministic`); version→root is mapped by `more_like_lookup`.
    let db = state.db();
    let params = ctx.query.pairs();
    let req = PageRequest::from_query(&ctx.query);
    let filtered = Filtered { db, q: &q };
    let backend = pl_search::routes::backend(state);
    let ruser = pl_search::routes::request_user(user);
    let page_size = usize::try_from(req.page_size).unwrap_or(usize::MAX);
    // L3: all hit IDs only for API v9 `all` and selection_data, otherwise only up to the end of the page
    let need_all_ids = ctx.version().includes_all()
        || pl_authz::owned::get_boolean(ctx.query.get("include_selection_data").unwrap_or("false"));
    let opts = pl_search::views::SearchOptions { need_all_ids, ..Default::default() };
    let outcome =
        pl_search::views::document_search_with(db, backend, &ruser, params, page_size, &filtered, opts)
            .await
            .map_err(view_error)?;
    let page = match outcome {
        DocumentSearchOutcome::Page(p) => p,
        DocumentSearchOutcome::Response(r) => return Err(view_response(r)),
    };
    // paginate_queryset(TantivyRelevanceList): Django Paginator over len(rl)
    let count = page.count();
    let info = req.resolve(i64::try_from(count).unwrap_or(i64::MAX))?;
    let start = usize::try_from(info.offset()).unwrap_or(usize::MAX);
    let stop = start.saturating_add(page_size).min(count);
    let hits = page.slice(start, stop);
    let ids: Vec<i64> = hits.iter().map(|h| h.id).collect();
    // SearchResultSerializer: Document.objects…filter(id__in=ids) without version prefetch
    let mut opts = SerializeOpts::from_query(&ctx.query, SerializeAction::List);
    opts.viewset_queryset = false;
    let docs = serialize_documents_db(db, *state.tz(), user, &ids, &opts).await?;
    // Output in the order of the ids; if a document is missing → documents[hit["id"]] KeyError → 500
    if docs.len() != hits.len() {
        return Err(ApiError::internal("search hit without document"));
    }
    let results: Vec<Value> = docs
        .into_iter()
        .zip(hits.iter())
        .map(|(d, h)| with_hit(d, h))
        .collect();
    let all = ctx
        .version()
        .includes_all()
        .then(|| page.ordered_ids.iter().copied().map(Value::from).collect());
    let mut body = pl_core::pagination::paginated_body(ctx, &info, results, all);
    if let Some(m) = body.as_object_mut() {
        m.insert("corrected_query".into(), Value::Null);
        if pl_authz::owned::get_boolean(ctx.query.get("include_selection_data").unwrap_or("false"))
        {
            // _get_selection_data_for_queryset(filtered_qs.filter(pk__in=ordered_ids))
            m.insert(
                "selection_data".into(),
                crate::views::selection_data_list(db, &page.ordered_ids).await?,
            );
        }
    }
    Ok(body)
}

/// `r["__search_hit__"] = {...}` as the last key.
fn with_hit(mut d: Value, h: &pl_search::SearchHit) -> Value {
    if let Some(m) = d.as_object_mut() {
        m.insert("__search_hit__".into(), h.to_json());
    }
    d
}
