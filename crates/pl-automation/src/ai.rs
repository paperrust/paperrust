//! AI integration: [`PlAiJudge`] (AI condition via pl-ai `ai_verdict` + `AiCondition::matches`) and
//! "workflow from description" (`POST /draft-from-description` → pl-ai `workflow_draft`).
//! Without feature `ai`: no integration (AI condition is ignored and flagged), draft → 501.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use pl_core::AppState;

use crate::common::{self, ADD_WORKFLOW, Who};
use crate::error::AErr;

#[cfg(feature = "ai")]
mod imp {
    use std::sync::{Arc, OnceLock};

    use futures::future::BoxFuture;
    use pl_consumer::workflow_ext::{AiConditionDef, AiJudge, AiJudgement};
    use pl_core::AppState;
    use serde_json::json;

    /// AI condition via pl-ai. Judges with the permissions of the document owner (no user in the run).
    pub struct PlAiJudge {
        pub state: AppState,
    }

    impl AiJudge for PlAiJudge {
        fn enabled(&self) -> BoxFuture<'_, bool> {
            Box::pin(async move { pl_ai::server::effective_config(&self.state).await.map(|c| c.ai_enabled).unwrap_or(false) })
        }

        fn judge<'a>(&'a self, document_id: i64, condition: &'a AiConditionDef) -> BoxFuture<'a, Result<AiJudgement, String>> {
            Box::pin(async move {
                let cond: pl_ai::workflow_assist::AiCondition = serde_json::from_value(json!({
                    "field": condition.field,
                    "object_id": condition.object_id,
                    "min_confidence": condition.min_confidence,
                }))
                .map_err(|e| e.to_string())?;
                let v = pl_ai::server::assist::ai_verdict(&self.state, document_id, None).await?;
                let matched = cond.matches(&v);
                let op = match condition.field.as_str() {
                    "document_type" => v.document_type.clone(),
                    "correspondent" => v.correspondent.clone(),
                    "storage_path" => v.storage_path.clone(),
                    _ => v
                        .tags
                        .iter()
                        .find(|o| o.id == Some(condition.object_id))
                        .or_else(|| v.tags.iter().max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal)))
                        .cloned(),
                };
                Ok(AiJudgement { matched, value: op.as_ref().and_then(|o| o.value.clone()), confidence: op.map(|o| o.confidence) })
            })
        }
    }

    /// Hook in the integration (server and worker call this at startup).
    pub fn install(state: &AppState) {
        pl_consumer::workflow_ext::install_ai_judge(Some(Arc::new(PlAiJudge { state: state.clone() })));
    }

    pub fn budget() -> &'static pl_ai::mcp::ratelimit::Budget {
        static B: OnceLock<pl_ai::mcp::ratelimit::Budget> = OnceLock::new();
        // same limit as /api/x-ai/assist/workflow-draft/
        B.get_or_init(|| pl_ai::mcp::ratelimit::Budget::new(30, std::time::Duration::from_secs(3600)))
    }
}

#[cfg(feature = "ai")]
pub use imp::{PlAiJudge, install};

/// Without feature `ai`: no integration.
#[cfg(not(feature = "ai"))]
pub fn install(_state: &AppState) {
    pl_consumer::workflow_ext::install_ai_judge(None);
}

/// POST /draft-from-description `{"description"}` → `{"workflow", "missing", "explanation", "dropped"}`
/// (proposal only, nothing saved).
pub async fn draft(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    let user = who.require(&[ADD_WORKFLOW])?;
    let b = common::body_object(&headers, &body)?;
    let desc = b.get("description").and_then(serde_json::Value::as_str).map(str::trim).unwrap_or("");
    if desc.is_empty() {
        return Err(AErr::field("description", "This field may not be blank."));
    }
    if desc.chars().count() > 2000 {
        return Err(AErr::field("description", "Ensure this field has no more than 2000 characters."));
    }
    draft_impl(&st, user, desc).await
}

#[cfg(feature = "ai")]
async fn draft_impl(st: &AppState, user: &pl_authz::User, desc: &str) -> Result<Response, AErr> {
    if let Err(wait) = imp::budget().try_take(&format!("u{}", user.id), 1) {
        return Err(AErr::Throttled(Some(wait)));
    }
    match pl_ai::server::assist::workflow_draft(st, desc, Some(user)).await {
        Ok(d) => Ok(crate::error::json(axum::http::StatusCode::OK, &d)),
        Err(e) if e == "AI disabled" => Err(AErr::Unavailable("AI is switched off.".into())),
        Err(e) => Err(AErr::invalid(e.chars().take(160).collect::<String>())),
    }
}

#[cfg(not(feature = "ai"))]
async fn draft_impl(_st: &AppState, _user: &pl_authz::User, _desc: &str) -> Result<Response, AErr> {
    Err(AErr::NotImplemented("built without AI (feature `ai`)".into()))
}
