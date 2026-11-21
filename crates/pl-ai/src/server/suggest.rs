//! `GET /api/documents/{id}/ai_suggestions/` – upstream `documents/views.py::DocumentViewSet.ai_suggestions`
//! and `paperless_ai/ai_classifier.py::get_ai_document_classification` (v3.4.2).

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_authz::models::DOCUMENT;
use pl_authz::objects::user_is_active_superuser;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Value, json};

use super::data::{self, AiDoc, Cat};
use crate::classifier::{
    self, MAX_SINGLE_VALUE_CANDIDATES, MAX_TAG_CANDIDATES, TAXONOMY_CANDIDATE_TOP_K, TaxonomyCandidates, Weights, build_prompt_with_rag,
    candidate_id_allowlist, context_blocks, fulltext_weights, node_document_weights, visible_ranked, weigh_neighbours,
};
use crate::compat;
use crate::config::AiConfig;
use crate::llm::{LlmClient, LlmError};
use crate::schema::{self, ClassificationSuggestions};
use crate::tokenizer::TiktokenTruncator;

fn no_cache(mut r: Response) -> Response {
    r.headers_mut().insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

fn parse_pk(pk: &str) -> Result<i64, ApiError> {
    // Django IntegerField.get_prep_value: int(value) (Python int() allows whitespace, sign, "_")
    let t = pk.trim();
    t.replace('_', "")
        .parse::<i64>()
        .ok()
        .filter(|_| !t.starts_with('_') && !t.ends_with('_') && !t.contains("__"))
        .ok_or_else(|| ApiError::internal(format!("Field 'id' expected a number but got '{pk}'.")))
}

/// Candidates + context for own templates/preview (like [`taxonomy_context`]).
pub async fn candidates_for(state: &AppState, cfg: &AiConfig, doc: &AiDoc, user: Option<&User>) -> (TaxonomyCandidates, String) {
    taxonomy_context(state, cfg, doc, user).await
}

/// `get_taxonomy_context(document, user)` → (candidates, context block). Error → empty.
async fn taxonomy_context(state: &AppState, cfg: &AiConfig, doc: &AiDoc, user: Option<&User>) -> (TaxonomyCandidates, String) {
    match taxonomy_context_inner(state, cfg, doc, user).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(target: "pl_ai", document = doc.id, error = %e,
                "Failed to retrieve similar-document context; continuing without taxonomy candidates or similar-document context.");
            (TaxonomyCandidates::default(), String::new())
        }
    }
}

async fn taxonomy_context_inner(
    state: &AppState,
    cfg: &AiConfig,
    doc: &AiDoc,
    user: Option<&User>,
) -> Result<(TaxonomyCandidates, String), String> {
    let db = state.db();
    let su = user_is_active_superuser(user);
    let similar = if cfg.llm_embedding_backend.is_some() {
        let visible = if su { None } else { Some(data::permitted_doc_ids(db, user).await.map_err(|e| e.status().to_string())?) };
        let hits = super::index::retrieve_similar_nodes(
            state,
            cfg,
            doc.id,
            &doc.title,
            &doc.content,
            TAXONOMY_CANDIDATE_TOP_K,
            visible.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let nodes: Vec<(Option<i64>, Option<f64>)> = hits.iter().map(|h| (h.node.document_id(), Some(h.score))).collect();
        node_document_weights(&nodes)
    } else {
        // _fulltext_similar_documents
        let backend = pl_search::routes::backend(state);
        let search_user = if su { None } else { user.map(|u| pl_search::SearchUser { id: u.id, group_ids: u.groups.clone() }) };
        let root = data::root_id(db, doc.id).await.map_err(|e| e.status().to_string())?;
        let su2 = search_user.clone();
        let ids = tokio::task::spawn_blocking(move || backend.more_like_this_ids(root, su2.as_ref(), Some(TAXONOMY_CANDIDATE_TOP_K)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let ids = if su {
            ids
        } else {
            let allowed = data::filter_permitted_docs(db, user, &ids, true).await.map_err(|e| e.status().to_string())?;
            ids.into_iter().filter(|i| allowed.contains(i)).collect()
        };
        fulltext_weights(&ids, TAXONOMY_CANDIDATE_TOP_K)
    };
    let candidates = build_taxonomy_candidates(state, &similar, user).await.map_err(|e| e.status().to_string())?;
    let ids: Vec<i64> = similar.iter().map(|s| s.document_id).collect();
    let by_id = data::context_docs(db, user, &ids).await.map_err(|e| e.status().to_string())?;
    let docs: Vec<(Option<String>, Option<String>, Option<String>)> = ids.iter().filter_map(|i| by_id.get(i).cloned()).collect();
    Ok((candidates, context_blocks(&docs)))
}

/// `build_taxonomy_candidates(similar_documents, user)`.
async fn build_taxonomy_candidates(
    state: &AppState,
    similar: &[classifier::SimilarDocument],
    user: Option<&User>,
) -> Result<TaxonomyCandidates, ApiError> {
    if similar.is_empty() {
        return Ok(TaxonomyCandidates::default());
    }
    let db = state.db();
    let ids: Vec<i64> = similar.iter().map(|s| s.document_id).collect();
    let nb = data::neighbours(db, &ids).await?;
    let w = weigh_neighbours(similar, &nb);
    async fn ranked(
        db: &sqlx::PgPool,
        user: Option<&User>,
        cat: Cat,
        w: &Weights,
        limit: usize,
    ) -> Result<Vec<classifier::TaxonomyCandidate>, ApiError> {
        if w.is_empty() {
            return Ok(Vec::new());
        }
        let names: BTreeMap<i64, String> = data::visible_objects(db, user, cat, &w.ids()).await?.into_iter().collect();
        Ok(visible_ranked(w, &names, limit))
    }
    Ok(TaxonomyCandidates {
        tags: ranked(db, user, Cat::Tags, &w.tags, MAX_TAG_CANDIDATES).await?,
        document_types: ranked(db, user, Cat::DocumentTypes, &w.document_types, MAX_SINGLE_VALUE_CANDIDATES).await?,
        correspondents: ranked(db, user, Cat::Correspondents, &w.correspondents, MAX_SINGLE_VALUE_CANDIDATES).await?,
        storage_paths: ranked(db, user, Cat::StoragePaths, &w.storage_paths, MAX_SINGLE_VALUE_CANDIDATES).await?,
    })
}

/// `get_ai_document_classification(document, user, output_language)`.
pub async fn classify(
    state: &AppState,
    cfg: &AiConfig,
    doc: &AiDoc,
    user: Option<&User>,
    output_language: Option<&str>,
) -> Result<ClassificationSuggestions, LlmError> {
    let (candidates, context) = taxonomy_context(state, cfg, doc, user).await;
    // PaperRust: own template `classification` (paperrust_ai_prompt), otherwise upstream prompt byte-exact
    let custom = super::store::load_prompts(state.db()).await.custom(crate::prompt_store::PromptKind::Classification).map(str::to_string);
    let prompt = match custom {
        Some(t) => {
            let head: String = doc.content.chars().take(classifier::PROMPT_CONTENT_CHARS).collect();
            let content = classifier::Truncator::truncate_content(
                &TiktokenTruncator,
                &head,
                cfg.llm_embedding_chunk_size as usize,
                cfg.llm_context_size as usize,
            );
            let block = classifier::format_taxonomy_for_prompt(&candidates);
            let base = crate::prompt_store::render(
                &t,
                &[
                    ("content", &content),
                    ("filename", doc.filename.as_deref().unwrap_or("")),
                    ("candidates", &block),
                    ("candidate_rules", crate::prompt_store::classification_rules(candidates.any())),
                ],
            );
            let ctx = classifier::Truncator::truncate_content(
                &TiktokenTruncator,
                &context,
                cfg.llm_embedding_chunk_size as usize,
                cfg.llm_context_size as usize,
            );
            crate::prompts::rag_context(&base, &ctx)
        }
        None => build_prompt_with_rag(
            doc.filename.as_deref().unwrap_or(""),
            &doc.content,
            Some(&candidates),
            &context,
            &TiktokenTruncator,
            cfg.llm_embedding_chunk_size as usize,
            cfg.llm_context_size as usize,
        ),
    };
    let client = LlmClient::from_config(cfg)?;
    let sugg = client.run_llm_query(&prompt, &candidate_id_allowlist(&candidates)).await?;
    let sugg = schema::parse_ai_response(sugg);
    let Some(lang) = output_language.filter(|l| !l.is_empty()) else { return Ok(sugg) };
    let model_json = schema::classification_suggestions_to_model(&sugg).model_dump_json();
    let lp = crate::prompts::localization(&classifier::get_language_name(lang), &model_json);
    let loc = client.run_llm_query(&lp, &Default::default()).await?;
    Ok(schema::merge_localized(&sugg, &schema::parse_ai_response(loc)))
}

/// Resolution of a category: visible `existing_ids` (sorted by name) + name matching,
/// deduplicated by pk.
async fn resolve_choice(state: &AppState, user: Option<&User>, cat: Cat, choice: &Value) -> Result<(Vec<i64>, Vec<String>), ApiError> {
    let db = state.db();
    let existing: Vec<i64> =
        choice.get("existing_ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let names: Vec<String> = choice
        .get("new_names")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut matched: Vec<(i64, String)> =
        if existing.is_empty() { Vec::new() } else { data::visible_objects(db, user, cat, &existing).await? };
    if !names.is_empty() {
        let pool = data::permitted_pool(db, user, cat).await?;
        let refs: Vec<Option<&str>> = names.iter().map(|n| Some(n.as_str())).collect();
        matched.extend(compat::match_names_to_pool(&refs, &pool));
    }
    let mut seen = std::collections::HashSet::new();
    matched.retain(|(id, _)| seen.insert(*id));
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let unmatched = compat::extract_unmatched_names(&name_refs, &matched);
    Ok((matched.into_iter().map(|(id, _)| id).collect(), unmatched))
}

/// Handler.
pub async fn ai_suggestions(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
) -> Result<Response, ApiError> {
    // (IsAuthenticated, PaperlessObjectPermissions): GET → documents.view_document
    let user = auth.require_model_perms(&DOCUMENT)?;
    let id = parse_pk(&pk)?;
    let db = state.db();
    let Some(doc) = data::load_doc(db, id).await? else {
        return Err(ApiError::not_found_model("Document"));
    };
    if !has_perms_owner_aware(db, Some(user), "change_document", &DOCUMENT, ObjRef { id: doc.root_id, owner_id: doc.root_owner_id }).await?
    {
        return Ok(no_cache(ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions").into_response()));
    }
    let cfg = super::effective_config(&state).await?;
    if !cfg.ai_enabled {
        return Ok(no_cache(ApiError::django_text(StatusCode::BAD_REQUEST, "AI is required for this feature").into_response()));
    }
    let ui = data::ui_settings(db, user.id).await?;
    let lang = classifier::get_llm_output_language(cfg.llm_output_language.as_deref(), ui.as_ref());
    let backend_key = classifier::llm_cache_backend(
        cfg.llm_backend.map(|b| b.as_str()),
        cfg.llm_model.as_deref(),
        cfg.llm_endpoint.as_deref(),
        lang.as_deref(),
        user.id,
    );
    let suggestions: Value = match super::cache::get_llm_suggestions(&state, doc.id, &backend_key).await {
        Some((_, v)) => {
            super::cache::refresh(&state, doc.id, &backend_key).await;
            v
        }
        None => match classify(&state, &cfg, &doc, Some(user), lang.as_deref()).await {
            Ok(s) => {
                let v = serde_json::to_value(&s).unwrap_or(Value::Null);
                super::cache::set_llm_suggestions(&state, doc.id, &v, &backend_key).await;
                v
            }
            Err(LlmError::InvalidConfig(m)) => {
                tracing::error!(target: "pl_ai", document = doc.id, error = %m, "Invalid AI configuration while generating suggestions");
                return Err(ApiError::validation(json!({"ai": ["Invalid AI configuration."]})));
            }
            Err(LlmError::Timeout) => {
                tracing::error!(target: "pl_ai", document = doc.id, "AI backend timed out while generating suggestions");
                return Ok(no_cache(
                    Drf::with_status(StatusCode::SERVICE_UNAVAILABLE, json!({"ai": ["AI backend request timed out."]}))
                        .indent(ctx.negotiation.indent)
                        .into_response(),
                ));
            }
            Err(LlmError::Provider(s)) => {
                tracing::error!(target: "pl_ai", document = doc.id, status = s, "AI backend rejected the request");
                return Ok(no_cache(
                    Drf::with_status(StatusCode::BAD_GATEWAY, json!({"ai": ["AI backend rejected the request. Check logs for details."]}))
                        .indent(ctx.negotiation.indent)
                        .into_response(),
                ));
            }
            // tests/chaos C4: AI service unreachable → 503 with a clear message instead of "Server Error (500)"
            // (upstream: unhandled exception → 500). The classic suggestions remain available under
            // /api/documents/{id}/suggestions/.
            Err(LlmError::Transport(m)) => {
                tracing::error!(target: "pl_ai", document = doc.id, error = %m, "AI backend not reachable while generating suggestions");
                return Ok(no_cache(
                    Drf::with_status(StatusCode::SERVICE_UNAVAILABLE, json!({"ai": ["AI backend not reachable."]}))
                        .indent(ctx.negotiation.indent)
                        .into_response(),
                ));
            }
        },
    };
    let mut out = serde_json::Map::new();
    out.insert("title".into(), suggestions.get("title").cloned().unwrap_or(Value::Null));
    for (cat, ids_key, sugg_key) in [
        (Cat::Tags, "tags", "suggested_tags"),
        (Cat::Correspondents, "correspondents", "suggested_correspondents"),
        (Cat::DocumentTypes, "document_types", "suggested_document_types"),
        (Cat::StoragePaths, "storage_paths", "suggested_storage_paths"),
    ] {
        let choice = suggestions.get(cat.key()).cloned().unwrap_or(Value::Null);
        let (ids, unmatched) = resolve_choice(&state, Some(user), cat, &choice).await?;
        out.insert(ids_key.into(), json!(ids));
        out.insert(sugg_key.into(), json!(unmatched));
    }
    out.insert("dates".into(), suggestions.get("dates").cloned().unwrap_or(json!([])));
    Ok(no_cache(Drf::ok(Value::Object(out)).indent(ctx.negotiation.indent).into_response()))
}
