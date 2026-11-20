//! pl-documents – document serializer and read-only `/api/documents/` routes (incl. full-text list).
//! See docs/ARCHITECTURE.md and docs/contract/pl-documents.md (contract).
//!
//! Authentication/permissions: exclusively `pl_authz` (no own auth in this crate).
//! Filter/sorting: `pl_docfilter`, full text: `pl_search`, pagination/errors/JSON: `pl_core`.
//!
//! Public interface for other crates (pl-documents-write, pl-files):
//! [`serialize_documents`], [`serialize_documents_db`], [`SerializeOpts`], [`SerializeAction`],
//! [`Viewer`] (= `pl_authz::User`).

use axum::http::Method;
use axum::routing::{get, post};
use pl_core::ApiRoute;
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_POST_ONLY, ALLOW_READ_ONLY, drf_view};

pub mod db;
pub mod langdetect;
pub mod listing;
pub mod selection_data;
pub mod selection_filter;
pub mod metadata;
pub mod serializer;
pub mod suggestions;
pub mod views;

/// The requesting user as the serializer needs it (`request.user`).
pub type Viewer = pl_authz::User;

pub use pl_core::ApiError;
pub use serializer::{
    DOCUMENT_FIELDS, SerializeAction, SerializeOpts, serialize_documents, serialize_documents_db,
};
pub use views::is_search_request;

/// `Allow` of the `notes` action (GET, POST, DELETE).
const ALLOW_NOTES: &str = "GET, POST, DELETE, HEAD, OPTIONS";

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else goes to Django via proxy.
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new(
            "documents.list",
            Method::GET,
            "/api/documents/",
            "src/documents/views.py::UnifiedSearchViewSet.list (inkl. Volltext)",
            drf_view(get(views::list), ALLOW_READ_ONLY),
        )
        // more_like_id: hit order still differs from tantivy-py -> to Django (see search_list).
        .delegatable(),
        ApiRoute::new(
            "documents.next_asn",
            Method::GET,
            "/api/documents/next_asn/",
            "src/documents/views.py::UnifiedSearchViewSet.next_asn",
            drf_view(get(views::next_asn), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "documents.selection_data",
            Method::POST,
            "/api/documents/selection_data/",
            "src/documents/views.py::SelectionDataView.post",
            drf_view(post(views::selection_data_view), ALLOW_POST_ONLY),
        )
        // parser_classes = (MultiPartParser, JSONParser): multipart parsed in the handler,
        // form-urlencoded → 415 (pl-server gate)
        .accepts_multipart(),
        ApiRoute::new(
            "documents.retrieve",
            Method::GET,
            "/api/documents/{id}/",
            "src/documents/views.py::DocumentViewSet.retrieve",
            drf_view(get(views::retrieve), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "documents.root",
            Method::GET,
            "/api/documents/{id}/root/",
            "src/documents/views.py::DocumentViewSet.root",
            drf_view(get(views::root), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "documents.notes.list",
            Method::GET,
            "/api/documents/{id}/notes/",
            "src/documents/views.py::DocumentViewSet.notes (GET)",
            drf_view(get(views::notes), ALLOW_NOTES),
        ),
        ApiRoute::new(
            "documents.share_links",
            Method::GET,
            "/api/documents/{id}/share_links/",
            "src/documents/views.py::DocumentViewSet.share_links",
            drf_view(get(views::share_links), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "documents.history",
            Method::GET,
            "/api/documents/{id}/history/",
            "src/documents/views.py::DocumentViewSet.history",
            drf_view(get(views::history), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "documents.metadata",
            Method::GET,
            "/api/documents/{id}/metadata/",
            "src/documents/views.py::DocumentViewSet.metadata",
            drf_view(get(metadata::metadata), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "documents.suggestions",
            Method::GET,
            "/api/documents/{id}/suggestions/",
            "src/documents/views.py::DocumentViewSet.suggestions",
            drf_view(get(suggestions::suggestions), ALLOW_READ_ONLY),
        ),
    ]
}
