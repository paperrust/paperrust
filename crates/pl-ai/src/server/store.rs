//! Own prompts (`paperrust_ai_prompt`), AI options (`paperrust_ai_option`), status and
//! provider test (PaperRust extension, contract section "Einstellungen KI").
//!
//! | Route | Rechte |
//! |---|---|
//! | `GET /api/x-ai/status/` | logged in (counters only over visible documents) |
//! | `GET /api/x-ai/prompts/`, `PUT/DELETE /api/x-ai/prompts/{kind}/` | active superuser |
//! | `POST /api/x-ai/prompts/{kind}/preview/` | active superuser + `view_document` on the document |
//! | `GET/PATCH /api/x-ai/options/` | active superuser |
//! | `POST /api/x-ai/provider/check/` | active superuser |

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Map, Value, json};
use sqlx::Row;

use crate::config::{AiConfig, AutoMode, LlmBackend};
use crate::custom_fields::{self, FieldDef};
use crate::llm::LlmClient;
use crate::options::AiOptions;
use crate::prompt_store::{self, PromptKind, PromptSet};

pub(crate) fn require_superuser(auth: &AuthUser) -> Result<&User, ApiError> {
    let u = auth.user();
    if u.is_active && u.is_superuser { Ok(u) } else { Err(ApiError::permission_denied()) }
}

pub(crate) fn storage_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01")) {
        return ApiError::with_detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "AI storage is not installed (migration 0003_paperrust_ai_prompts_jobs.sql).",
        );
    }
    tracing::error!(target: "pl_ai", error = %e, "ai storage failed");
    ApiError::internal("ai storage failed")
}

pub(crate) fn body_json(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|_| ApiError::parse_error(Some("JSON parse error")))
}

/// Stored templates (table missing/error → built-in ones).
pub async fn load_prompts(db: &sqlx::PgPool) -> PromptSet {
    let mut set = PromptSet::default();
    if let Ok(rows) = sqlx::query("SELECT kind, template FROM paperrust_ai_prompt").fetch_all(db).await {
        for r in rows {
            if let (Ok(k), Ok(t)) = (r.try_get::<String, _>(0), r.try_get::<String, _>(1))
                && let Some(kind) = PromptKind::parse(&k)
            {
                set.custom.insert(kind, t);
            }
        }
    }
    set
}

/// Effective options (table missing/error → environment/default).
pub async fn load_options(db: &sqlx::PgPool) -> AiOptions {
    let pairs: Vec<(String, Value)> = sqlx::query("SELECT key, value FROM paperrust_ai_option")
        .fetch_all(db)
        .await
        .map(|rows| rows.iter().filter_map(|r| Some((r.try_get::<String, _>(0).ok()?, r.try_get::<Value, _>(1).ok()?))).collect())
        .unwrap_or_default();
    AiOptions::from_pairs(&pl_core::config::var, &pairs)
}

/// Custom fields (all; `supported` = the AI can handle the type).
pub async fn load_field_defs(db: &sqlx::PgPool) -> Result<Vec<FieldDef>, ApiError> {
    let rows = sqlx::query("SELECT id, name, data_type, extra_data FROM documents_customfield ORDER BY name, id")
        .fetch_all(db)
        .await
        .map_err(ApiError::internal)?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(FieldDef {
                id: i64::from(r.try_get::<i32, _>(0).ok()?),
                name: r.try_get(1).ok()?,
                data_type: r.try_get(2).ok()?,
                extra_data: r.try_get::<Option<Value>, _>(3).ok().flatten(),
            })
        })
        .collect())
}

// ------------------------------------------------------------------ Status

/// GET /api/x-ai/status/ – for navigation/home page: AI on?, open suggestions (visible documents).
pub async fn status(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    let user = auth.user();
    let cfg = super::effective_config(&state).await?;
    let opts = load_options(state.db()).await;
    let pending: Option<i64> = if cfg.ai_enabled && user.has_perm("documents.view_document") {
        super::review::pending_count(&state, user).await.ok()
    } else {
        None
    };
    let mode = match cfg.rs.auto_mode {
        AutoMode::Off => "off",
        AutoMode::Suggest => "suggest",
        AutoMode::Apply => "apply",
    };
    let v = json!({
        "enabled": cfg.ai_enabled,
        "configured": cfg.llm_backend.is_some(),
        "auto_mode": mode,
        "trigger_tag": cfg.rs.auto_trigger_tag,
        "pending": pending,
        "vision_enabled": cfg.ai_enabled && opts.vision_enabled,
        "custom_fields_enabled": cfg.ai_enabled && opts.custom_fields_enabled,
        "can_configure": user.is_active && user.is_superuser,
        "can_review": user.has_perm("documents.change_document"),
    });
    Ok(Drf::ok(v).indent(ctx.negotiation.indent).into_response())
}

// ------------------------------------------------------------------ Prompts

fn prompt_json(kind: PromptKind, set: &PromptSet, updated: Option<String>) -> Value {
    json!({
        "kind": kind.as_str(),
        "template": set.get(kind),
        "default": kind.default_template(),
        "custom": set.custom(kind).is_some(),
        "placeholders": kind.placeholders(),
        "required": kind.required(),
        "updated": updated,
    })
}

/// GET /api/x-ai/prompts/
pub async fn prompts_list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let set = load_prompts(state.db()).await;
    let updated: Vec<(String, chrono::DateTime<chrono::Utc>)> = sqlx::query("SELECT kind, updated FROM paperrust_ai_prompt")
        .fetch_all(state.db())
        .await
        .map(|rows| rows.iter().filter_map(|r| Some((r.try_get(0).ok()?, r.try_get(1).ok()?))).collect())
        .unwrap_or_default();
    let results: Vec<Value> = PromptKind::ALL
        .iter()
        .map(|k| prompt_json(*k, &set, updated.iter().find(|(n, _)| n == k.as_str()).map(|(_, d)| pl_core::datetime::python_isoformat(d))))
        .collect();
    Ok(Drf::ok(json!({"count": results.len(), "results": results})).indent(ctx.negotiation.indent).into_response())
}

fn parse_kind(k: &str) -> Result<PromptKind, ApiError> {
    PromptKind::parse(k).ok_or_else(ApiError::not_found)
}

/// PUT /api/x-ai/prompts/{kind}/ `{"template": "…"}`
pub async fn prompts_put(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(kind): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?;
    let kind = parse_kind(&kind)?;
    let b = body_json(&body)?;
    let Some(t) = b.get("template").and_then(Value::as_str) else {
        return Err(ApiError::validation(json!({"template": ["This field is required."]})));
    };
    if let Err(codes) = prompt_store::validate(kind, t) {
        return Err(ApiError::validation(json!({"template": codes})));
    }
    sqlx::query(
        "INSERT INTO paperrust_ai_prompt (kind, template, updated, updated_by_id) VALUES ($1, $2, now(), $3) \
         ON CONFLICT (kind) DO UPDATE SET template = EXCLUDED.template, updated = now(), updated_by_id = EXCLUDED.updated_by_id",
    )
    .bind(kind.as_str())
    .bind(t)
    .bind(user.id as i32)
    .execute(state.db())
    .await
    .map_err(storage_err)?;
    let set = load_prompts(state.db()).await;
    Ok(Drf::ok(prompt_json(kind, &set, Some(pl_core::datetime::python_isoformat(&chrono::Utc::now()))))
        .indent(ctx.negotiation.indent)
        .into_response())
}

/// DELETE /api/x-ai/prompts/{kind}/ – back to the built-in template.
pub async fn prompts_reset(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(kind): Path<String>,
) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let kind = parse_kind(&kind)?;
    sqlx::query("DELETE FROM paperrust_ai_prompt WHERE kind = $1").bind(kind.as_str()).execute(state.db()).await.map_err(storage_err)?;
    let set = load_prompts(state.db()).await;
    Ok(Drf::ok(prompt_json(kind, &set, None)).indent(ctx.negotiation.indent).into_response())
}

/// Language for `{{language}}` (output language of the AI, otherwise English).
pub fn language_name(cfg: &AiConfig, ui_lang: Option<&str>) -> String {
    let code = cfg.llm_output_language.as_deref().filter(|s| !s.is_empty()).or(ui_lang).unwrap_or("en-us");
    crate::classifier::get_language_name(code)
}

/// Fills a template for a document (without a model call).
pub async fn render_for_document(
    state: &AppState,
    cfg: &AiConfig,
    kind: PromptKind,
    template: &str,
    doc: &super::data::AiDoc,
    user: Option<&User>,
    fields: &[FieldDef],
) -> String {
    let content_head: String = doc.content.chars().take(crate::classifier::PROMPT_CONTENT_CHARS).collect();
    let content = crate::classifier::Truncator::truncate_content(
        &crate::tokenizer::TiktokenTruncator,
        &content_head,
        cfg.llm_embedding_chunk_size as usize,
        cfg.llm_context_size as usize,
    );
    let filename = doc.filename.clone().unwrap_or_default();
    let lang = language_name(cfg, None);
    match kind {
        PromptKind::Classification => {
            let (cands, _ctx) = super::suggest::candidates_for(state, cfg, doc, user).await;
            let block = crate::classifier::format_taxonomy_for_prompt(&cands);
            prompt_store::render(
                template,
                &[
                    ("content", &content),
                    ("filename", &filename),
                    ("candidates", &block),
                    ("candidate_rules", prompt_store::classification_rules(cands.any())),
                ],
            )
        }
        PromptKind::Title => prompt_store::render(template, &[("content", &content), ("filename", &filename), ("language", &lang)]),
        PromptKind::CustomFields => {
            let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
            prompt_store::render(
                template,
                &[("content", &content), ("fields", &custom_fields::prompt_fields(fields)), ("language", &lang), ("today", &today)],
            )
        }
        PromptKind::VisionOcr => prompt_store::render(template, &[("language", &lang)]),
        PromptKind::ChatSystem => template.trim().to_string(),
    }
}

/// POST /api/x-ai/prompts/{kind}/preview/ `{"document": id, "template"?: "…", "run"?: bool}` →
/// `{"prompt", "result"?, "error"?}`. `run` calls the model (classification, title, custom fields).
pub async fn prompts_preview(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(kind): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?;
    let kind = parse_kind(&kind)?;
    let b = body_json(&body)?;
    let set = load_prompts(state.db()).await;
    let template = b.get("template").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| set.get(kind).to_string());
    if let Err(codes) = prompt_store::validate(kind, &template) {
        return Err(ApiError::validation(json!({"template": codes})));
    }
    let cfg = super::effective_config(&state).await?;
    let doc = match b.get("document").and_then(Value::as_i64) {
        Some(id) => {
            let d = super::data::load_doc(state.db(), id).await?.ok_or_else(ApiError::not_found)?;
            if !has_perms_owner_aware(
                state.db(),
                Some(user),
                "view_document",
                &DOCUMENT,
                ObjRef { id: d.root_id, owner_id: d.root_owner_id },
            )
            .await?
            {
                return Err(ApiError::not_found());
            }
            Some(d)
        }
        None => None,
    };
    let needs_doc = matches!(kind, PromptKind::Classification | PromptKind::Title | PromptKind::CustomFields);
    let Some(doc) = doc.filter(|_| needs_doc) else {
        if needs_doc {
            return Err(ApiError::validation(json!({"document": ["This field is required."]})));
        }
        let lang = language_name(&cfg, None);
        let p = if kind == PromptKind::VisionOcr {
            prompt_store::render(&template, &[("language", &lang)])
        } else {
            template.trim().to_string()
        };
        return Ok(Drf::ok(json!({"prompt": p, "result": Value::Null, "error": Value::Null}))
            .indent(ctx.negotiation.indent)
            .into_response());
    };
    let opts = load_options(state.db()).await;
    let fields: Vec<FieldDef> = load_field_defs(state.db()).await?.into_iter().filter(|f| opts.custom_fields.contains(&f.id)).collect();
    let prompt = render_for_document(&state, &cfg, kind, &template, &doc, Some(user), &fields).await;
    let mut out = Map::new();
    out.insert("prompt".into(), json!(prompt));
    out.insert("result".into(), Value::Null);
    out.insert("error".into(), Value::Null);
    if b.get("run").and_then(Value::as_bool) == Some(true) {
        if !cfg.ai_enabled {
            out.insert("error".into(), json!("ai_disabled"));
        } else {
            match LlmClient::from_config(&cfg) {
                Err(e) => {
                    out.insert("error".into(), json!(crate::llm_ext::friendly_error(&e)));
                }
                Ok(client) => {
                    let r = match kind {
                        PromptKind::Classification => client.query_raw(&prompt).await.map(|r| serde_json::to_value(r).unwrap_or(Value::Null)),
                        PromptKind::Title => client.complete_text(None, &prompt).await.map(|t| json!(clean_title(&t))),
                        _ => client.complete_json(&prompt, "CustomFields", &custom_fields::response_schema()).await.map(|v| {
                            let vals = custom_fields::parse_answer(&fields, &v);
                            Value::Array(vals.iter().map(|f| json!({"field": f.field_id, "value": f.value, "display": f.display, "confidence": f.confidence})).collect())
                        }),
                    };
                    match r {
                        Ok(v) => {
                            out.insert("result".into(), v);
                        }
                        Err(e) => {
                            out.insert("error".into(), json!(crate::llm_ext::friendly_error(&e)));
                        }
                    }
                }
            }
        }
    }
    Ok(Drf::ok(Value::Object(out)).indent(ctx.negotiation.indent).into_response())
}

/// Title from a free response: first non-empty line, without quotation marks/prefix, ≤ 128 characters.
pub fn clean_title(t: &str) -> String {
    let line = t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.strip_prefix("Title:").or_else(|| line.strip_prefix("Titel:")).unwrap_or(line).trim();
    line.trim_matches(|c| c == '"' || c == '\'' || c == '„' || c == '“' || c == '*').trim().chars().take(128).collect()
}

// ------------------------------------------------------------------ Options

/// GET /api/x-ai/options/ → `{"options": {…}, "custom_fields": [{id, name, data_type, supported}]}`
pub async fn options_get(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let o = load_options(state.db()).await;
    let fields: Vec<Value> = load_field_defs(state.db())
        .await?
        .iter()
        .map(|f| json!({"id": f.id, "name": f.name, "data_type": f.data_type, "supported": custom_fields::supported(&f.data_type)}))
        .collect();
    Ok(Drf::ok(json!({"options": o, "custom_fields": fields})).indent(ctx.negotiation.indent).into_response())
}

/// PATCH /api/x-ai/options/ `{"<key>": <value>, …}` (values are validated and clamped).
pub async fn options_patch(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?;
    let b = body_json(&body)?;
    let Some(m) = b.as_object() else { return Err(ApiError::validation(json!({"non_field_errors": ["Invalid data."]}))) };
    let mut errs = Map::new();
    let mut ok: Vec<(String, Value)> = Vec::new();
    for (k, v) in m {
        match AiOptions::validate_pair(k, v) {
            Ok(v) => ok.push((k.clone(), v)),
            Err(e) => {
                errs.insert(k.clone(), json!([e]));
            }
        }
    }
    if !errs.is_empty() {
        return Err(ApiError::validation(Value::Object(errs)));
    }
    for (k, v) in ok {
        sqlx::query(
            "INSERT INTO paperrust_ai_option (key, value, updated, updated_by_id) VALUES ($1, $2, now(), $3) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated = now(), updated_by_id = EXCLUDED.updated_by_id",
        )
        .bind(&k)
        .bind(&v)
        .bind(user.id as i32)
        .execute(state.db())
        .await
        .map_err(storage_err)?;
    }
    options_get(State(state), ctx, auth).await
}

// ------------------------------------------------------------------ Provider test

/// POST /api/x-ai/provider/check/ `{"backend"?, "endpoint"?, "model"?, "api_key"?}` → connection test
/// with model list. Missing values = stored ones. The stored API key is sent only to the
/// stored endpoint (never to a newly entered address).
pub async fn provider_check(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let b = body_json(&body)?;
    let mut cfg = super::effective_config(&state).await?;
    if let Some(be) = b.get("backend").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        cfg.llm_backend = Some(LlmBackend::parse(be).ok_or_else(|| ApiError::validation(json!({"backend": ["Unknown provider."]})))?);
    }
    let old_endpoint = cfg.llm_endpoint.clone();
    if let Some(ep) = b.get("endpoint").and_then(Value::as_str) {
        cfg.llm_endpoint = Some(ep.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(m) = b.get("model").and_then(Value::as_str) {
        cfg.llm_model = Some(m.trim().to_string()).filter(|s| !s.is_empty());
    }
    match b.get("api_key").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(k) => cfg.llm_api_key = Some(k.to_string()),
        None if cfg.llm_endpoint != old_endpoint => cfg.llm_api_key = None,
        None => {}
    }
    if cfg.llm_backend.is_none() {
        cfg.llm_backend = Some(LlmBackend::Ollama);
    }
    cfg.llm_request_timeout = cfg.llm_request_timeout.min(20);
    let check = match LlmClient::from_config(&cfg) {
        Ok(c) => c.check_connection().await,
        Err(e) => crate::llm_ext::ConnectionCheck {
            ok: false,
            models: Vec::new(),
            model_available: None,
            error: Some(crate::llm_ext::friendly_error(&e)),
        },
    };
    Ok(Drf::ok(serde_json::to_value(check).unwrap_or(Value::Null)).indent(ctx.negotiation.indent).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(clean_title("\n\"Swisscom Rechnung Mai 2024\"\nmore"), "Swisscom Rechnung Mai 2024");
        assert_eq!(clean_title("Titel: **Mietvertrag**"), "Mietvertrag");
        assert_eq!(clean_title(""), "");
    }
}
