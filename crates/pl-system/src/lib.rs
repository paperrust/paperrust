//! pl-system – `/api/tasks/`, `/api/statistics/`, `/api/status/`, `/api/config/`, `/api/logs/`,
//! `/api/remote_version/`.
//!
//! Contract: `docs/contract/pl-system.md`. Architecture: `docs/ARCHITECTURE.md`.

/// Python-compatible JSON error texts (`pl_core::pyjson`, moved from pl-fields-views by W11).
pub use pl_core::pyjson;
pub mod config;
pub mod config_write;
pub mod error;
pub mod labels;
pub mod logs;
pub mod pep440;
pub mod py;
pub mod remote_version;
pub mod statistics;
pub mod status;
pub mod sysinfo;
pub mod tasks;

use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{delete, get, patch, post, put};
use pl_core::ApiRoute;
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST, ALLOW_POST_ONLY, ALLOW_READ_ONLY, drf_view};

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else is proxied to Django.
pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new(
            "logs.list",
            Method::GET,
            "/api/logs/",
            "src/documents/views.py::LogViewSet.list",
            drf_view(get(logs::list), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "logs.retrieve",
            Method::GET,
            "/api/logs/{pk}/",
            "src/documents/views.py::LogViewSet.retrieve",
            drf_view(get(logs::retrieve), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "statistics.get",
            Method::GET,
            "/api/statistics/",
            "src/documents/views.py::StatisticsView.get",
            drf_view(get(statistics::get), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "remote_version.get",
            Method::GET,
            "/api/remote_version/",
            "src/documents/views.py::RemoteVersionView.get",
            drf_view(get(remote_version::get), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "status.get",
            Method::GET,
            "/api/status/",
            "src/documents/views.py::SystemStatusView.get",
            drf_view(get(status::get), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.list",
            Method::GET,
            "/api/tasks/",
            "src/documents/views.py::TasksViewSet.list",
            drf_view(get(tasks::list), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.retrieve",
            Method::GET,
            "/api/tasks/{pk}/",
            "src/documents/views.py::TasksViewSet.retrieve",
            drf_view(get(tasks::retrieve), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.acknowledge",
            Method::POST,
            "/api/tasks/acknowledge/",
            "src/documents/views.py::TasksViewSet.acknowledge",
            drf_view(post(tasks::acknowledge), ALLOW_POST_ONLY),
        )
        .accepts_drf_parsers(),
        ApiRoute::new(
            "tasks.summary",
            Method::GET,
            "/api/tasks/summary/",
            "src/documents/views.py::TasksViewSet.summary",
            drf_view(get(tasks::summary), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.status_counts",
            Method::GET,
            "/api/tasks/status_counts/",
            "src/documents/views.py::TasksViewSet.status_counts",
            drf_view(get(tasks::status_counts), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.active",
            Method::GET,
            "/api/tasks/active/",
            "src/documents/views.py::TasksViewSet.active",
            drf_view(get(tasks::active), ALLOW_READ_ONLY),
        ),
        ApiRoute::new(
            "tasks.run",
            Method::POST,
            "/api/tasks/run/",
            "src/documents/views.py::TasksViewSet.run",
            drf_view(post(tasks::run::run), ALLOW_POST_ONLY),
        )
        .accepts_drf_parsers(),
        ApiRoute::new(
            "config.list",
            Method::GET,
            "/api/config/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.list",
            drf_view(get(config::list), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "config.create",
            Method::POST,
            "/api/config/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.create",
            drf_view(post(config::create), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "config.retrieve",
            Method::GET,
            "/api/config/{pk}/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.retrieve",
            drf_view(get(config::retrieve), ALLOW_MODEL_DETAIL),
        ),
        // Standalone (docs/STANDALONE-GAPS.md): writes without Django. Multipart (logo) is read
        // by the handler itself (pl-server STREAMING_ROUTES, no 2.5 MiB gate limit).
        ApiRoute::new(
            "config.update",
            Method::PUT,
            "/api/config/{pk}/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.update",
            drf_view(put(config_write::update).layer(DefaultBodyLimit::disable()), ALLOW_MODEL_DETAIL),
        )
        .accepts_drf_parsers(),
        ApiRoute::new(
            "config.partial_update",
            Method::PATCH,
            "/api/config/{pk}/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.partial_update",
            drf_view(patch(config_write::partial_update).layer(DefaultBodyLimit::disable()), ALLOW_MODEL_DETAIL),
        )
        .accepts_drf_parsers(),
        ApiRoute::new(
            "config.destroy",
            Method::DELETE,
            "/api/config/{pk}/",
            "src/paperless/views.py::ApplicationConfigurationViewSet.destroy",
            drf_view(delete(config_write::destroy), ALLOW_MODEL_DETAIL),
        ),
    ]
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
        assert_eq!(ids.len(), n);
    }
}
