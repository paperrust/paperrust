//! pl-search – full-text search over the tantivy index that the Python worker writes.
//! Rust ONLY READS (never an IndexWriter). See docs/ARCHITECTURE.md and
//! docs/contract/pl-search.md (contract, interface for pl-documents).
//!
//! upstream: src/documents/search/*@v3.4.2, src/documents/views.py
//! (UnifiedSearchViewSet.list, DocumentSelectionMixin._get_search_document_ids,
//! SearchAutoCompleteView, GlobalSearchView).

pub mod backend;
pub mod db;
pub mod error;
pub mod query_simple;
pub mod query_user;
pub mod schema;
pub mod tokenizer;
pub mod views;
mod wc;

#[cfg(test)]
mod testutil;
#[cfg(test)]
mod tests_backend;

pub use backend::{RankedWindow, SearchBackend, SearchHit, SearchMode, SearchSettings, SearchUser};
pub use error::SearchError;

#[cfg(feature = "routes")]
pub mod global;
#[cfg(feature = "routes")]
pub mod routes;

/// All routes of this module (`search.autocomplete`, `search.global`). The server
/// mounts only those activated in `routes.toml`; everything else goes via proxy to Django.
/// The full-text document list (`/api/documents/?query=...`) belongs to the route of
/// pl-documents, which calls [`views::document_search`].
#[cfg(feature = "routes")]
pub fn routes() -> Vec<pl_core::ApiRoute> {
    routes::routes()
}
