//! pl-files – file serving (download, preview, thumb, share links, bulk download, logo).
//! See docs/ARCHITECTURE.md (module layout) and docs/contract/pl-files.md (contract).
//!
//! upstream: src/documents/views.py@v3.4.2, src/documents/bulk_download.py@v3.4.2,
//!           src/documents/conditionals.py@v3.4.2, src/documents/versioning.py@v3.4.2

pub mod bulk;
pub mod compat;
pub mod conditional;
pub mod config;
pub mod db;
pub mod docfile;
pub mod files;
pub mod logo;
pub mod naming;
pub mod perms;
pub mod resolve;
pub mod serve;
pub mod share;
pub mod share_i18n;
pub mod zipw;

use axum::http::Method;
use axum::routing::{get, post};
use pl_core::ApiRoute;
use pl_core::route::{ALLOW_POST_ONLY, ALLOW_READ_ONLY, drf_view};

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else is proxied to Django. Several entries with the same `id` belong
/// together (same Django URL with/without slash or optional suffix).
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute {
            id: "documents.download",
            method: Method::GET,
            path: "/api/documents/{id}/download/",
            upstream: "src/documents/views.py::DocumentViewSet.download",
            handler: drf_view(get(files::download), ALLOW_READ_ONLY),
        },
        ApiRoute {
            id: "documents.preview",
            method: Method::GET,
            path: "/api/documents/{id}/preview/",
            upstream: "src/documents/views.py::DocumentViewSet.preview",
            handler: drf_view(get(files::preview), ALLOW_READ_ONLY),
        },
        ApiRoute {
            id: "documents.thumb",
            method: Method::GET,
            path: "/api/documents/{id}/thumb/",
            upstream: "src/documents/views.py::DocumentViewSet.thumb",
            handler: drf_view(get(files::thumb), ALLOW_READ_ONLY),
        },
        ApiRoute {
            id: "documents.bulk_download",
            method: Method::POST,
            path: "/api/documents/bulk_download/",
            upstream: "src/documents/views.py::BulkDownloadView.post",
            handler: drf_view(post(bulk::bulk_download), ALLOW_POST_ONLY),
        }
        .delegatable(),
        ApiRoute {
            id: "share.get_noslash",
            method: Method::GET,
            path: "/share/{slug}",
            upstream: "src/documents/views.py::SharedLinkView.get",
            handler: get(share::shared_link),
        }
        .delegatable(),
        ApiRoute {
            id: "share.get",
            method: Method::GET,
            path: "/share/{slug}/",
            upstream: "src/documents/views.py::SharedLinkView.get",
            handler: get(share::shared_link),
        }
        .delegatable(),
        ApiRoute {
            id: "logo.get_noslash",
            method: Method::GET,
            path: "/logo",
            upstream: "src/documents/views.py::serve_logo",
            handler: get(logo::serve_logo),
        },
        ApiRoute {
            id: "logo.get",
            method: Method::GET,
            path: "/logo/",
            upstream: "src/documents/views.py::serve_logo",
            handler: get(logo::serve_logo),
        },
        ApiRoute {
            id: "logo.get_file",
            method: Method::GET,
            path: "/logo/{*filename}",
            upstream: "src/documents/views.py::serve_logo",
            handler: get(logo::serve_logo),
        },
        ApiRoute {
            id: "favicon.get",
            method: Method::GET,
            path: "/favicon.ico",
            upstream: "src/paperless/views.py::FaviconView",
            handler: get(logo::favicon),
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_table() {
        let r = super::routes();
        assert_eq!(r.len(), 10);
        assert!(r.iter().all(|x| x.path.starts_with('/')));
    }
}
