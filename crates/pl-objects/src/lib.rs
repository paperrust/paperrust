//! pl-objects – `/api/tags/`, `/api/correspondents/`, `/api/document_types/`, `/api/storage_paths/`,
//! `/api/bulk_edit_objects/`. Contract: docs/contract/pl-objects.md.
//!
//! upstream: src/documents/views.py@v3.4.2 (CorrespondentViewSet, TagViewSet, DocumentTypeViewSet,
//!           StoragePathViewSet, BulkEditObjectsView)

pub mod audit;
pub mod bulk;
pub mod counters;
pub mod drf;
pub mod fields;
pub mod handlers;
pub mod listing;
pub mod model;
pub mod objects;
pub mod perms;
pub mod serialize;
pub mod sidefx;
pub mod sp_test;
pub mod tagtree;
pub mod text;
pub mod treenode;
pub mod write;

/// Own migrations (via `pl_cli::migrate::paperrust_migrations`).
pub mod migrations {
    /// L6 step 1: tag hierarchy as adjacency list, django-treenode columns dropped
    /// (docs/DATAMODEL-V2.md §1.9, docs/contract/pl-objects.md „Tag-Baum“).
    pub const TAG_ADJACENCY: (&str, &str) =
        ("pl_objects.0001_paperrust_tag_adjacency", include_str!("../migrations/0001_paperrust_tag_adjacency.sql"));
    pub const ALL: &[(&str, &str)] = &[TAG_ADJACENCY];
}

use axum::http::Method;
use axum::routing::{MethodRouter, delete, get, patch, post, put};
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST, ALLOW_POST_ONLY, drf_view};
use pl_core::{ApiRoute, AppState};

fn list_view(h: MethodRouter<AppState>) -> MethodRouter<AppState> {
    drf_view(h, ALLOW_MODEL_LIST)
}

fn detail_view(h: MethodRouter<AppState>) -> MethodRouter<AppState> {
    drf_view(h, ALLOW_MODEL_DETAIL)
}

macro_rules! viewset_routes {
    ($out:ident, $m:ident, $prefix:literal, $view:literal, $writes:expr) => {{
        $out.push(ApiRoute::new(
            concat!($prefix, ".list"),
            Method::GET,
            concat!("/api/", $prefix, "/"),
            concat!("src/documents/views.py::", $view, ".list"),
            list_view(get(handlers::$m::list)),
        ));
        $out.push(ApiRoute::new(
            concat!($prefix, ".retrieve"),
            Method::GET,
            concat!("/api/", $prefix, "/{id}/"),
            concat!("src/documents/views.py::", $view, ".retrieve"),
            detail_view(get(handlers::$m::retrieve)),
        ));
        $out.push(ApiRoute::new(
            concat!($prefix, ".destroy"),
            Method::DELETE,
            concat!("/api/", $prefix, "/{id}/"),
            concat!("src/documents/views.py::", $view, ".destroy"),
            detail_view(delete(handlers::$m::destroy)),
        ));
        if $writes {
            $out.push(ApiRoute::new(
                concat!($prefix, ".create"),
                Method::POST,
                concat!("/api/", $prefix, "/"),
                concat!("src/documents/views.py::", $view, ".create"),
                list_view(post(handlers::$m::create)),
            ).accepts_drf_parsers());
            $out.push(ApiRoute::new(
                concat!($prefix, ".update"),
                Method::PUT,
                concat!("/api/", $prefix, "/{id}/"),
                concat!("src/documents/views.py::", $view, ".update"),
                detail_view(put(handlers::$m::update)),
            ).accepts_drf_parsers());
            $out.push(ApiRoute::new(
                concat!($prefix, ".partial_update"),
                Method::PATCH,
                concat!("/api/", $prefix, "/{id}/"),
                concat!("src/documents/views.py::", $view, ".partial_update"),
                detail_view(patch(handlers::$m::partial_update)),
            ).accepts_drf_parsers());
        }
    }};
}

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else is proxied to Django.
///
/// Storage paths: create/update/partial_update with `validate_path` via pl-templating
/// (standalone, docs/STANDALONE-GAPS.md); `storage_paths.test`: [`sp_test`] (W23).
pub fn routes() -> Vec<ApiRoute> {
    let mut r: Vec<ApiRoute> = Vec::new();
    viewset_routes!(r, correspondents, "correspondents", "CorrespondentViewSet", true);
    viewset_routes!(r, document_types, "document_types", "DocumentTypeViewSet", true);
    viewset_routes!(r, tags, "tags", "TagViewSet", true);
    viewset_routes!(r, storage_paths, "storage_paths", "StoragePathViewSet", true);
    r.push(ApiRoute::new(
        "storage_paths.test",
        Method::POST,
        "/api/storage_paths/test/",
        "src/documents/views.py::StoragePathViewSet.test",
        drf_view(post(sp_test::post), ALLOW_POST_ONLY),
    ));
    r.push(ApiRoute::new(
        "bulk_edit_objects",
        Method::POST,
        "/api/bulk_edit_objects/",
        "src/documents/views.py::BulkEditObjectsView.post",
        drf_view(post(bulk::post), ALLOW_POST_ONLY),
    ));
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_ids_unique() {
        let r = super::routes();
        let mut ids: Vec<&str> = r.iter().map(|x| x.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len());
        assert_eq!(n, 26); // 4 viewsets x 6 + storage_paths.test + bulk_edit_objects
    }
}
