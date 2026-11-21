//! Routes of pl-ai (see docs/contract/pl-ai.md).
//!
//! | id | Method | Path | Source |
//! |---|---|---|---|
//! | `documents.ai_suggestions` | GET | `/api/documents/{id}/ai_suggestions/` | upstream `DocumentViewSet.ai_suggestions` |
//! | `documents.chat` | POST | `/api/documents/chat/` | upstream `ChatStreamingView` |
//! | `ai.review.list` | GET | `/api/x-ai/review/` | extension |
//! | `ai.review.retrieve` | GET | `/api/x-ai/review/{id}/` | extension |
//! | `ai.review.apply` | POST | `/api/x-ai/review/{id}/apply/` | extension |
//! | `ai.review.reject` | POST | `/api/x-ai/review/{id}/reject/` | extension |
//! | `ai.review.undo` | POST | `/api/x-ai/review/{id}/undo/` | extension |
//! | `ai.run` | POST | `/api/x-ai/run/` | extension |
//! | `ai.mcp.post` / `.get` / `.delete` | POST/GET/DELETE | `/mcp` | extension (only with `PAPERRUST_AI_MCP_ENABLED=yes`) |
//! | `ai.mcp_scopes.list` / `.create` | GET/POST | `/api/x-ai/mcp-scopes/` | extension (superuser: MCP token scopes) |
//! | `ai.mcp_scopes.delete` | DELETE | `/api/x-ai/mcp-scopes/{id}/` | extension (superuser) |
//! | `ai.status` | GET | `/api/x-ai/status/` | extension (AI on?, open suggestions) |
//! | `ai.prompts.*` | GET/PUT/DELETE/POST | `/api/x-ai/prompts/[{kind}/[preview/]]` | extension (own prompts) |
//! | `ai.options.*` | GET/PATCH | `/api/x-ai/options/` | extension (AI options) |
//! | `ai.provider.check` | POST | `/api/x-ai/provider/check/` | extension (connection test, model list) |
//! | `ai.jobs.*` | GET/POST | `/api/x-ai/jobs/…` | extension (post-process archive) |
//! | `ai.assist.*` | POST | `/api/x-ai/assist/{verdict,workflow-draft}/` | extension (AI condition, workflow draft) |

use std::sync::{Arc, OnceLock};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use pl_core::route::drf_view;
use pl_core::{ApiError, ApiRoute, AppState};

use crate::config::AiConfig;
use crate::llm::LlmClient;
use crate::mcp::api::ApiBackend;
use crate::mcp::{McpService, McpSettings, http};
use crate::pg::PgSideStore;
use crate::tokenizer::TiktokenTruncator;

type Svc = McpService<ApiBackend>;

static SERVICE: OnceLock<Option<Arc<Svc>>> = OnceLock::new();

fn api_base() -> String {
    if let Some(b) = pl_core::config::var("PAPERRUST_AI_API_BASE")
        && !b.trim().is_empty()
    {
        return b;
    }
    let port = pl_core::config::var("PAPERLESS_PORT").unwrap_or_else(|| "8000".into());
    format!("http://127.0.0.1:{port}")
}

fn service(state: &AppState) -> Option<Arc<Svc>> {
    SERVICE
        .get_or_init(|| {
            let cfg = match AiConfig::from_env() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(target: "pl_ai", error = %e, "invalid AI configuration; /mcp disabled");
                    return None;
                }
            };
            if !cfg.rs.mcp_enabled {
                return None;
            }
            let llm = if cfg.ai_enabled { LlmClient::from_config(&cfg).ok().map(|c| c.with_system_prompt(true)) } else { None };
            let settings = McpSettings {
                match_threshold: cfg.rs.match_threshold,
                rate_limit_per_minute: cfg.rs.mcp_rate_limit_per_minute,
                max_creates_per_hour: cfg.rs.mcp_max_creates_per_hour,
                allowed_origins: cfg.rs.mcp_allowed_origins.clone(),
                llm_chunk_size: cfg.llm_embedding_chunk_size as usize,
                llm_context_size: cfg.llm_context_size as usize,
                trusted_proxies: state.config().trusted_proxies.clone(),
                tool_groups: cfg.rs.mcp_tool_groups.clone(),
                allow_url_fetch: cfg.rs.mcp_allow_url_fetch,
                max_upload_bytes: cfg.rs.mcp_max_upload_bytes as usize,
                max_ingests_per_hour: cfg.rs.mcp_max_ingests_per_hour,
                ..Default::default()
            };
            let store = Arc::new(PgSideStore::new(state.db().clone()));
            let backend = Arc::new(ApiBackend::new(&api_base(), store));
            Some(Arc::new(
                McpService::new(backend, settings, llm, Arc::new(TiktokenTruncator))
                    .with_llm_source(Arc::new(DbLlmSource(state.clone())))
                    .with_knowledge(Arc::new(crate::server::mcp_index::LlmIndexKnowledge(state.clone()))),
            ))
        })
        .clone()
}

/// Security wave 3 AI-N1: LLM client per MCP tool call from `AIConfig()` (environment +
/// `ApplicationConfiguration`), like `ai_suggestions`/`chat`. AI disabled in the UI → no client.
struct DbLlmSource(AppState);

impl crate::mcp::LlmSource for DbLlmSource {
    fn current(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<LlmClient>> + Send + '_>> {
        Box::pin(async move {
            let cfg = crate::server::effective_config(&self.0).await.ok()?;
            if !cfg.ai_enabled {
                return None;
            }
            LlmClient::from_config(&cfg).ok().map(|c| c.with_system_prompt(true))
        })
    }
}

/// Body limit for `POST /mcp`: base64 of the largest file + JSON framing.
fn mcp_body_limit() -> usize {
    let max = AiConfig::from_env().map(|c| c.rs.mcp_max_upload_bytes as usize).unwrap_or(1_572_864);
    max.saturating_mul(4) / 3 + 256 * 1024
}

fn disabled() -> Response {
    ApiError::not_found().into_response()
}

async fn post_h(State(state): State<AppState>, ext: axum::http::Extensions, headers: HeaderMap, body: Bytes) -> Response {
    match service(&state) {
        Some(s) => http::handle_post_from(&s, http::peer_ip(&ext), &headers, &body).await,
        None => disabled(),
    }
}

async fn get_h(State(state): State<AppState>) -> Response {
    match service(&state) {
        Some(_) => http::handle_get(),
        None => disabled(),
    }
}

async fn delete_h(State(state): State<AppState>, ext: axum::http::Extensions, headers: HeaderMap) -> Response {
    match service(&state) {
        Some(s) => http::handle_delete_from(&s, http::peer_ip(&ext), &headers).await,
        None => disabled(),
    }
}

/// All routes of pl-ai.
pub fn routes() -> Vec<ApiRoute> {
    use crate::server::{assist, chat, jobs, review, store, suggest};
    const ALLOW_GET: &str = "GET, HEAD, OPTIONS";
    const ALLOW_POST: &str = "POST, OPTIONS";
    vec![
        ApiRoute::new(
            "documents.ai_suggestions",
            Method::GET,
            "/api/documents/{id}/ai_suggestions/",
            "src/documents/views.py::DocumentViewSet.ai_suggestions",
            drf_view(get(suggest::ai_suggestions), ALLOW_GET),
        ),
        ApiRoute::new(
            "documents.chat",
            Method::POST,
            "/api/documents/chat/",
            "src/documents/views.py::ChatStreamingView.post",
            drf_view(post(chat::chat), ALLOW_POST),
        )
        // DRF default parsers: JSON, form and multipart (parsed in `chat::request_data`)
        .accepts_drf_parsers(),
        ApiRoute::new(
            "ai.review.list",
            Method::GET,
            "/api/x-ai/review/",
            "pl-ai extension (AI review list)",
            drf_view(get(review::list), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.review.retrieve",
            Method::GET,
            "/api/x-ai/review/{id}/",
            "pl-ai extension (AI review list)",
            drf_view(get(review::retrieve), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.review.apply",
            Method::POST,
            "/api/x-ai/review/{id}/apply/",
            "pl-ai extension (AI review list)",
            drf_view(post(review::apply), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.review.reject",
            Method::POST,
            "/api/x-ai/review/{id}/reject/",
            "pl-ai extension (AI review list)",
            drf_view(post(review::reject), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.review.undo",
            Method::POST,
            "/api/x-ai/review/{id}/undo/",
            "pl-ai extension (AI review list)",
            drf_view(post(review::undo), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.run",
            Method::POST,
            "/api/x-ai/run/",
            "pl-ai extension (automatic processing, manual run)",
            drf_view(post(review::run_view), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.mcp.post",
            Method::POST,
            "/mcp",
            "pl-ai extension (MCP Streamable HTTP)",
            // add_document sends files base64-encoded (limit PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES);
            // pl-server forwards bodies over 2.5 MiB as long as /mcp is not a streaming route.
            post(post_h).layer(axum::extract::DefaultBodyLimit::max(mcp_body_limit())),
        ),
        ApiRoute::new("ai.mcp.get", Method::GET, "/mcp", "pl-ai extension (MCP Streamable HTTP)", get(get_h)),
        ApiRoute::new("ai.mcp.delete", Method::DELETE, "/mcp", "pl-ai extension (MCP Streamable HTTP)", delete(delete_h)),
        ApiRoute::new(
            "ai.mcp_scopes.list",
            Method::GET,
            "/api/x-ai/mcp-scopes/",
            "pl-ai extension (MCP token scopes)",
            drf_view(get(crate::server::mcp_admin::list), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.mcp_scopes.create",
            Method::POST,
            "/api/x-ai/mcp-scopes/",
            "pl-ai extension (MCP token scopes)",
            drf_view(post(crate::server::mcp_admin::create), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.status",
            Method::GET,
            "/api/x-ai/status/",
            "pl-ai extension (AI status)",
            drf_view(get(store::status), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.prompts.list",
            Method::GET,
            "/api/x-ai/prompts/",
            "pl-ai extension (custom prompts)",
            drf_view(get(store::prompts_list), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.prompts.update",
            Method::PUT,
            "/api/x-ai/prompts/{kind}/",
            "pl-ai extension (custom prompts)",
            drf_view(put(store::prompts_put), "PUT, DELETE, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.prompts.reset",
            Method::DELETE,
            "/api/x-ai/prompts/{kind}/",
            "pl-ai extension (custom prompts)",
            drf_view(delete(store::prompts_reset), "PUT, DELETE, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.prompts.preview",
            Method::POST,
            "/api/x-ai/prompts/{kind}/preview/",
            "pl-ai extension (custom prompts)",
            drf_view(post(store::prompts_preview), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.options.get",
            Method::GET,
            "/api/x-ai/options/",
            "pl-ai extension (AI options)",
            drf_view(get(store::options_get), "GET, PATCH, HEAD, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.options.patch",
            Method::PATCH,
            "/api/x-ai/options/",
            "pl-ai extension (AI options)",
            drf_view(patch(store::options_patch), "GET, PATCH, HEAD, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.provider.check",
            Method::POST,
            "/api/x-ai/provider/check/",
            "pl-ai extension (provider connection test)",
            drf_view(post(store::provider_check), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.jobs.list",
            Method::GET,
            "/api/x-ai/jobs/",
            "pl-ai extension (archive backfill)",
            drf_view(get(jobs::list), "GET, POST, HEAD, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.jobs.create",
            Method::POST,
            "/api/x-ai/jobs/",
            "pl-ai extension (archive backfill)",
            drf_view(post(jobs::create), "GET, POST, HEAD, OPTIONS"),
        ),
        ApiRoute::new(
            "ai.jobs.preview",
            Method::POST,
            "/api/x-ai/jobs/preview/",
            "pl-ai extension (archive backfill)",
            drf_view(post(jobs::preview), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.jobs.retrieve",
            Method::GET,
            "/api/x-ai/jobs/{id}/",
            "pl-ai extension (archive backfill)",
            drf_view(get(jobs::retrieve), ALLOW_GET),
        ),
        ApiRoute::new(
            "ai.jobs.cancel",
            Method::POST,
            "/api/x-ai/jobs/{id}/cancel/",
            "pl-ai extension (archive backfill)",
            drf_view(post(jobs::cancel), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.jobs.resume",
            Method::POST,
            "/api/x-ai/jobs/{id}/resume/",
            "pl-ai extension (archive backfill)",
            drf_view(post(jobs::resume), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.assist.verdict",
            Method::POST,
            "/api/x-ai/assist/verdict/",
            "pl-ai extension (AI condition)",
            drf_view(post(assist::verdict_view), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.assist.workflow_draft",
            Method::POST,
            "/api/x-ai/assist/workflow-draft/",
            "pl-ai extension (workflow draft from description)",
            drf_view(post(assist::workflow_draft_view), ALLOW_POST),
        ),
        ApiRoute::new(
            "ai.mcp_scopes.delete",
            Method::DELETE,
            "/api/x-ai/mcp-scopes/{id}/",
            "pl-ai extension (MCP token scopes)",
            drf_view(delete(crate::server::mcp_admin::remove), "DELETE, OPTIONS"),
        ),
    ]
}
