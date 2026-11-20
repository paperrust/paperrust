//! pl-docfilter – filtering and ordering of the document list (`/api/documents/`).
//!
//! upstream: src/documents/filters.py@v3.4.2 (DocumentFilterSet, CustomFieldQueryParser,
//! DocumentsOrderingFilter), src/documents/views.py@v3.4.2 (DocumentViewSet.filter_backends,
//! search_fields, ordering_fields, get_queryset)
//!
//! Contract: docs/contract/pl-docfilter.md. This crate provides **no routes**, but the
//! building blocks with which pl-documents filters and sorts the list:
//!
//! ```ignore
//! let mut q = Query::select().column((DOC, "id")).from(Alias::new("documents_document"))
//!     .and_where(root_document_id IS NULL AND deleted_at IS NULL).to_owned();
//! pl_docfilter::apply(&mut q, &params, &user, &ctx, &pool).await?;          // DjangoFilterBackend + SearchFilter
//! pl_docfilter::apply_ordering(&mut q, params.get("ordering"), &pool).await?; // DocumentsOrderingFilter
//! // afterwards: visibility (PermittedObjectsFilter, pl-authz), pagination
//! ```
//!
//! The outer query must read `FROM "documents_document"` without an alias (see [`sql::DOC`]).

pub mod ast;
pub mod cfquery;
pub mod dates;
mod relative_dp;
mod relative_dp_data;
pub mod db;
pub mod error;
pub mod filterset;
pub mod ordering;
pub mod params;
pub mod perms;
pub mod pyval;
pub mod search;
pub mod sql;
pub mod textfold;
pub mod trigger;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_core::ApiRoute;
use sea_query::SelectStatement;

pub use db::{ConnFilterDb, CustomFieldDef, FilterDb};
pub use trigger::{CustomFieldQueryMatch, custom_field_query_matching_ids};
pub use error::FilterError;
pub use params::QueryParams;
pub use sql::DOC;

/// The requesting (authenticated) user.
pub use pl_authz::User;

/// Runtime context (configuration and reference time).
#[derive(Debug, Clone)]
pub struct FilterContext {
    /// `PAPERLESS_TIME_ZONE` (Django `TIME_ZONE`, active time zone)
    pub tz: Tz,
    /// Reference time for relative date expressions (`timezone.localtime()` in upstream)
    pub now: DateTime<Utc>,
    /// `PAPERLESS_DATE_PARSER_LANGUAGES` (None = all languages)
    pub date_parser_languages: Option<Vec<String>>,
}

impl FilterContext {
    pub fn new(tz: Tz) -> Self {
        FilterContext {
            tz,
            now: Utc::now(),
            date_parser_languages: None,
        }
    }
}

/// DjangoFilterBackend (`DocumentFilterSet`) and SearchFilter (`?search=`) in upstream
/// order. Only adds `WHERE` conditions (AND-combined); never widens
/// visibility.
pub async fn apply<D: FilterDb>(
    q: &mut SelectStatement,
    params: &QueryParams,
    user: &User,
    ctx: &FilterContext,
    db: &D,
) -> Result<(), FilterError> {
    filterset::apply_filterset(q, params, user, ctx, db).await?;
    let search = params.get("search");
    let fold = if search.is_some_and(|s| !s.trim().is_empty()) {
        db.upper_fold().await
    } else {
        None
    };
    search::search_cond_folded(search, fold.as_deref())?.apply_to(q);
    Ok(())
}

/// `true` if the request contains a full-text substring filter over the content
/// (`title_content`, `content__i*`, `?search=`). Such conditions cannot be evaluated via an
/// index (full scan with `UPPER` per row, see docs/contract/pgvalidate.md section 9);
/// pl-documents then fetches page and count in **one** scan.
pub fn has_content_text_filter(params: &QueryParams) -> bool {
    const NAMES: [&str; 6] = [
        "title_content",
        "content__icontains",
        "content__istartswith",
        "content__iendswith",
        "content__iexact",
        "search",
    ];
    NAMES
        .iter()
        .any(|n| params.get(n).is_some_and(|v| !pyval::py_strip(v).is_empty()))
}

/// `DocumentsOrderingFilter` + default ordering `-created, -id`. Expects a query
/// without ORDER BY and sets the complete ordering.
pub async fn apply_ordering<D: FilterDb>(
    q: &mut SelectStatement,
    ordering: Option<&str>,
    db: &D,
) -> Result<(), FilterError> {
    ordering::apply_ordering(q, ordering, db).await
}

/// This crate provides no routes.
pub fn routes() -> Vec<ApiRoute> {
    Vec::new()
}
