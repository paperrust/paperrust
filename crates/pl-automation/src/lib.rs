//! pl-automation – PaperRust area "Automatik" (docs/WORKFLOWS-NEU.md), API under
//! [`PREFIX`] = `/rs/api/v2/automation`. Contract: docs/contract/automation.md. No upstream counterpart.
//!
//! Error shape and sign-in like API v2 (`{"error":{…}}`, pl-authz). Permissions via the Django
//! model permissions of workflows/mail rules/documents (pl-authz), documents always filtered to
//! what the user may see.
//!
//! | Method | Path | Purpose |
//! |---|---|---|
//! | GET | `/workflows` | Overview with summary, "runs this week" |
//! | POST | `/workflows` | create (ngx workflow + `else_actions` + `ai_condition`) |
//! | GET/PUT/PATCH | `/workflows/{id}` | read / save together |
//! | GET | `/workflows/{id}/runs` | History (cursor) |
//! | POST | `/workflows/{id}/apply` | apply retroactively (worker task) |
//! | POST | `/dry-run` | Dry run (saved or draft) |
//! | POST | `/runs/cleanup` | delete old history entries (superuser) |
//! | GET | `/suggestions` | Suggestions from the archive |
//! | POST | `/suggestions/draft` | Draft from a suggestion |
//! | POST | `/draft-from-description` | Draft from description (AI) |
//! | GET | `/mail-flows` | Mail rules with their workflows |
//! | POST | `/mail-flows` | Create mail rule + workflows |
//! | GET/PUT/PATCH | `/mail-flows/{rule}` | read / save together |
//! | GET | `/export-ngx` | Extensions as ngx workflows (way back) |

pub mod ai;
pub mod common;
pub mod dryrun;
pub mod error;
pub mod export;
pub mod mailflow;
pub mod runs;
pub mod suggest;
pub mod summary;
pub mod workflows;

use axum::Router;
use axum::routing::{get, post};
use pl_core::AppState;

/// Path prefix.
pub const PREFIX: &str = "/rs/api/v2/automation";

/// Router (relative to [`PREFIX`]); needs `pl_authz::Authz` as an extension (set by pl-server).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(workflows::list).post(workflows::create))
        .route("/workflows/{id}", get(workflows::get_one).put(workflows::put).patch(workflows::patch))
        .route("/workflows/{id}/runs", get(runs::list))
        .route("/workflows/{id}/apply", post(dryrun::apply))
        .route("/dry-run", post(dryrun::dry_run))
        .route("/runs/cleanup", post(runs::cleanup))
        .route("/suggestions", get(suggest::list))
        .route("/suggestions/draft", post(suggest::draft))
        .route("/draft-from-description", post(ai::draft))
        .route("/mail-flows", get(mailflow::list).post(mailflow::create))
        .route("/mail-flows/{id}", get(mailflow::get_one).put(mailflow::put).patch(mailflow::patch))
        .route("/export-ngx", get(export::plan))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
}

/// At startup of server and worker: ensure tables and hook in the AI integration.
pub async fn init(state: &AppState) {
    if let Err(e) = pl_consumer::workflow_ext::ensure_pool(state.db()).await {
        tracing::warn!(target: "paperrust.automation", "workflow extension tables not ready: {e}");
    }
    ai::install(state);
}

async fn not_found() -> axum::response::Response {
    use axum::response::IntoResponse as _;
    error::AErr::NotFound.into_response()
}

async fn method_not_allowed() -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let body = serde_json::json!({"error": {"code": "method_not_allowed", "message": "Method not allowed."}});
    (axum::http::StatusCode::METHOD_NOT_ALLOWED, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// pl-server nests this router before the v2 router (which also occupies `/rs/api/v2/*`);
    /// both must work side by side and the more specific prefix wins.
    #[tokio::test]
    async fn nests_beside_api_v2() {
        let v2: axum::Router = axum::Router::new()
            .route("/server", axum::routing::get(|| async { "v2" }))
            .fallback(|| async { (StatusCode::IM_A_TEAPOT, "v2 fallback") });
        let auto: axum::Router = axum::Router::new().route("/workflows", axum::routing::get(|| async { "auto" }));
        let app = axum::Router::new().nest(super::PREFIX, auto).nest("/rs/api/v2", v2);
        let call = |uri: &'static str| {
            let app = app.clone();
            async move {
                let r = app.oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
                let s = r.status();
                let b = axum::body::to_bytes(r.into_body(), 1024).await.unwrap();
                (s, String::from_utf8_lossy(&b).to_string())
            }
        };
        assert_eq!(call("/rs/api/v2/automation/workflows").await, (StatusCode::OK, "auto".into()));
        assert_eq!(call("/rs/api/v2/server").await, (StatusCode::OK, "v2".into()));
        assert_eq!(call("/rs/api/v2/other").await.0, StatusCode::IM_A_TEAPOT);
    }
}
