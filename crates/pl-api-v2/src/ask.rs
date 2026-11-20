//! `/rs/api/v2/ask` – questions to your own archive with source references, actions and history
//! (contract docs/contract/pl-api-v2.md, "PaperRust-Oberfläche").
//!
//! Retrieval and answer are **the same** as with the MCP tool `ask_documents`
//! ([`pl_ai::mcp::knowledge::retrieve`] + [`pl_ai::mcp::knowledge::start_answer`]: full text via
//! v1 `GET /api/documents/?query=`, LLM index with the chat's permission filter, Reciprocal Rank
//! Fusion, chat templates). Only difference: the MCP backend talks to the v1 API over HTTP with
//! the agent's token, here the same v1 call runs **in-process** ([`InProcTransport`]) with
//! the already verified login context (session, token, device token).

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::NaiveDate;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use pl_ai::classifier::IdentityTruncator;
use pl_ai::llm::LlmClient;
use pl_ai::mcp::api::{ApiBackend, MemorySideStore, Transport};
use pl_ai::mcp::backend::{BResult, BackendError, Credential};
use pl_ai::mcp::knowledge::{self, AnswerStart, Citation, IndexState};
use pl_ai::mcp::{Actor, LlmSource, McpService, McpSettings};
use pl_authz::AuthContext;
use pl_core::AppState;
use pl_domain::{DomainError, Principal};
use pl_timeline::DateKind;
use pl_timeline::extract::{DateParse, extract};
use pl_timeline::history;
use pl_timeline::store::{self, KIND_DOCUMENT};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

use crate::V2;
use crate::d2::{self, tl};
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json};

/// Questions per user and minute.
pub const ASK_PER_MINUTE: usize = 20;
/// Largest response of an internal v1 call.
const MAX_INTERNAL_BODY: usize = 32 * 1024 * 1024;

pub fn routes(r: axum::Router<AppState>) -> axum::Router<AppState> {
    r.route("/ask", post(ask))
        .route("/ask/conversations", get(conversations).delete(delete_all))
        .route(
            "/ask/conversations/{uuid}",
            get(conversation).delete(delete_conversation),
        )
}

// ------------------------------------------------------------------------------ Transport

/// v1 calls in-process via the internal router of `pl-server` with the login context of the
/// caller (like the v2 write paths in `pl-store/src/legacy.rs`).
pub struct InProcTransport {
    pub router: axum::Router,
    pub ctx: Arc<AuthContext>,
    pub peer: Option<SocketAddr>,
}

impl Transport for InProcTransport {
    fn send<'a>(
        &'a self,
        _token: &'a str,
        method: Method,
        path: &'a str,
        content_type: Option<&'a str>,
        body: Vec<u8>,
    ) -> BoxFuture<'a, BResult<(u16, Vec<u8>)>> {
        Box::pin(async move {
            let mut ctx = (*self.ctx).clone();
            ctx.http_method = method.clone();
            let mut b = Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, "localhost")
                .header(header::ACCEPT, "application/json; version=10");
            if let Some(ct) = content_type {
                b = b.header(header::CONTENT_TYPE, ct);
            }
            let mut req = b
                .body(Body::from(body))
                .map_err(|_| BackendError::Internal)?;
            req.extensions_mut().insert(Arc::new(ctx));
            if let Some(peer) = self.peer {
                req.extensions_mut()
                    .insert(axum::extract::ConnectInfo(peer));
            }
            let resp = match self.router.clone().oneshot(req).await {
                Ok(r) => r,
                Err(e) => match e {},
            };
            let status = resp.status().as_u16();
            let raw = axum::body::to_bytes(resp.into_body(), MAX_INTERNAL_BODY)
                .await
                .map_err(|_| BackendError::Internal)?;
            Ok((status, raw.to_vec()))
        })
    }
}

/// LLM per question from environment + `ApplicationConfiguration` (like MCP `DbLlmSource`).
struct DbLlm(AppState);

impl LlmSource for DbLlm {
    fn current(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<LlmClient>> + Send + '_>> {
        Box::pin(async move {
            let cfg = pl_ai::server::effective_config(&self.0).await.ok()?;
            if !cfg.ai_enabled {
                return None;
            }
            LlmClient::from_config(&cfg)
                .ok()
                .map(|c| c.with_system_prompt(true))
        })
    }
}

fn actor_of(p: &Principal) -> V2Result<Actor> {
    let u = d2::user(p)?;
    Ok(Actor {
        principal: pl_ai::taxonomy::Principal {
            user_id: Some(u.id),
            is_active: u.is_active,
            is_superuser: u.is_superuser,
            group_ids: u.groups.clone(),
        },
        username: u.username.clone(),
        perms: u.permissions.iter().cloned().collect(),
        token_hint: "v2".into(),
        rate_key: format!("user:{}", u.id),
        credential: Credential::default(),
    })
}

/// MCP service for a question (backend in-process, LLM + LLM index like MCP).
async fn service(state: &AppState, v2: &V2, p: &Principal) -> V2Result<McpService<ApiBackend>> {
    let Some(router) = v2.legacy_read.clone() else {
        return Err(V2Error(DomainError::NotImplemented(
            "ask is not mounted on this server".into(),
        )));
    };
    let ext = p
        .ext
        .downcast_ref::<pl_store::AuthExt>()
        .ok_or(V2Error(DomainError::Unauthenticated))?;
    let transport = InProcTransport {
        router,
        ctx: ext.ctx.clone(),
        peer: ext.peer,
    };
    let backend = Arc::new(ApiBackend::with_transport(
        Arc::new(transport),
        Arc::new(MemorySideStore::default()),
    ));
    let mut settings = McpSettings::default();
    if let Ok(cfg) = pl_ai::server::effective_config(state).await {
        settings.llm_chunk_size = cfg.llm_embedding_chunk_size as usize;
        settings.llm_context_size = cfg.llm_context_size as usize;
    }
    let mut svc = McpService::new(backend, settings, None, Arc::new(IdentityTruncator))
        .with_llm_source(Arc::new(DbLlm(state.clone())));
    svc = match &v2.ask_knowledge {
        Some(k) => svc.with_knowledge(k.clone()),
        None => svc.with_knowledge(Arc::new(pl_ai::server::mcp_index::LlmIndexKnowledge(
            state.clone(),
        ))),
    };
    Ok(svc)
}

// ------------------------------------------------------------------------------ Throttling

static LIMITER: Mutex<Option<HashMap<i64, VecDeque<Instant>>>> = Mutex::new(None);

fn throttle(user: i64) -> V2Result<()> {
    let mut g = LIMITER.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    let q = m.entry(user).or_default();
    while q
        .front()
        .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
    {
        q.pop_front();
    }
    if q.len() >= ASK_PER_MINUTE {
        let wait = q.front().map(|t| {
            60u64
                .saturating_sub(now.duration_since(*t).as_secs())
                .max(1)
        });
        return Err(V2Error(DomainError::Throttled { wait }));
    }
    q.push_back(now);
    Ok(())
}

// ------------------------------------------------------------------------------ Shapes

/// Source in v2 form.
pub fn citation_json(c: &Citation, uuid: Option<Uuid>) -> Value {
    json!({
        "n": c.n,
        "document": uuid,
        "legacy_id": c.document_id,
        "title": c.title,
        "page": c.page,
        "snippet": c.snippet,
        "source": c.source,
        "score": c.score,
    })
}

fn kind_label(k: DateKind, en: bool) -> &'static str {
    match (k, en) {
        (DateKind::Due, false) => "fällig am",
        (DateKind::Due, true) => "due on",
        (DateKind::CancelBy, false) => "Kündigung bis",
        (DateKind::CancelBy, true) => "cancel by",
        (DateKind::ValidUntil, false) => "gültig bis",
        (DateKind::ValidUntil, true) => "valid until",
        (DateKind::WarrantyUntil, false) => "Garantie bis",
        (DateKind::WarrantyUntil, true) => "warranty until",
        (DateKind::ValidFrom, false) => "gültig ab",
        (DateKind::ValidFrom, true) => "valid from",
        (DateKind::Created, false) => "Dokumentdatum",
        (DateKind::Created, true) => "document date",
    }
}

/// A finding of the content detection as action `create_reminder`.
pub fn reminder_action(
    doc: Uuid,
    legacy_id: i64,
    title: &str,
    kind: DateKind,
    date: NaiveDate,
    phrase: &str,
    en: bool,
) -> Value {
    let label = if en {
        format!(
            "Reminder: {title} – {} {}",
            kind_label(kind, true),
            date.format("%Y-%m-%d")
        )
    } else {
        format!(
            "Erinnerung: {title} – {} {}",
            kind_label(kind, false),
            date.format("%d.%m.%Y")
        )
    };
    json!({
        "type": "create_reminder",
        "label": label,
        "document": doc,
        "legacy_id": legacy_id,
        "kind": kind,
        "date": date,
        "phrase": phrase,
        "request": {
            "method": "POST",
            "path": format!("{}/timeline/reminders", crate::PREFIX),
            "body": {"document": doc, "kind": kind, "date": date, "note": phrase},
        },
    })
}

/// Action `open_documents` (documents in citation order, without duplicates).
pub fn open_action(docs: &[(Uuid, i64)], en: bool) -> Value {
    let n = docs.len();
    let label = match (en, n) {
        (true, 1) => "Open the source".to_string(),
        (true, _) => format!("Open {n} sources"),
        (false, 1) => "Quelle öffnen".to_string(),
        (false, _) => format!("{n} Quellen öffnen"),
    };
    json!({
        "type": "open_documents",
        "label": label,
        "documents": docs.iter().map(|d| d.0).collect::<Vec<_>>(),
        "legacy_ids": docs.iter().map(|d| d.1).collect::<Vec<_>>(),
        "query": {"uuids": docs.iter().map(|d| d.0).collect::<Vec<_>>()},
    })
}

/// SSE event (`event: …\ndata: <JSON>\n\n`).
pub fn sse(event: &str, data: &Value) -> String {
    format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(data).unwrap_or_else(|_| "null".into())
    )
}

// ------------------------------------------------------------------------------ POST /ask

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskBody {
    pub question: String,
    #[serde(default)]
    pub conversation: Option<Uuid>,
    #[serde(default)]
    pub documents: Option<Vec<Uuid>>,
    #[serde(default)]
    pub max_passages: Option<usize>,
    #[serde(default)]
    pub answer: Option<bool>,
    #[serde(default)]
    pub stream: Option<bool>,
    /// Language of the action labels (`de` default, `en`).
    #[serde(default)]
    pub lang: Option<String>,
}

struct Job {
    state: AppState,
    svc: McpService<ApiBackend>,
    actor: Actor,
    user_id: i64,
    question: String,
    restrict: Option<Vec<i64>>,
    max: usize,
    want_answer: bool,
    conv_id: i64,
    conv: Uuid,
    message: Uuid,
    en: bool,
}

type Emit = futures::channel::mpsc::UnboundedSender<(String, Value)>;

fn emit(tx: &Emit, ev: &str, data: Value) {
    let _ = tx.unbounded_send((ev.to_string(), data));
}

async fn actions(job: &Job, cits: &[(Citation, Option<Uuid>)]) -> Vec<Value> {
    let mut docs: Vec<(Uuid, i64)> = Vec::new();
    let mut titles: HashMap<i64, String> = HashMap::new();
    for (c, u) in cits {
        if let Some(u) = u
            && !docs.iter().any(|d| d.1 == c.document_id)
        {
            docs.push((*u, c.document_id));
            titles.insert(c.document_id, c.title.clone());
        }
    }
    let mut out = Vec::new();
    if docs.is_empty() {
        return out;
    }
    out.push(open_action(&docs, job.en));
    // Deadlines in the cited documents (content detection on demand)
    let ids: Vec<i64> = docs.iter().map(|d| d.1).collect();
    let pool = job.state.db();
    let (Ok(contents), Ok(existing)) = (
        store::contents(pool, &ids).await,
        store::existing_dates(
            pool,
            job.user_id,
            &ids,
            pl_timeline::refresh::enabled(&d2::setting),
        )
        .await,
    ) else {
        return out;
    };
    let dp = DateParse::from_lookup(d2::setting);
    let today = d2::today(&job.state);
    let mut n = 0;
    for (u, id) in &docs {
        let Some((content, created)) = contents.get(id) else {
            continue;
        };
        for f in extract(content, Some(*created), &dp) {
            if n >= 3 {
                break;
            }
            if f.kind.is_deadline() && f.date >= today && !existing.contains(&(*id, f.date)) {
                out.push(reminder_action(
                    *u,
                    *id,
                    titles.get(id).map(String::as_str).unwrap_or(""),
                    f.kind,
                    f.date,
                    &f.phrase,
                    job.en,
                ));
                n += 1;
            }
        }
    }
    out
}

/// Answer a question; events to `tx` (meta, citations, delta*, actions, done | error).
async fn run(job: Job, tx: Emit) {
    let r = knowledge::retrieve(
        &job.svc,
        &job.actor,
        &job.question,
        job.max,
        job.restrict.as_deref(),
    )
    .await;
    let ids: Vec<i64> = r.citations.iter().map(|c| c.document_id).collect();
    let uuids = match store::uuids(job.state.db(), KIND_DOCUMENT, &ids).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(target: "paperrust.ask", error = %e, "citation uuids");
            emit(
                &tx,
                "error",
                json!({"error": {"code": "internal", "message": "Internal server error."}}),
            );
            return;
        }
    };
    let cits: Vec<(Citation, Option<Uuid>)> = r
        .citations
        .iter()
        .map(|c| (c.clone(), uuids.get(&c.document_id).copied()))
        .collect();
    let cits_json: Vec<Value> = cits.iter().map(|(c, u)| citation_json(c, *u)).collect();
    let state_s = match r.index_state {
        IndexState::Disabled => "disabled",
        IndexState::NotBuilt => "not_built",
        IndexState::Ready => "ready",
    };
    let mut notes = r.notes.clone();
    emit(
        &tx,
        "meta",
        json!({"conversation": job.conv, "message": job.message, "index_state": state_s, "retrieval": r.retrieval, "notes": notes}),
    );
    emit(&tx, "citations", json!({"citations": cits_json}));
    let (mut answer, mut answer_error): (Option<String>, Option<String>) = (None, None);
    if job.want_answer && !r.citations.is_empty() {
        match knowledge::start_answer(&job.svc, &job.actor, &job.question, &r.citations).await {
            AnswerStart::NoLlm => notes.push(knowledge::NOTE_NO_LLM.into()),
            AnswerStart::Empty => answer = Some("Empty Response".into()),
            AnswerStart::Failed(m) => answer_error = Some(m),
            AnswerStart::Stream(mut s) => {
                let mut out = String::new();
                let mut failed = false;
                while let Some(d) = s.next().await {
                    match d {
                        Ok(t) => {
                            if !t.is_empty() {
                                emit(&tx, "delta", json!({"text": t}));
                            }
                            out.push_str(&t);
                        }
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                }
                if failed {
                    answer_error = Some(knowledge::ANSWER_FAILED.into());
                } else {
                    answer = Some(out);
                }
            }
        }
    }
    if r.citations.is_empty() {
        notes.push(knowledge::NOTE_NO_PASSAGES.into());
    }
    let acts = actions(&job, &cits).await;
    emit(&tx, "actions", json!({"actions": acts}));
    let meta = json!({"index_state": state_s, "retrieval": r.retrieval, "notes": notes, "answer_error": answer_error, "question": job.question});
    let saved = history::add(
        job.state.db(),
        job.conv_id,
        Some(job.message),
        "assistant",
        answer.as_deref().unwrap_or(""),
        &Value::Array(cits_json.clone()),
        &Value::Array(acts.clone()),
        &meta,
    )
    .await;
    let created = match saved {
        Ok((_, created)) => created,
        Err(e) => {
            tracing::error!(target: "paperrust.ask", error = %e, "saving the answer failed");
            emit(
                &tx,
                "error",
                json!({"error": {"code": "internal", "message": "Internal server error."}}),
            );
            return;
        }
    };
    emit(
        &tx,
        "done",
        json!({
            "uuid": job.message,
            "conversation": job.conv,
            "role": "assistant",
            "question": job.question,
            "answer": answer,
            "answer_error": answer_error,
            "index_state": state_s,
            "retrieval": r.retrieval,
            "notes": notes,
            "citations": cits_json,
            "actions": acts,
            "created": created,
        }),
    );
}

fn wants_stream(h: &HeaderMap, body: Option<bool>) -> bool {
    body.unwrap_or_else(|| {
        h.get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/event-stream"))
    })
}

pub async fn ask(
    State(state): State<AppState>,
    v2: V2,
    Caller(p): Caller,
    headers: HeaderMap,
    V2Json(b): V2Json<AskBody>,
) -> V2Result<Response> {
    d2::require_view_documents(&p)?;
    let question = b.question.trim().to_string();
    let n = question.chars().count();
    if n == 0 || n > 2000 {
        return Err(V2Error(DomainError::field(
            "question",
            "The question must be 1–2000 characters.",
        )));
    }
    let max = b.max_passages.unwrap_or(8);
    if !(1..=20).contains(&max) {
        return Err(V2Error(DomainError::field(
            "max_passages",
            "Expected 1–20.",
        )));
    }
    if b.documents.as_ref().is_some_and(|d| d.len() > 500) {
        return Err(V2Error(DomainError::field(
            "documents",
            "At most 500 documents.",
        )));
    }
    let en = match b.lang.as_deref().map(str::trim) {
        None | Some("") | Some("de") => false,
        Some("en") => true,
        Some(_) => return Err(V2Error(DomainError::field("lang", "Expected de or en."))),
    };
    d2::ensure(&state).await?;
    throttle(p.user_id)?;
    let restrict = match &b.documents {
        None => None,
        Some(u) => Some(
            store::resolve(state.db(), KIND_DOCUMENT, u)
                .await
                .map_err(tl)?
                .into_values()
                .collect::<Vec<i64>>(),
        ),
    };
    let (conv_id, conv) = history::open(state.db(), p.user_id, b.conversation, &question)
        .await
        .map_err(tl)?;
    history::add(
        state.db(),
        conv_id,
        None,
        "user",
        &question,
        &json!([]),
        &json!([]),
        &json!({}),
    )
    .await
    .map_err(tl)?;
    let svc = service(&state, &v2, &p).await?;
    let job = Job {
        state: state.clone(),
        svc,
        actor: actor_of(&p)?,
        user_id: p.user_id,
        question,
        restrict,
        max,
        want_answer: b.answer.unwrap_or(true),
        conv_id,
        conv,
        message: Uuid::new_v4(),
        en,
    };
    let (tx, rx) = futures::channel::mpsc::unbounded::<(String, Value)>();
    let task = tokio::spawn(run(job, tx));
    if wants_stream(&headers, b.stream) {
        let body = rx
            .map(|(ev, data)| Ok::<Bytes, std::convert::Infallible>(Bytes::from(sse(&ev, &data))));
        let mut r = Response::new(Body::from_stream(body));
        let h = r.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        h.insert("x-accel-buffering", HeaderValue::from_static("no"));
        return Ok(r);
    }
    let events: Vec<(String, Value)> = rx.collect().await;
    let _ = task.await;
    match events
        .into_iter()
        .rev()
        .find(|(e, _)| e == "done" || e == "error")
    {
        Some((e, data)) if e == "done" => Ok(json(&json!({"conversation": conv, "message": data}))),
        _ => Err(V2Error(DomainError::Internal("ask failed".into()))),
    }
}

// ------------------------------------------------------------------------------ History

pub async fn conversations(
    State(state): State<AppState>,
    Caller(p): Caller,
    Query(q): Query<HashMap<String, String>>,
) -> V2Result<Response> {
    d2::user(&p)?;
    let limit = d2::parse_int("limit", q.get("limit"), 50, 1, 200)?;
    let (items, next) = history::list(
        state.db(),
        p.user_id,
        limit,
        q.get("cursor").map(String::as_str),
    )
    .await
    .map_err(tl)?;
    Ok(json(&json!({"items": items, "next_cursor": next})))
}

/// Restrict sources/actions to currently visible documents.
fn filter_visible(v: &Value, visible: &HashSet<i64>, key: &str) -> Value {
    let Some(a) = v.as_array() else {
        return json!([]);
    };
    let mut out = Vec::new();
    for x in a {
        match x.get("type").and_then(Value::as_str) {
            Some("open_documents") => {
                let ids: Vec<i64> = x["legacy_ids"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default();
                let docs: Vec<Value> = x["documents"].as_array().cloned().unwrap_or_default();
                let keep: Vec<usize> = ids
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| visible.contains(i))
                    .map(|(n, _)| n)
                    .collect();
                if keep.is_empty() {
                    continue;
                }
                let mut y = x.clone();
                let d: Vec<Value> = keep.iter().filter_map(|n| docs.get(*n).cloned()).collect();
                y["documents"] = Value::Array(d.clone());
                y["legacy_ids"] = json!(keep.iter().map(|n| ids[*n]).collect::<Vec<_>>());
                y["query"] = json!({"uuids": d});
                out.push(y);
            }
            _ => {
                if x.get(key)
                    .and_then(Value::as_i64)
                    .is_some_and(|i| visible.contains(&i))
                {
                    out.push(x.clone());
                }
            }
        }
    }
    Value::Array(out)
}

fn legacy_ids(v: &Value, acc: &mut Vec<i64>) {
    if let Some(a) = v.as_array() {
        for x in a {
            if let Some(i) = x.get("legacy_id").and_then(Value::as_i64) {
                acc.push(i);
            }
            if let Some(l) = x.get("legacy_ids").and_then(Value::as_array) {
                acc.extend(l.iter().filter_map(Value::as_i64));
            }
        }
    }
}

pub async fn conversation(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(uuid): Path<String>,
) -> V2Result<Response> {
    d2::user(&p)?;
    let u = Uuid::parse_str(&uuid).map_err(|_| V2Error(DomainError::NotFound))?;
    let (c, msgs) = history::get(state.db(), p.user_id, u)
        .await
        .map_err(tl)?
        .ok_or(V2Error(DomainError::NotFound))?;
    let mut ids = Vec::new();
    for m in &msgs {
        legacy_ids(&m.citations, &mut ids);
        legacy_ids(&m.actions, &mut ids);
    }
    let visible = match d2::require_view_documents(&p) {
        Ok(_) => store::visible_documents(state.db(), &d2::visibility(&p)?, &ids)
            .await
            .map_err(tl)?,
        Err(_) => HashSet::new(),
    };
    let messages: Vec<Value> = msgs
        .iter()
        .map(|m| {
            json!({
                "uuid": m.uuid,
                "role": m.role,
                "content": m.content,
                "citations": filter_visible(&m.citations, &visible, "legacy_id"),
                "actions": filter_visible(&m.actions, &visible, "legacy_id"),
                "meta": m.meta,
                "created": m.created,
            })
        })
        .collect();
    Ok(json(&json!({
        "uuid": c.uuid,
        "title": c.title,
        "created": c.created,
        "updated": c.updated,
        "message_count": c.message_count,
        "messages": messages,
    })))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    Caller(p): Caller,
    Path(uuid): Path<String>,
) -> V2Result<Response> {
    d2::user(&p)?;
    let u = Uuid::parse_str(&uuid).map_err(|_| V2Error(DomainError::NotFound))?;
    if !history::delete(state.db(), p.user_id, u)
        .await
        .map_err(tl)?
    {
        return Err(V2Error(DomainError::NotFound));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn delete_all(State(state): State<AppState>, Caller(p): Caller) -> V2Result<Response> {
    d2::user(&p)?;
    history::delete_all(state.db(), p.user_id)
        .await
        .map_err(tl)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_format() {
        let s = sse("delta", &json!({"text": "a\nb"}));
        assert_eq!(
            s, "event: delta\ndata: {\"text\":\"a\\nb\"}\n\n",
            "JSON einzeilig, Zeilenumbruch maskiert"
        );
        assert!(s.ends_with("\n\n"));
    }

    #[test]
    fn actions_shape() {
        let u = Uuid::from_u128(5);
        let a = reminder_action(
            u,
            12,
            "Swisscom",
            DateKind::Due,
            NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
            "zahlbar bis 30.11.2026",
            false,
        );
        assert_eq!(a["type"], "create_reminder");
        assert_eq!(a["label"], "Erinnerung: Swisscom – fällig am 30.11.2026");
        assert_eq!(a["request"]["path"], "/rs/api/v2/timeline/reminders");
        assert_eq!(a["request"]["body"]["date"], "2026-11-30");
        assert_eq!(a["request"]["body"]["kind"], "due");
        let o = open_action(&[(u, 12), (Uuid::from_u128(6), 13)], true);
        assert_eq!(o["label"], "Open 2 sources");
        assert_eq!(o["legacy_ids"], json!([12, 13]));
        assert_eq!(o["query"]["uuids"][1], Uuid::from_u128(6).to_string());
    }

    #[test]
    fn visibility_filter() {
        let cits = json!([{"n":1,"legacy_id":1},{"n":2,"legacy_id":2}]);
        let acts = json!([
            {"type":"open_documents","documents":["a","b"],"legacy_ids":[1,2],"query":{"uuids":["a","b"]}},
            {"type":"create_reminder","legacy_id":2},
            {"type":"open_documents","documents":["b"],"legacy_ids":[2]}
        ]);
        let vis: HashSet<i64> = [1].into();
        assert_eq!(
            filter_visible(&cits, &vis, "legacy_id"),
            json!([{"n":1,"legacy_id":1}])
        );
        let f = filter_visible(&acts, &vis, "legacy_id");
        assert_eq!(f.as_array().unwrap().len(), 1);
        assert_eq!(f[0]["documents"], json!(["a"]));
        assert_eq!(f[0]["query"]["uuids"], json!(["a"]));
        let mut ids = Vec::new();
        legacy_ids(&acts, &mut ids);
        assert_eq!(ids, vec![1, 2, 2, 2]);
    }

    #[test]
    fn throttling() {
        let user = 987_654;
        for _ in 0..ASK_PER_MINUTE {
            throttle(user).unwrap();
        }
        let e = throttle(user).unwrap_err();
        assert!(matches!(e.0, DomainError::Throttled { .. }));
        throttle(user + 1).unwrap();
    }

    #[test]
    fn stream_negotiation() {
        let mut h = HeaderMap::new();
        assert!(!wants_stream(&h, None));
        assert!(wants_stream(&h, Some(true)));
        h.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        assert!(wants_stream(&h, None));
        assert!(!wants_stream(&h, Some(false)));
    }
}
