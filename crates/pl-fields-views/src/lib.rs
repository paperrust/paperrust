//! pl-fields-views – see docs/ARCHITECTURE.md (module layout) and
//! docs/contract/pl-fields-views.md (contract).
//!
//! Routes: `/api/custom_fields/`, `/api/saved_views/`, `/api/share_links/`,
//! `/api/share_link_bundles/` (paperless-ngx v3.4.2).

pub mod audit;
pub mod authz;
pub mod bundle_task;
pub mod bundles;
pub mod celery;
pub mod cf_counters;
pub mod custom_fields;
pub mod fields;
/// Moved to pl-core (W11); re-exported for `crate::pyjson::…` paths.
pub use pl_core::pyjson;
pub mod query;
pub mod saved_views;
pub mod share_links;
pub mod shim;

use axum::http::Method;
use axum::routing::{delete, get, patch, post, put};
use pl_core::ApiRoute;
// Allow/Vary also for extractor rejections (401/406) like DRF – parity run 2026-11-08
use pl_core::route::drf_view;

pub use shim::{DelegateToDjango, User};

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else is proxied to Django.
pub fn routes() -> Vec<ApiRoute> {
    const CF: &str = "src/documents/views.py::CustomFieldViewSet";
    const SV: &str = "src/documents/views.py::SavedViewViewSet";
    const SL: &str = "src/documents/views.py::ShareLinkViewSet";
    const SB: &str = "src/documents/views.py::ShareLinkBundleViewSet";
    let routes = vec![
        // custom fields
        ApiRoute {
            id: "custom_fields.list",
            method: Method::GET,
            path: "/api/custom_fields/",
            upstream: CF,
            handler: drf_view(get(custom_fields::list), custom_fields::LIST_ALLOW),
        },
        ApiRoute {
            id: "custom_fields.create",
            method: Method::POST,
            path: "/api/custom_fields/",
            upstream: CF,
            handler: drf_view(post(custom_fields::create), custom_fields::LIST_ALLOW),
        },
        ApiRoute {
            id: "custom_fields.retrieve",
            method: Method::GET,
            path: "/api/custom_fields/{id}/",
            upstream: CF,
            handler: drf_view(get(custom_fields::retrieve), custom_fields::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "custom_fields.update",
            method: Method::PUT,
            path: "/api/custom_fields/{id}/",
            upstream: CF,
            handler: drf_view(put(custom_fields::update), custom_fields::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "custom_fields.partial_update",
            method: Method::PATCH,
            path: "/api/custom_fields/{id}/",
            upstream: CF,
            handler: drf_view(patch(custom_fields::partial_update), custom_fields::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "custom_fields.destroy",
            method: Method::DELETE,
            path: "/api/custom_fields/{id}/",
            upstream: CF,
            handler: drf_view(delete(custom_fields::destroy), custom_fields::DETAIL_ALLOW),
        },
        // saved views
        ApiRoute {
            id: "saved_views.list",
            method: Method::GET,
            path: "/api/saved_views/",
            upstream: SV,
            handler: drf_view(get(saved_views::list), saved_views::LIST_ALLOW),
        },
        ApiRoute {
            id: "saved_views.create",
            method: Method::POST,
            path: "/api/saved_views/",
            upstream: SV,
            handler: drf_view(post(saved_views::create), saved_views::LIST_ALLOW),
        },
        ApiRoute {
            id: "saved_views.retrieve",
            method: Method::GET,
            path: "/api/saved_views/{id}/",
            upstream: SV,
            handler: drf_view(get(saved_views::retrieve), saved_views::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "saved_views.update",
            method: Method::PUT,
            path: "/api/saved_views/{id}/",
            upstream: SV,
            handler: drf_view(put(saved_views::update), saved_views::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "saved_views.partial_update",
            method: Method::PATCH,
            path: "/api/saved_views/{id}/",
            upstream: SV,
            handler: drf_view(patch(saved_views::partial_update), saved_views::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "saved_views.destroy",
            method: Method::DELETE,
            path: "/api/saved_views/{id}/",
            upstream: SV,
            handler: drf_view(delete(saved_views::destroy), saved_views::DETAIL_ALLOW),
        },
        // share links
        ApiRoute {
            id: "share_links.list",
            method: Method::GET,
            path: "/api/share_links/",
            upstream: SL,
            handler: drf_view(get(share_links::list), share_links::LIST_ALLOW),
        },
        ApiRoute {
            id: "share_links.create",
            method: Method::POST,
            path: "/api/share_links/",
            upstream: SL,
            handler: drf_view(post(share_links::create), share_links::LIST_ALLOW),
        },
        ApiRoute {
            id: "share_links.retrieve",
            method: Method::GET,
            path: "/api/share_links/{id}/",
            upstream: SL,
            handler: drf_view(get(share_links::retrieve), share_links::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "share_links.destroy",
            method: Method::DELETE,
            path: "/api/share_links/{id}/",
            upstream: SL,
            handler: drf_view(delete(share_links::destroy), share_links::DETAIL_ALLOW),
        },
        // share link bundles
        ApiRoute {
            id: "share_link_bundles.list",
            method: Method::GET,
            path: "/api/share_link_bundles/",
            upstream: SB,
            handler: drf_view(get(bundles::list), bundles::LIST_ALLOW),
        },
        ApiRoute {
            id: "share_link_bundles.create",
            method: Method::POST,
            path: "/api/share_link_bundles/",
            upstream: SB,
            handler: drf_view(post(bundles::create), bundles::LIST_ALLOW),
        },
        ApiRoute {
            id: "share_link_bundles.retrieve",
            method: Method::GET,
            path: "/api/share_link_bundles/{id}/",
            upstream: SB,
            handler: drf_view(get(bundles::retrieve), bundles::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "share_link_bundles.destroy",
            method: Method::DELETE,
            path: "/api/share_link_bundles/{id}/",
            upstream: SB,
            handler: drf_view(delete(bundles::destroy), bundles::DETAIL_ALLOW),
        },
        ApiRoute {
            id: "share_link_bundles.rebuild",
            method: Method::POST,
            path: "/api/share_link_bundles/{id}/rebuild/",
            upstream: "src/documents/views.py::ShareLinkBundleViewSet.rebuild",
            handler: drf_view(post(bundles::rebuild), bundles::REBUILD_ALLOW),
        },
    ];
    // Bodies go through `shim::parse_body` (JSON + form-urlencoded; multipart -> Django).
    routes
        .into_iter()
        .map(|r| r.accepts_form())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_ids_unique() {
        let r = super::routes();
        let mut ids: Vec<&str> = r.iter().map(|r| r.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert_eq!(n, 21);
    }

    /// Parity run 2026-11-08 (`custom_fields.list.apiver@admin/v99` among others): the rejection in the
    /// extractor (here 406 invalid version) also carries `Allow` and `Vary: Accept` like DRF.
    #[tokio::test]
    async fn extractor_rejection_has_drf_headers() {
        use tower::ServiceExt;
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "pl-fields-views-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
        )]))
        .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        let state = pl_core::AppState::with_pool(cfg, pool).unwrap();
        let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
        for r in super::routes().into_iter().filter(|r| r.id == "custom_fields.list" || r.id == "share_links.retrieve") {
            router = router.route(r.path, r.handler);
        }
        let app = router.with_state(state);
        for (uri, allow) in [("/api/custom_fields/", "GET, POST, HEAD, OPTIONS"), ("/api/share_links/1/", "GET, DELETE, HEAD, OPTIONS")] {
            let req = axum::http::Request::builder()
                .uri(uri)
                .header("host", "localhost")
                .header("accept", "application/json; version=99")
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), 406, "{uri}");
            assert_eq!(resp.headers()["allow"], allow);
            assert!(resp.headers()["vary"].to_str().unwrap().contains("Accept"));
        }
    }
}
