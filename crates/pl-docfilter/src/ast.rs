//! L1 (ARCHITECTURE-V2 §5): v1 query parameters → typed AST [`pl_domain::filter`].
//!
//! `pl-docfilter` is thus the **parser** of the v1 filter logic: form validation, error texts and
//! their order, Python/Django peculiarities (integer overflow, `Decimal` syntax, date formats,
//! `custom_field_query` DSL, `search_smart_split`, DRF ordering) stay here. The result is a
//! [`DocFilter`] with resolved values; SQL is built by `pl_store::docsql`.
//!
//! Order like [`crate::apply`] + [`crate::apply_ordering`]: form errors of all fields →
//! runtime error per filter → `?search=` → `ordering`. Same DB accesses (custom field
//! definitions, document link back references, options for `custom_fields__icontains`).
//!
//! Parameter → node mapping: docs/contract/pl-docfilter.md, section "L1".

use pl_domain::filter::{DocFilter, Pred, Sort};

use crate::db::FilterDb;
use crate::error::FilterError;
use crate::sql::TextLookup;
use crate::{FilterContext, QueryParams};

/// [`TextLookup`] → AST.
pub fn text_lookup_ast(l: TextLookup) -> pl_domain::filter::TextLookup {
    use pl_domain::filter::TextLookup as T;
    match l {
        TextLookup::IContains => T::IContains,
        TextLookup::IStartsWith => T::IStartsWith,
        TextLookup::IEndsWith => T::IEndsWith,
        TextLookup::IExact => T::IExact,
    }
}

/// `DjangoFilterBackend` + `SearchFilter` + `DocumentsOrderingFilter` of the document list.
pub async fn parse<D: FilterDb>(params: &QueryParams, ctx: &FilterContext, db: &D) -> Result<DocFilter, FilterError> {
    let filter = crate::filterset::parse_filterset(params, ctx, db, false).await?;
    let search = crate::search::search_pred(params.get("search"))?;
    let sort = crate::ordering::parse_ordering(params.get("ordering"), db).await?;
    Ok(DocFilter { pred: filter.and(search), sort })
}

/// Only the filters (without `?search=`/`ordering`), optionally lenient (`FilterSet(data=dict).qs`
/// of the `all=true` selection: invalid fields are dropped).
pub async fn parse_filters<D: FilterDb>(params: &QueryParams, ctx: &FilterContext, db: &D, lenient: bool) -> Result<Pred, FilterError> {
    crate::filterset::parse_filterset(params, ctx, db, lenient).await
}

/// Only the ordering.
pub async fn parse_sort<D: FilterDb>(ordering: Option<&str>, db: &D) -> Result<Sort, FilterError> {
    crate::ordering::parse_ordering(ordering, db).await
}
