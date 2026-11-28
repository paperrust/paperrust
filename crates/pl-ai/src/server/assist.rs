//! Entry points for other crates (workflow rework, OCR) and two helper routes.
//!
//! * [`ai_verdict`] – "AI condition": what does the AI think about a document (without changing anything).
//! * [`workflow_draft`] – "workflow from description" (suggestion only, nothing is stored).
//! * AI text recognition (KI-Abschrift) moved to [`super::vision`] (`plan`, `transcribe`, `record`).
//!
//! | Route | Rechte |
//! |---|---|
//! | `POST /api/x-ai/assist/verdict/` `{"document"}` | `view_document` on the document, AI on, 60/h per user |
//! | `POST /api/x-ai/assist/workflow-draft/` `{"description"}` | `documents.add_workflow`, AI on, 30/h per user |

use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use super::data::{self, Cat};
use super::store::body_json;
use crate::auto::{self, Field, Visible};
use crate::config::AutoMode;
use crate::llm::LlmClient;
use crate::taxonomy::{MatchOptions, TaxonomyKind};
use crate::workflow_assist::{self, AiVerdict, DraftTaxonomy, WorkflowDraft};

const ALL_FIELDS: [Field; 6] = [Field::Title, Field::Tags, Field::Correspondent, Field::DocumentType, Field::StoragePath, Field::Created];

/// What does the AI think about `doc_id`? Classification with the permissions of `user` (otherwise of the
/// document owner) + matching like the automation in "suggest" mode. Changes nothing,
/// writes no review-list entry. Errors: AI off, document missing, model error.
pub async fn ai_verdict(state: &AppState, doc_id: i64, user: Option<&User>) -> Result<AiVerdict, String> {
    let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
    if !cfg.ai_enabled {
        return Err("AI disabled".into());
    }
    let doc = data::load_doc(state.db(), doc_id).await.map_err(|e| e.status().to_string())?.ok_or("document not found")?;
    let owner = match (user, doc.owner_id) {
        (Some(u), _) => Some(u.clone()),
        (None, Some(o)) => pl_authz::user::load_user_by_id(state.db(), o).await.map_err(|e| e.to_string())?,
        (None, None) => None,
    };
    let s = super::suggest::classify(state, &cfg, &doc, owner.as_ref(), None).await.map_err(|e| e.to_string())?;
    let tax = super::review::load_taxonomy(state, owner.as_ref()).await.map_err(|e| e.status().to_string())?;
    let vis = |_k: TaxonomyKind| Visible { entries: &tax.entries, aliases: &tax.aliases };
    let opts = MatchOptions { threshold: cfg.rs.match_threshold, ..MatchOptions::default() };
    let limits = auto::RunLimits { max_changes_per_document: 32, ..Default::default() };
    let plan = auto::plan(&s, &ALL_FIELDS, AutoMode::Suggest, &limits, false, 0, &vis, &opts);
    Ok(AiVerdict::from_plan(doc_id, &plan))
}

/// Workflow draft from a description (ngx format, `enabled: false`), only with objects that
/// `user` can see. Nothing is stored.
pub async fn workflow_draft(state: &AppState, description: &str, user: Option<&User>) -> Result<WorkflowDraft, String> {
    let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
    if !cfg.ai_enabled {
        return Err("AI disabled".into());
    }
    let db = state.db();
    let pool = |cat| data::permitted_pool(db, user, cat);
    let t = DraftTaxonomy {
        tags: pool(Cat::Tags).await.map_err(|e| e.status().to_string())?,
        correspondents: pool(Cat::Correspondents).await.map_err(|e| e.status().to_string())?,
        document_types: pool(Cat::DocumentTypes).await.map_err(|e| e.status().to_string())?,
        storage_paths: pool(Cat::StoragePaths).await.map_err(|e| e.status().to_string())?,
    };
    let client = LlmClient::from_config(&cfg).map_err(|e| e.to_string())?;
    let raw = client
        .complete_json(&workflow_assist::workflow_prompt(description, &t), "WorkflowDraft", &workflow_assist::draft_schema())
        .await
        .map_err(|e| e.to_string())?;
    Ok(workflow_assist::sanitize_draft(&raw, &t))
}

fn budget(kind: &str, per_hour: u32) -> &'static crate::mcp::ratelimit::Budget {
    static V: std::sync::OnceLock<crate::mcp::ratelimit::Budget> = std::sync::OnceLock::new();
    static W: std::sync::OnceLock<crate::mcp::ratelimit::Budget> = std::sync::OnceLock::new();
    let cell = if kind == "verdict" { &V } else { &W };
    cell.get_or_init(|| crate::mcp::ratelimit::Budget::new(per_hour, std::time::Duration::from_secs(3600)))
}

/// POST /api/x-ai/assist/verdict/ `{"document": id}` → [`AiVerdict`].
pub async fn verdict_view(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let b = body_json(&body)?;
    let id =
        b.get("document").and_then(Value::as_i64).ok_or_else(|| ApiError::validation(json!({"document": ["This field is required."]})))?;
    let d = data::load_doc(state.db(), id).await?.ok_or_else(ApiError::not_found)?;
    if !has_perms_owner_aware(state.db(), Some(user), "view_document", &DOCUMENT, ObjRef { id: d.root_id, owner_id: d.root_owner_id })
        .await?
    {
        return Err(ApiError::not_found());
    }
    if let Err(wait) = budget("verdict", 60).try_take(&format!("u{}", user.id), 1) {
        return Err(ApiError::throttled(Some(wait as f64)));
    }
    let v = ai_verdict(&state, id, Some(user)).await.map_err(|e| ApiError::validation(json!({"non_field_errors": [short(&e)]})))?;
    Ok(Drf::ok(serde_json::to_value(v).unwrap_or(Value::Null)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/assist/workflow-draft/ `{"description": "…"}` → [`WorkflowDraft`].
pub async fn workflow_draft_view(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = auth.require_perms_by_method(&[("POST", &["documents.add_workflow"])])?;
    let b = body_json(&body)?;
    let desc = b.get("description").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if desc.is_empty() {
        return Err(ApiError::validation(json!({"description": ["This field may not be blank."]})));
    }
    if let Err(wait) = budget("draft", 30).try_take(&format!("u{}", user.id), 1) {
        return Err(ApiError::throttled(Some(wait as f64)));
    }
    let d = workflow_draft(&state, desc, Some(user)).await.map_err(|e| ApiError::validation(json!({"non_field_errors": [short(&e)]})))?;
    Ok(Drf::ok(serde_json::to_value(d).unwrap_or(Value::Null)).indent(ctx.negotiation.indent).into_response())
}

/// Error text without internals (addresses/keys never appear in [`crate::llm::LlmError`] texts; truncated).
fn short(e: &str) -> String {
    e.chars().take(160).collect()
}
