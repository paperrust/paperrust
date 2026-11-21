//! Entry points for other crates (workflow rework, OCR) and two helper routes.
//!
//! * [`ai_verdict`] – "AI condition": what does the AI think about a document (without changing anything).
//! * [`workflow_draft`] – "workflow from description" (suggestion only, nothing is stored).
//! * [`vision_ocr`] / [`vision_wanted`] – AI text recognition; pl-server hooks it into pl-consumer as pl-core `vision::VisionOcr`.
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
use crate::prompt_store::PromptKind;
use crate::taxonomy::{MatchOptions, TaxonomyKind};
use crate::vision::{PageImage, VisionResult};
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

/// Should AI text recognition run? `requested` = user chose "AI text recognition" for this
/// document; `ocr_chars` = characters of the normal text recognition (if it has already run).
/// Rules: AI off or option off → never; `requested` → yes; `vision_when = always` → yes;
/// `difficult` → if fewer than 50 characters per page were recognized.
pub async fn vision_wanted(state: &AppState, requested: bool, ocr_chars: Option<usize>, pages: usize) -> bool {
    let Ok(cfg) = super::effective_config(state).await else { return false };
    let opts = super::store::load_options(state.db()).await;
    vision_decision(cfg.ai_enabled && opts.vision_enabled, &opts.vision_when, requested, ocr_chars, pages)
}

/// Pure decision for [`vision_wanted`].
pub fn vision_decision(enabled: bool, when: &str, requested: bool, ocr_chars: Option<usize>, pages: usize) -> bool {
    if !enabled {
        return false;
    }
    if requested || when == "always" {
        return true;
    }
    when == "difficult" && ocr_chars.is_some_and(|c| c < 50 * pages.max(1))
}

/// AI text recognition with the effective configuration (image model, template `vision_ocr`, limits).
/// `Ok(None)` if AI or the option is off. Called via pl-server `worker::AiVisionOcr` (ingestion, reprocessing).
pub async fn vision_ocr(state: &AppState, pages: &[PageImage]) -> Result<Option<VisionResult>, String> {
    let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
    let opts = super::store::load_options(state.db()).await;
    if !cfg.ai_enabled || !opts.vision_enabled {
        return Ok(None);
    }
    let prompts = super::store::load_prompts(state.db()).await;
    let lang = super::store::language_name(&cfg, None);
    let prompt = crate::prompt_store::render(prompts.get(PromptKind::VisionOcr), &[("language", &lang)]);
    let client = LlmClient::from_config(&cfg).map_err(|e| e.to_string())?.with_model(&opts.vision_model);
    crate::vision::recognize_pages(&client, &prompt, pages, &opts.vision_limits).await.map(Some).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_rules() {
        assert!(!vision_decision(false, "always", true, None, 1));
        assert!(vision_decision(true, "off", true, Some(5000), 1));
        assert!(!vision_decision(true, "off", false, Some(0), 1));
        assert!(vision_decision(true, "always", false, None, 3));
        assert!(vision_decision(true, "difficult", false, Some(80), 2));
        assert!(!vision_decision(true, "difficult", false, Some(400), 2));
        assert!(!vision_decision(true, "difficult", false, None, 2));
    }
}
