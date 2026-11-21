//! `POST /api/tasks/run/`
//!
//! upstream: src/documents/views.py@v3.4.2 (`TasksViewSet.run`, `_RUNNABLE_TASKS`),
//! src/documents/serialisers.py@v3.4.2 (`RunTaskSerializer`). The task is sent with
//! `pl_core::tasks::send_task` into the PaperRust queue (creates the `PaperlessTask` row like the
//! `before_task_publish` signal).

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_authz::models::PAPERLESS_TASK;
use pl_core::tasks::{TaskRequest, trigger_source};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use crate::error::{Error, Result, insufficient_permissions};
use crate::labels::TASK_TYPE;
use crate::tasks::body;

/// `_RUNNABLE_TASKS`: task type → (task name, kwargs).
pub fn runnable(task_type: &str) -> Option<(&'static str, Vec<(&'static str, Value)>)> {
    match task_type {
        "train_classifier" => Some(("documents.tasks.train_classifier", vec![])),
        "sanity_check" => Some(("documents.tasks.sanity_check", vec![("raise_on_error", Value::Bool(false))])),
        "llm_index" => Some(("documents.tasks.llmindex_index", vec![("rebuild", Value::Bool(false))])),
        _ => None,
    }
}

/// `POST /api/tasks/run/`
pub async fn run(State(state): State<AppState>, _ctx: RequestContext, auth: AuthUser, req: Request) -> Result<Response> {
    // permission_classes of the viewset: PaperlessObjectPermissions → documents.add_paperlesstask
    auth.require_model_perms(&PAPERLESS_TASK)?;
    if !auth.user().is_superuser {
        return Err(insufficient_permissions());
    }
    let data = body::parse_request(req).await?;
    let task_type = body::run_task_type(&data, TASK_TYPE).map_err(Error::Api)?;
    let Some((name, kwargs)) = runnable(&task_type) else {
        return Err(Error::Api(ApiError::validation(json!({"error": format!("Task type '{task_type}' cannot be manually triggered")}))));
    };
    let mut treq = TaskRequest::new(name).trigger_source(trigger_source::MANUAL);
    for (k, v) in kwargs {
        treq = treq.kwarg(k, v);
    }
    match pl_core::tasks::send_task(&state, treq).await {
        Ok(id) => Ok(Drf::ok(json!({"task_id": id.to_string()})).into_response()),
        Err(e) => {
            tracing::warn!("Error running task: {e}");
            Err(Error::Api(ApiError::django_text(StatusCode::INTERNAL_SERVER_ERROR, "Error running task, check logs for more detail.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runnable_tasks() {
        assert_eq!(runnable("train_classifier").unwrap().0, "documents.tasks.train_classifier");
        assert_eq!(runnable("sanity_check").unwrap().1, vec![("raise_on_error", Value::Bool(false))]);
        assert_eq!(runnable("llm_index").unwrap().1, vec![("rebuild", Value::Bool(false))]);
        assert!(runnable("consume_file").is_none());
    }
}
