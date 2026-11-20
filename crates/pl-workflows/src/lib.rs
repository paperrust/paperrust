//! pl-workflows – `/api/workflows/`, `/api/workflow_triggers/`, `/api/workflow_actions/` (CRUD only).
//! Contract: docs/contract/pl-workflows.md. Workflow execution stays in the Python worker.
//! upstream: src/documents/views.py@v3.4.2 (WorkflowTriggerViewSet, WorkflowActionViewSet,
//! WorkflowViewSet), src/paperless/urls.py@v3.4.2 (router registration).

/// Python-compatible JSON error texts (`pl_core::pyjson`, moved from pl-fields-views by W11).
pub use pl_core::pyjson;
pub mod api;
pub mod cfquery;
pub mod drf;
pub mod filters;
pub mod jinja;
pub mod load;
pub mod pyfmt;
pub mod schema;
pub mod serializers;
pub mod write;

use api::Kind;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use pl_authz::AuthUser;
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST, drf_view};
use pl_core::{ApiRoute, AppState, QueryParams, RequestContext};

pub use jinja::validate_template as validate_workflow_template;

macro_rules! viewset {
    ($out:ident, $kind:expr, $base:literal, $detail:literal, $prefix:literal, $view:literal) => {{
        async fn list_h(State(s): State<AppState>, ctx: RequestContext, auth: AuthUser, q: QueryParams) -> Response {
            api::flatten(api::list($kind, s, ctx, auth, q).await)
        }
        async fn create_h(
            State(s): State<AppState>,
            _ctx: RequestContext,
            auth: AuthUser,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            api::flatten(api::create($kind, s, auth, headers, body).await)
        }
        async fn retrieve_h(
            State(s): State<AppState>,
            _ctx: RequestContext,
            auth: AuthUser,
            Path(pk): Path<String>,
        ) -> Response {
            api::flatten(api::retrieve($kind, s, auth, pk).await)
        }
        async fn update_h(
            State(s): State<AppState>,
            _ctx: RequestContext,
            auth: AuthUser,
            Path(pk): Path<String>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            api::flatten(api::update($kind, false, s, auth, pk, headers, body).await)
        }
        async fn partial_update_h(
            State(s): State<AppState>,
            _ctx: RequestContext,
            auth: AuthUser,
            Path(pk): Path<String>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            api::flatten(api::update($kind, true, s, auth, pk, headers, body).await)
        }
        async fn destroy_h(
            State(s): State<AppState>,
            _ctx: RequestContext,
            auth: AuthUser,
            Path(pk): Path<String>,
        ) -> Response {
            api::flatten(api::destroy($kind, s, auth, pk).await)
        }
        $out.push(ApiRoute::new(concat!($prefix, ".list"), Method::GET, $base, concat!("src/documents/views.py::", $view, ".list"), drf_view(get(list_h), ALLOW_MODEL_LIST)));
        $out.push(ApiRoute::new(concat!($prefix, ".create"), Method::POST, $base, concat!("src/documents/views.py::", $view, ".create"), drf_view(post(create_h), ALLOW_MODEL_LIST)));
        $out.push(ApiRoute::new(concat!($prefix, ".retrieve"), Method::GET, $detail, concat!("src/documents/views.py::", $view, ".retrieve"), drf_view(get(retrieve_h), ALLOW_MODEL_DETAIL)));
        $out.push(ApiRoute::new(concat!($prefix, ".update"), Method::PUT, $detail, concat!("src/documents/views.py::", $view, ".update"), drf_view(put(update_h), ALLOW_MODEL_DETAIL)));
        $out.push(ApiRoute::new(
            concat!($prefix, ".partial_update"),
            Method::PATCH,
            $detail,
            concat!("src/documents/views.py::", $view, ".partial_update"),
            drf_view(patch(partial_update_h), ALLOW_MODEL_DETAIL),
        ));
        $out.push(ApiRoute::new(concat!($prefix, ".destroy"), Method::DELETE, $detail, concat!("src/documents/views.py::", $view, ".destroy"), drf_view(delete(destroy_h), ALLOW_MODEL_DETAIL)));
    }};
}

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// everything else is proxied to Django.
pub fn routes() -> Vec<ApiRoute> {
    let mut out = Vec::new();
    viewset!(out, Kind::Trigger, "/api/workflow_triggers/", "/api/workflow_triggers/{id}/", "workflow_triggers", "WorkflowTriggerViewSet");
    viewset!(out, Kind::Action, "/api/workflow_actions/", "/api/workflow_actions/{id}/", "workflow_actions", "WorkflowActionViewSet");
    viewset!(out, Kind::Workflow, "/api/workflows/", "/api/workflows/{id}/", "workflows", "WorkflowViewSet");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_ids_unique() {
        let r = super::routes();
        assert_eq!(r.len(), 18);
        let mut ids: Vec<&str> = r.iter().map(|x| x.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 18);
    }
}
