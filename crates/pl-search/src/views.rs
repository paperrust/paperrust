// upstream: src/documents/views.py@v3.4.2
//   _TANTIVY_SEARCH_PARAM_NAMES, _TANTIVY_COMBINABLE_SEARCH_PARAMS, _MAX_QUERY_LENGTH,
//   _validate_tantivy_search_params, _get_tantivy_query_and_mode, _get_more_like_id,
//   UnifiedSearchViewSet.list (search branch), DocumentSelectionMixin._get_search_document_ids,
//   SearchAutoCompleteView.get, GlobalSearchView.get
// src/documents/serialisers.py (SearchResultSerializer: `__search_hit__`)
// src/documents/search/_backend.py (TantivyRelevanceList)
//
// Framework-light: the functions take the already authenticated user and the
// raw query parameters and return a `ViewResponse`. Mounting (auth, AppState)
// waits for pl-core/pl-authz (see lib.rs).

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::backend::{SearchBackend, SearchHit, SearchMode, is_sortable_field};
use crate::db::{self, MoreLikeLookup, RequestUser};
use crate::error::SearchError;

pub const SEARCH_PARAM_NAMES: [&str; 4] = ["text", "title_search", "query", "more_like_id"];
/// `_MAX_QUERY_LENGTH`.
pub const MAX_QUERY_LENGTH: usize = 4096;
/// `_TANTIVY_INTERSECT_THRESHOLD`.
pub const INTERSECT_THRESHOLD: usize = 5_000;

pub type Params = [(String, String)];
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Django `QueryDict.__getitem__`/`.get`: the LAST value of a repeated key.
pub fn param<'a>(params: &'a Params, key: &str) -> Option<&'a str> {
    params.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

pub fn has_param(params: &Params, key: &str) -> bool {
    params.iter().any(|(k, _)| k == key)
}

/// Python `str.strip()` whitespace set.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Python `int(str)`: surrounding whitespace, optional sign, ASCII digits with
/// single underscores between digits. (Non-ASCII decimal digits, which Python
/// also accepts, are rejected here — see Known differences.)
pub fn py_int(s: &str) -> Option<i128> {
    let s = py_strip(s);
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)?;
        v = v.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    Some(if neg { -v } else { v })
}

/// A finished response (status + body), shaped like Django/DRF.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewResponse {
    /// DRF `Response(data)` – application/json
    Json(StatusCode, Value),
    /// Django `HttpResponseBadRequest(..)` / `HttpResponseForbidden(..)` –
    /// text/html; charset=utf-8 with the plain message as body.
    Text(StatusCode, String),
}

impl IntoResponse for ViewResponse {
    fn into_response(self) -> Response {
        match self {
            ViewResponse::Json(status, v) => (status, axum::Json(v)).into_response(),
            ViewResponse::Text(status, body) => (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response(),
        }
    }
}

/// Errors that are not plain responses (become 500 upstream).
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Search(SearchError),
    #[error("internal: {0}")]
    Internal(String),
}

fn bad_request_text(msg: impl Into<String>) -> ViewResponse {
    ViewResponse::Text(StatusCode::BAD_REQUEST, msg.into())
}

fn forbidden_text(msg: impl Into<String>) -> ViewResponse {
    ViewResponse::Text(StatusCode::FORBIDDEN, msg.into())
}

/// DRF `ValidationError({"query": [...]})`.
fn query_error_response(e: &SearchError) -> Option<ViewResponse> {
    e.messages().map(|m| {
        ViewResponse::Json(StatusCode::BAD_REQUEST, json!({ "query": m.iter().map(|s| Value::String(s.clone())).collect::<Vec<_>>() }))
    })
}

/// `_validate_tantivy_search_params`: Err = 400 `{"detail": "..."}`.
pub fn validate_search_params(active: &[&str]) -> Result<(), ViewResponse> {
    if active.len() > 1 {
        let set: HashSet<&str> = active.iter().copied().collect();
        let ok = set == HashSet::from(["text", "query"]) || set == HashSet::from(["title_search", "query"]);
        if !ok {
            return Err(ViewResponse::Json(
                StatusCode::BAD_REQUEST,
                json!({"detail": "Specify only one of text, title_search, query, or more_like_id."}),
            ));
        }
    }
    Ok(())
}

/// Active search params in `_TANTIVY_SEARCH_PARAM_NAMES` order.
pub fn active_search_params(params: &Params) -> Vec<&'static str> {
    SEARCH_PARAM_NAMES.iter().copied().filter(|p| has_param(params, p)).collect()
}

/// `UnifiedSearchViewSet._is_search_request`.
pub fn is_search_request(params: &Params) -> bool {
    !active_search_params(params).is_empty()
}

/// `_get_tantivy_query_and_mode` result.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryAndMode {
    pub raw: String,
    pub mode: SearchMode,
    pub filter_query: Option<String>,
}

/// `_get_tantivy_query_and_mode(params)`; lookup by key returns the value as
/// Python `str(...)` would. `QueryTooLongError` -> `SearchError::Query`.
pub fn get_query_and_mode(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<QueryAndMode>, SearchError> {
    let (raw, mode) = if let Some(v) = lookup("text") {
        (v, SearchMode::Text)
    } else if let Some(v) = lookup("title_search") {
        (v, SearchMode::Title)
    } else if let Some(v) = lookup("query") {
        (v, SearchMode::Query)
    } else {
        return Ok(None);
    };
    let filter_query = if mode != SearchMode::Query { lookup("query") } else { None };
    for value in [Some(&raw), filter_query.as_ref()].into_iter().flatten() {
        // Python len() counts code points
        let len = value.chars().count();
        if len > MAX_QUERY_LENGTH {
            return Err(SearchError::too_long(len, MAX_QUERY_LENGTH));
        }
    }
    Ok(Some(QueryAndMode { raw, mode, filter_query }))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, SearchError> + Send + 'static,
) -> Result<Result<T, SearchError>, ViewError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ViewError::Internal(format!("search task failed: {e}")))
}

// ---------------------------------------------------------------------------
// /api/search/autocomplete/  (SearchAutoCompleteView.get)
// ---------------------------------------------------------------------------

/// GET /api/search/autocomplete/. Permission classes
/// (IsAuthenticated, ViewDocumentsPermissions) are checked by the caller via
/// [`autocomplete_permitted`].
pub async fn autocomplete_view(backend: Arc<SearchBackend>, user: &RequestUser, params: &Params) -> Result<ViewResponse, ViewError> {
    let Some(term) = param(params, "term") else {
        return Ok(bad_request_text("Term required"));
    };
    let term = py_strip(term).to_string();
    let limit = match param(params, "limit") {
        Some(raw) => match py_int(raw) {
            // int() ValueError is unhandled upstream -> 500
            None => {
                return Err(ViewError::Internal(format!("invalid literal for int() with base 10: {raw:?}")));
            }
            Some(l) if l <= 0 => return Ok(bad_request_text("Invalid limit")),
            Some(l) => usize::try_from(l).unwrap_or(usize::MAX),
        },
        None => 10,
    };
    // backend.autocomplete filters unless user.is_superuser
    let search_user = if user.is_superuser { None } else { Some(crate::SearchUser { id: user.id, group_ids: user.group_ids.clone() }) };
    let words = blocking(move || backend.autocomplete(&term, limit, search_user.as_ref())).await?.map_err(ViewError::Search)?;
    Ok(ViewResponse::Json(StatusCode::OK, Value::Array(words.into_iter().map(Value::String).collect())))
}

/// `ViewDocumentsPermissions` (+ IsAuthenticated, done by the caller).
pub fn autocomplete_permitted(user: &RequestUser) -> bool {
    user.has_perm("documents.view_document")
}

// ---------------------------------------------------------------------------
// /api/search/  (GlobalSearchView.get)
// ---------------------------------------------------------------------------

/// Object sections of the global search other crates serialize
/// (TagSerializer, UserSerializer, ...). Each returns the serialized objects
/// matching `name__icontains=query` (`username` for users), already
/// restricted to what the user may see and the section's model permission,
/// and limited to `limit` rows. Sections without provider are empty lists.
pub trait ObjectSearchProvider: Send + Sync {
    fn search<'a>(&'a self, user: &'a RequestUser, query: &'a str, limit: usize) -> BoxFut<'a, Result<Vec<Value>, ViewError>>;
}

/// Sections in response order after `documents`.
pub const GLOBAL_SECTIONS: [&str; 11] = [
    "saved_views",
    "tags",
    "correspondents",
    "document_types",
    "storage_paths",
    "users",
    "groups",
    "mail_rules",
    "mail_accounts",
    "workflows",
    "custom_fields",
];

#[derive(Default, Clone)]
pub struct GlobalSearchProviders {
    /// keyed by section name in [`GLOBAL_SECTIONS`]
    pub providers: std::collections::HashMap<&'static str, Arc<dyn ObjectSearchProvider>>,
}

const OBJECT_LIMIT: usize = 3;

/// GET /api/search/ (IsAuthenticated is checked by the caller).
pub async fn global_search_view(
    db: &PgPool,
    backend: Arc<SearchBackend>,
    user: &RequestUser,
    params: &Params,
    providers: &GlobalSearchProviders,
) -> Result<ViewResponse, ViewError> {
    let Some(query) = param(params, "query") else {
        return Ok(bad_request_text("Query required"));
    };
    let qlen = query.chars().count();
    if qlen < 3 {
        return Ok(bad_request_text("Query must be at least 3 characters"));
    }
    if qlen > MAX_QUERY_LENGTH {
        return Ok(bad_request_text(format!("Query must be at most {MAX_QUERY_LENGTH} characters")));
    }
    // request.query_params.get("db_only", False): any non-empty string is truthy
    let db_only = param(params, "db_only").is_some_and(|v| !v.is_empty());

    let mut docs: Vec<db::DocTitleRow> = Vec::new();
    if user.has_perm("documents.view_document") {
        if db_only {
            docs = db::docs_title_icontains(db, user, query, OBJECT_LIMIT as i64).await?;
        } else {
            let search_user = user.search_user();
            let q = query.to_string();
            let matching_ids =
                blocking(move || backend.search_ids(&q, search_user.as_ref(), None, false, SearchMode::Text, None, Some(OBJECT_LIMIT * 3)))
                    .await?
                    .map_err(ViewError::Search)?;
            let rows = db::docs_in_bulk(db, user, &matching_ids).await?;
            let by_id: std::collections::HashMap<i64, db::DocTitleRow> = rows.into_iter().map(|r| (r.id, r)).collect();
            docs = matching_ids.iter().filter_map(|id| by_id.get(id).cloned()).take(OBJECT_LIMIT).collect();
        }
    }

    let mut sections: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for name in GLOBAL_SECTIONS {
        let items = match providers.providers.get(name) {
            Some(p) => {
                let mut v = p.search(user, query, OBJECT_LIMIT).await?;
                v.truncate(OBJECT_LIMIT);
                v
            }
            None => Vec::new(),
        };
        sections.push((name, items));
    }

    let total = docs.len() + sections.iter().map(|(_, v)| v.len()).sum::<usize>();
    let mut out = Map::new();
    out.insert("total".into(), json!(total));
    out.insert(
        "documents".into(),
        Value::Array(
            docs.iter()
                .map(|d| {
                    let mut m = Map::new();
                    m.insert("id".into(), json!(d.id));
                    m.insert("title".into(), json!(d.title));
                    m.insert("created".into(), json!(d.created.format("%Y-%m-%d").to_string()));
                    Value::Object(m)
                })
                .collect(),
        ),
    );
    for (name, items) in sections {
        out.insert(name.into(), Value::Array(items));
    }
    Ok(ViewResponse::Json(StatusCode::OK, Value::Object(out)))
}

// ---------------------------------------------------------------------------
// GET /api/documents/?query=|text=|title_search=|more_like_id=
// (UnifiedSearchViewSet.list, search branch) – called by pl-documents
// ---------------------------------------------------------------------------

/// What pl-documents must provide: the ORM side of the search list
/// (`self.filter_queryset(self.get_queryset())`, i.e. the document list
/// queryset with permission filter, DocumentFilterSet and `ordering`).
pub trait FilteredDocuments: Send + Sync {
    /// `filtered_qs.filter(pk__in=ids).values_list("pk")` in the queryset's
    /// ORDER (ordering param / default ordering applied).
    fn ids_in<'a>(&'a self, ids: &'a [i64]) -> BoxFut<'a, Result<Vec<i64>, ViewError>>;
    /// `filtered_qs.values_list("pk")` (order irrelevant).
    fn all_ids(&self) -> BoxFut<'_, Result<Vec<i64>, ViewError>>;
}

/// `SearchResultPage` + `TantivyRelevanceList`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResultPage {
    /// All hits in order – or, on the L3 fast path without [`SearchOptions::need_all_ids`], only
    /// the hits up to the end of the requested page (`count` holds the total then).
    pub ordered_ids: Vec<i64>,
    pub hits: Vec<SearchHit>,
    pub page_offset: usize,
    /// `len(rl)`; `None` = `ordered_ids.len()`.
    pub total: Option<usize>,
}

impl SearchResultPage {
    /// `len(rl)` – the paginator `count`.
    pub fn count(&self) -> usize {
        self.total.unwrap_or(self.ordered_ids.len())
    }

    /// `ordered_ids` holds every hit (always true unless the L3 fast path served a window).
    pub fn has_all_ids(&self) -> bool {
        self.ordered_ids.len() == self.count()
    }

    /// `TantivyRelevanceList.__getitem__(slice(start, stop))`, as DRF's
    /// paginator slices the page it actually serves. Matches the prefetched
    /// hits only when `start == page_offset`; otherwise stub hits (score 0,
    /// no highlights), exactly like upstream (e.g. `page=last`).
    pub fn slice(&self, start: usize, stop: usize) -> Vec<SearchHit> {
        if start == self.page_offset {
            let n = stop.saturating_sub(start).min(self.hits.len());
            return self.hits[..n].to_vec();
        }
        let end = stop.min(self.ordered_ids.len());
        if start >= end {
            return Vec::new();
        }
        self.ordered_ids[start..end].iter().enumerate().map(|(i, id)| SearchHit::stub(*id, start + i + 1)).collect()
    }
}

/// Outcome of the search branch of the list.
#[derive(Debug)]
pub enum DocumentSearchOutcome {
    /// Paginate this (count = `ordered_ids.len()`), serialize the page's hits
    /// with the DocumentSerializer and add `"__search_hit__": hit.to_json()`
    /// as last key of each document; then add `"corrected_query": null` and,
    /// with `include_selection_data`, `"selection_data"` over
    /// `filtered_qs.filter(pk__in=ordered_ids)`.
    Page(SearchResultPage),
    /// Respond with this as-is (400 / 403).
    Response(ViewResponse),
}

/// `int(request.query_params.get("page", 1))`, invalid -> 1.
pub fn page_num_param(params: &Params) -> usize {
    match param(params, "page") {
        None => 1,
        // page <= 0 -> 0 (DRF answers 404 "Invalid page." for it anyway)
        Some(raw) => py_int(raw).map(|v| usize::try_from(v).unwrap_or(0)).unwrap_or(1),
    }
}

// ---------------------------------------------------------------------------
// L3: PAPERRUST_SEARCH_ENGINE (ARCHITECTURE-V2 §5)
// ---------------------------------------------------------------------------

/// Switch for the search path (`PAPERRUST_SEARCH_ENGINE=legacy|native`).
pub const ENGINE_VAR: &str = "PAPERRUST_SEARCH_ENGINE";

/// Search path of the full-text document list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// upstream flow 1:1: rank all hits, intersect with all visible IDs (SQL).
    Legacy,
    /// L3: permissions in the index, rank only top N, hit count from tantivy, SQL only for the page –
    /// as long as the index for permissions/trash is demonstrably current; otherwise like `Legacy`.
    Native,
}

/// Default (since 2026-11-10 `native`: differential test and parity runs without deviation,
/// docs/contract/pl-search.md "L3").
pub const DEFAULT_ENGINE: Engine = Engine::Native;

/// `legacy`/`native` (case-insensitive), otherwise [`DEFAULT_ENGINE`].
pub fn parse_engine(v: Option<&str>) -> Engine {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("legacy") => Engine::Legacy,
        Some("native") => Engine::Native,
        _ => DEFAULT_ENGINE,
    }
}

/// Current search path (live, read per request).
pub fn engine() -> Engine {
    #[cfg(feature = "routes")]
    {
        parse_engine(pl_core::config::var(ENGINE_VAR).as_deref())
    }
    #[cfg(not(feature = "routes"))]
    {
        parse_engine(std::env::var(ENGINE_VAR).ok().as_deref())
    }
}

/// What the caller of [`document_search_with`] needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// `SearchResultPage::ordered_ids` must contain all hits (API v9 `all`,
    /// `include_selection_data`). Without it: on the fast path only up to the end of the page.
    pub need_all_ids: bool,
    /// Force the search path (differential test); `None` = [`engine()`].
    pub engine: Option<Engine>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { need_all_ids: true, engine: None }
    }
}

/// Parameters that do NOT restrict the ORM set (`filter_queryset(get_queryset())`): search,
/// sorting, pagination, presentation. Every other key (also unknown ones) → exact path.
const NON_FILTER_PARAMS: &[&str] = &[
    "query",
    "text",
    "title_search",
    "more_like_id",
    "ordering",
    "page",
    "page_size",
    "truncate_content",
    "include_selection_data",
    "fields",
    "full_perms",
    "format",
];

/// Fast path possible? (parameters without filters, active user, index order.)
fn fast_path_params(params: &Params) -> bool {
    params.iter().all(|(k, _)| NON_FILTER_PARAMS.contains(&k.as_str()))
}

/// `page` is a positive integer (then the delivered page starts at `page_offset`).
fn page_param_is_number(params: &Params) -> bool {
    match param(params, "page") {
        None => true,
        Some(raw) => py_int(raw).is_some_and(|v| v >= 1),
    }
}

/// UnifiedSearchViewSet.list search branch.
///
/// `page_size` = `self.paginator.get_page_size(request) or self.paginator.page_size`
/// (computed by pl-documents with pl-core's StandardPagination).
/// Call only when [`is_search_request`] is true.
pub async fn document_search(
    db: &PgPool,
    backend: Arc<SearchBackend>,
    user: &RequestUser,
    params: &Params,
    page_size: usize,
    filtered: &dyn FilteredDocuments,
) -> Result<DocumentSearchOutcome, ViewError> {
    document_search_with(db, backend, user, params, page_size, filtered, SearchOptions::default()).await
}

/// Like [`document_search`]; `opts.need_all_ids = false` allows the L3 path to rank only the hits up
/// to the end of the page (`ordered_ids` is then a prefix, `count()` the total).
pub async fn document_search_with(
    db: &PgPool,
    backend: Arc<SearchBackend>,
    user: &RequestUser,
    params: &Params,
    page_size: usize,
    filtered: &dyn FilteredDocuments,
    opts: SearchOptions,
) -> Result<DocumentSearchOutcome, ViewError> {
    // parse_search_params
    let active = active_search_params(params);
    if let Err(resp) = validate_search_params(&active) {
        return Ok(DocumentSearchOutcome::Response(resp));
    }
    let ordering = param(params, "ordering").unwrap_or("");
    let sort_reverse = ordering.starts_with('-');
    let sort_field_name: Option<String> = {
        let s = ordering.trim_start_matches('-');
        (!s.is_empty()).then(|| s.to_string())
    };
    let use_tantivy_sort = match sort_field_name.as_deref() {
        None => true,
        Some("score") => true,
        Some(f) => is_sortable_field(f),
    };
    let page_num = page_num_param(params);
    // Python: (page_num - 1) * page_size; negative offsets slice from the end
    // upstream (page=0 / negative) – clamp to 0, see Known differences.
    let page_offset = page_num.saturating_sub(1).saturating_mul(page_size);

    let search_user = user.search_user();

    let result = if has_param(params, "more_like_id") {
        // run_more_like_this
        let lookup_user = if user.is_superuser { None } else { Some(user) };
        let raw = param(params, "more_like_id").unwrap_or("");
        let target = match py_int(raw).and_then(|v| i64::try_from(v).ok()) {
            None => MoreLikeLookup::Invalid,
            Some(id) => db::more_like_lookup(db, id, lookup_user).await?,
        };
        let doc_id = match target {
            MoreLikeLookup::Ok(id) => id,
            MoreLikeLookup::Invalid => {
                return Ok(DocumentSearchOutcome::Response(forbidden_text("Invalid more_like_id")));
            }
            MoreLikeLookup::Forbidden => {
                return Ok(DocumentSearchOutcome::Response(forbidden_text("Insufficient permissions.")));
            }
        };
        let kind = NativeKind::MoreLike { backend: backend.clone(), doc_id };
        let fast = match native_window(db, user, params, page_offset, page_size, &opts, filtered, kind).await {
            Ok(f) => f,
            Err(NativeError::Query(e)) => return Err(ViewError::Search(e)),
            Err(NativeError::View(e)) => return Err(e),
        };
        if let Some(fast) = fast {
            let hits = fast.page_ids.iter().enumerate().map(|(i, id)| SearchHit::stub(*id, page_offset + i + 1)).collect();
            return Ok(DocumentSearchOutcome::Page(SearchResultPage {
                ordered_ids: fast.window.ids,
                hits,
                page_offset,
                total: Some(fast.window.total),
            }));
        }
        let b = backend.clone();
        let su = search_user.clone();
        let all_ids = blocking(move || b.more_like_this_ids(doc_id, su.as_ref(), None)).await?.map_err(ViewError::Search)?;
        let ordered_ids = intersect_and_order(&all_ids, filtered, true).await?;
        let page_ids: Vec<i64> = ordered_ids.iter().skip(page_offset).take(page_size).copied().collect();
        let hits = page_ids.iter().enumerate().map(|(i, id)| SearchHit::stub(*id, page_offset + i + 1)).collect();
        SearchResultPage { ordered_ids, hits, page_offset, total: None }
    } else {
        // run_text_search
        let qm = match get_query_and_mode(|k| param(params, k).map(str::to_string)) {
            Ok(Some(qm)) => qm,
            Ok(None) => {
                return Err(ViewError::Internal("no search parameter".into()));
            }
            Err(e) => {
                return match query_error_response(&e) {
                    Some(r) => Ok(DocumentSearchOutcome::Response(r)),
                    None => Err(ViewError::Search(e)),
                };
            }
        };
        let is_score_sort = sort_field_name.as_deref() == Some("score");
        let sort_field = if !use_tantivy_sort || is_score_sort { None } else { sort_field_name.clone() };
        let fast = if is_score_sort && !sort_reverse {
            // ordering=score: relevance reversed → page 1 = weakest hits, exact path
            None
        } else if use_tantivy_sort {
            let kind = NativeKind::Text { backend: backend.clone(), qm: qm.clone(), sort_field: sort_field.clone(), sort_reverse };
            match native_window(db, user, params, page_offset, page_size, &opts, filtered, kind).await {
                Ok(f) => f,
                Err(NativeError::Query(e)) => {
                    return match query_error_response(&e) {
                        Some(r) => Ok(DocumentSearchOutcome::Response(r)),
                        None => Err(ViewError::Search(e)),
                    };
                }
                Err(NativeError::View(e)) => return Err(e),
            }
        } else {
            None
        };
        if let Some(fast) = fast {
            let page_ids = fast.page_ids;
            let b = backend.clone();
            let hits = match blocking(move || b.highlight_hits(&qm.raw, &page_ids, qm.mode, page_offset + 1)).await? {
                Ok(h) => h,
                Err(e) => {
                    return match query_error_response(&e) {
                        Some(r) => Ok(DocumentSearchOutcome::Response(r)),
                        None => Err(ViewError::Search(e)),
                    };
                }
            };
            return Ok(DocumentSearchOutcome::Page(SearchResultPage {
                ordered_ids: fast.window.ids,
                hits,
                page_offset,
                total: Some(fast.window.total),
            }));
        }
        let b = backend.clone();
        let su = search_user.clone();
        let qm2 = qm.clone();
        let all_ids = match blocking(move || {
            b.search_ids(&qm2.raw, su.as_ref(), sort_field.as_deref(), sort_reverse, qm2.mode, qm2.filter_query.as_deref(), None)
        })
        .await?
        {
            Ok(ids) => ids,
            Err(e) => {
                return match query_error_response(&e) {
                    Some(r) => Ok(DocumentSearchOutcome::Response(r)),
                    None => Err(ViewError::Search(e)),
                };
            }
        };
        let mut ordered_ids = intersect_and_order(&all_ids, filtered, use_tantivy_sort).await?;
        if is_score_sort && !sort_reverse {
            ordered_ids.reverse();
        }
        let page_ids: Vec<i64> = ordered_ids.iter().skip(page_offset).take(page_size).copied().collect();
        let b = backend.clone();
        let hits = match blocking(move || b.highlight_hits(&qm.raw, &page_ids, qm.mode, page_offset + 1)).await? {
            Ok(h) => h,
            Err(e) => {
                return match query_error_response(&e) {
                    Some(r) => Ok(DocumentSearchOutcome::Response(r)),
                    None => Err(ViewError::Search(e)),
                };
            }
        };
        SearchResultPage { ordered_ids, hits, page_offset, total: None }
    };
    Ok(DocumentSearchOutcome::Page(result))
}

/// What the L3 path is to rank.
enum NativeKind {
    Text { backend: Arc<SearchBackend>, qm: QueryAndMode, sort_field: Option<String>, sort_reverse: bool },
    MoreLike { backend: Arc<SearchBackend>, doc_id: i64 },
}

enum NativeError {
    Query(SearchError),
    View(ViewError),
}

impl From<ViewError> for NativeError {
    fn from(e: ViewError) -> Self {
        NativeError::View(e)
    }
}

impl From<sqlx::Error> for NativeError {
    fn from(e: sqlx::Error) -> Self {
        NativeError::View(ViewError::Db(e))
    }
}

struct NativeResult {
    window: crate::backend::RankedWindow,
    page_ids: Vec<i64>,
}

/// L3 path (`PAPERRUST_SEARCH_ENGINE=native`), result-equal to the upstream flow
/// "rank all hits → intersect with `filter_queryset(get_queryset())`", if
///
/// 1. the ORM set consists only of the base (root documents outside the trash) + visibility
///    (no filter parameters, [`NON_FILTER_PARAMS`]),
/// 2. the user is active (superuser: no restriction, otherwise the permission disjunction in the index
///    = `PermittedObjectsFilter`: owner, no owner, direct `view_document` grants
///    to the user/current groups),
/// 3. the index is current for permissions and liveness: no open row in
///    `paperrust_search_dirty` (triggers on owner/trash/versions/guardian, drained
///    by the drainer in the server, pl-indexer `dirty`).
///
/// Then every index hit (with permission filter) is in the ORM set, so the intersection is the
/// index hits themselves: hit count = `Count`, page = top (offset + size). The page is
/// nevertheless checked via SQL against the ORM set; if a document is missing (index stale, race
/// with a change), the exact path applies. `Ok(None)` = exact path.
#[allow(clippy::too_many_arguments)]
async fn native_window(
    db: &PgPool,
    user: &RequestUser,
    params: &Params,
    page_offset: usize,
    page_size: usize,
    opts: &SearchOptions,
    filtered: &dyn FilteredDocuments,
    kind: NativeKind,
) -> Result<Option<NativeResult>, NativeError> {
    if opts.engine.unwrap_or_else(engine) != Engine::Native || !user.is_active || !fast_path_params(params) {
        return Ok(None);
    }
    if !db::search_index_clean(db).await {
        return Ok(None);
    }
    let window = if opts.need_all_ids || !page_param_is_number(params) { None } else { Some(page_offset.saturating_add(page_size)) };
    let su = user.search_user();
    let res = match kind {
        NativeKind::Text { backend, qm, sort_field, sort_reverse } => blocking(move || {
            backend.search_window(&qm.raw, su.as_ref(), sort_field.as_deref(), sort_reverse, qm.mode, qm.filter_query.as_deref(), window)
        })
        .await?
        .map_err(NativeError::Query)?,
        NativeKind::MoreLike { backend, doc_id } => blocking(move || backend.more_like_this_window(doc_id, su.as_ref(), window))
            .await?
            .map_err(|e| NativeError::View(ViewError::Search(e)))?,
    };
    let page_ids: Vec<i64> = res.ids.iter().skip(page_offset).take(page_size).copied().collect();
    if !page_ids.is_empty() {
        // filtered_qs.filter(pk__in=page) must contain the whole page
        let found: HashSet<i64> = filtered.ids_in(&page_ids).await?.into_iter().collect();
        if page_ids.iter().any(|id| !found.contains(id)) {
            tracing::info!(
                target: "paperless.search",
                "search index not in sync with the database for this page; using the exact path"
            );
            return Ok(None);
        }
    }
    Ok(Some(NativeResult { window: res, page_ids }))
}

/// `intersect_and_order`.
async fn intersect_and_order(all_ids: &[i64], filtered: &dyn FilteredDocuments, use_tantivy_sort: bool) -> Result<Vec<i64>, ViewError> {
    if all_ids.is_empty() {
        return Ok(Vec::new());
    }
    if use_tantivy_sort {
        let visible: HashSet<i64> = if all_ids.len() <= INTERSECT_THRESHOLD {
            filtered.ids_in(all_ids).await?.into_iter().collect()
        } else {
            filtered.all_ids().await?.into_iter().collect()
        };
        return Ok(all_ids.iter().copied().filter(|id| visible.contains(id)).collect());
    }
    filtered.ids_in(all_ids).await
}

// ---------------------------------------------------------------------------
// DocumentSelectionMixin._get_search_document_ids (bulk_edit & co. "all" filters)
// ---------------------------------------------------------------------------

/// Python `str(value)` of a JSON-decoded value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Error of the selection path (DRF exceptions, rendered as JSON).
#[derive(Debug)]
pub enum SelectionError {
    /// 400 body
    BadRequest(Value),
    /// 403 `{"detail": msg}` (PermissionDenied is not caught on this path)
    Forbidden(&'static str),
    Other(ViewError),
}

/// `_get_search_document_ids(user=..., filters=...)`: `Ok(None)` when no
/// search filter is present.
pub async fn selection_search_ids(
    db: &PgPool,
    backend: Arc<SearchBackend>,
    user: &RequestUser,
    filters: &Map<String, Value>,
) -> Result<Option<Vec<i64>>, SelectionError> {
    let active: Vec<&str> = SEARCH_PARAM_NAMES.iter().copied().filter(|k| filters.contains_key(*k)).collect();
    let Some(first) = active.first().copied() else {
        return Ok(None);
    };
    if let Err(ViewResponse::Json(_, body)) = validate_search_params(&active) {
        return Err(SelectionError::BadRequest(body));
    }
    let search_user = user.search_user();
    if first == "more_like_id" {
        // int(value): int, bool and float (truncated) and numeric strings work
        let id = match filters.get("more_like_id") {
            Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
            Some(Value::Bool(b)) => Some(i64::from(*b)),
            Some(Value::String(s)) => py_int(s).and_then(|v| i64::try_from(v).ok()),
            _ => None,
        };
        let lookup = match id {
            None => MoreLikeLookup::Invalid,
            Some(id) => db::more_like_lookup(db, id, Some(user)).await.map_err(|e| SelectionError::Other(e.into()))?,
        };
        let doc_id = match lookup {
            MoreLikeLookup::Ok(id) => id,
            MoreLikeLookup::Invalid => return Err(SelectionError::Forbidden("Invalid more_like_id")),
            MoreLikeLookup::Forbidden => {
                return Err(SelectionError::Forbidden("Insufficient permissions."));
            }
        };
        let ids = blocking(move || backend.more_like_this_ids(doc_id, search_user.as_ref(), None))
            .await
            .map_err(SelectionError::Other)?
            .map_err(|e| SelectionError::Other(ViewError::Search(e)))?;
        return Ok(Some(ids));
    }
    let qm = match get_query_and_mode(|k| filters.get(k).map(py_str)) {
        Ok(Some(qm)) => qm,
        Ok(None) => return Ok(None),
        Err(e) => return Err(selection_query_error(e)),
    };
    let ids = blocking(move || backend.search_ids(&qm.raw, search_user.as_ref(), None, false, qm.mode, qm.filter_query.as_deref(), None))
        .await
        .map_err(SelectionError::Other)?
        .map_err(selection_query_error)?;
    Ok(Some(ids))
}

fn selection_query_error(e: SearchError) -> SelectionError {
    match query_error_response(&e) {
        Some(ViewResponse::Json(_, body)) => SelectionError::BadRequest(body),
        _ => SelectionError::Other(ViewError::Search(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn python_int() {
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("+3"), Some(3));
        assert_eq!(py_int("-3"), Some(-3));
        assert_eq!(py_int("1_000"), Some(1000));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("abc"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("1.5"), None);
    }

    #[test]
    fn validation() {
        assert!(validate_search_params(&["query"]).is_ok());
        assert!(validate_search_params(&["text", "query"]).is_ok());
        assert!(validate_search_params(&["title_search", "query"]).is_ok());
        let err = validate_search_params(&["text", "title_search"]).unwrap_err();
        assert_eq!(
            err,
            ViewResponse::Json(
                StatusCode::BAD_REQUEST,
                json!({"detail": "Specify only one of text, title_search, query, or more_like_id."})
            )
        );
        assert!(validate_search_params(&["text", "title_search", "query"]).is_err());
    }

    #[test]
    fn query_and_mode() {
        let params = p(&[("text", "bank"), ("query", "created:[-1 week to now]")]);
        let qm = get_query_and_mode(|k| param(&params, k).map(str::to_string)).unwrap().unwrap();
        assert_eq!(qm.mode, SearchMode::Text);
        assert_eq!(qm.filter_query.as_deref(), Some("created:[-1 week to now]"));
        let long = "a".repeat(4097);
        let params = p(&[("query", &long)]);
        let e = get_query_and_mode(|k| param(&params, k).map(str::to_string)).unwrap_err();
        assert_eq!(
            e.messages().unwrap(),
            ["The search query is too long (4097 characters). The maximum allowed length is 4096 characters.".to_string()]
        );
    }

    #[test]
    fn last_value_wins() {
        let params = p(&[("term", "a"), ("term", "b")]);
        assert_eq!(param(&params, "term"), Some("b"));
    }

    #[test]
    fn relevance_list_slicing() {
        let page = SearchResultPage {
            ordered_ids: vec![5, 6, 7, 8],
            hits: vec![SearchHit { score: 1.5, ..SearchHit::stub(7, 3) }],
            page_offset: 2,
            total: None,
        };
        assert_eq!(page.slice(2, 4).len(), 1);
        assert_eq!(page.slice(2, 4)[0].score, 1.5);
        let stub = page.slice(0, 2);
        assert_eq!(stub, vec![SearchHit::stub(5, 1), SearchHit::stub(6, 2)]);
    }

    #[test]
    fn search_hit_json() {
        let mut h = SearchHit::stub(1, 1);
        h.score = 0.5;
        h.content_highlight = Some("<b>x</b>".into());
        assert_eq!(
            serde_json::to_string(&h.to_json()).unwrap(),
            r#"{"score":0.5,"highlights":"<b>x</b>","note_highlights":null,"rank":1}"#
        );
    }

    #[test]
    fn python_str() {
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(5)), "5");
        assert_eq!(py_str(&json!("x")), "x");
    }
}
