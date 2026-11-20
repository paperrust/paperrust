//! `POST /api/documents/chat/` – upstream `documents/views.py::ChatStreamingView` and
//! `paperless_ai/chat.py::stream_chat_with_documents` (v3.4.2).
//!
//! `@method_decorator([ensure_csrf_cookie, cache_control(no_cache=True)], name="dispatch")`:
//! **every** response (also 401/403/400) gets `Cache-Control: no-cache` and the CSRF cookie.
//! Body: raw text deltas, `Content-Type: text/event-stream` (no SSE framing), at the end
//! optionally the reference trailer.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use pl_authz::models::DOCUMENT;
use pl_authz::objects::user_is_unrestricted;
use pl_authz::perms::{has_perms_owner_aware, ObjRef, VIEW_DOCUMENTS_PERMISSIONS};
use pl_authz::MaybeUser;
use pl_core::{ApiError, AppState, RequestContext};
use serde_json::{json, Map, Value};

use super::data;
use crate::chat::{document_references, format_metadata_trailer, permitted_documents_filter, retain_permitted, CHAT_ERROR_MESSAGE, CHAT_MAX_QUESTION, CHAT_NO_CONTENT_MESSAGE, CHAT_RETRIEVER_TOP_K};
use crate::config::AiConfig;
use crate::embedding::Embedder;
use crate::llm::LlmClient;
use crate::tokenizer::PromptHelper;
use crate::vector_store::{DocFilter, VecStore};

/// `CSRF_COOKIE_AGE` (Django default, one year).
const CSRF_COOKIE_AGE: i64 = 31_449_600;

fn http_date(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// `get_token(request)` + `CsrfViewMiddleware._set_csrf_cookie`.
fn csrf_cookie(state: &AppState, headers: &HeaderMap) -> Option<HeaderValue> {
    let name = &state.config().csrf_cookie_name;
    let cookies = pl_authz::csrf::parse_cookies(headers);
    let secret = match pl_authz::csrf::cookie(&cookies, name) {
        Some(c) if pl_authz::csrf::check_token_format(c).is_ok() => {
            if c.len() == pl_authz::csrf::CSRF_TOKEN_LENGTH {
                pl_authz::csrf::unmask_cipher_token(c).unwrap_or_default()
            } else {
                c.to_string()
            }
        }
        _ => pl_authz::hashers::get_random_string(pl_authz::csrf::CSRF_SECRET_LENGTH),
    };
    let exp = http_date(chrono::Utc::now() + chrono::Duration::seconds(CSRF_COOKIE_AGE));
    HeaderValue::from_str(&format!("{name}={secret}; expires={exp}; Max-Age={CSRF_COOKIE_AGE}; Path=/; SameSite=Lax")).ok()
}

fn decorate(state: &AppState, headers: &HeaderMap, mut r: Response) -> Response {
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Some(c) = csrf_cookie(state, headers) {
        r.headers_mut().append(SET_COOKIE, c);
    }
    pl_core::route::patch_vary(r.headers_mut(), &["Cookie"]);
    r
}

/// `ChatStreamingSerializer` (`q`: CharField max 4000, `document_id`: IntegerField, null allowed).
fn validate(data: &Value) -> Result<(String, Option<i64>), ApiError> {
    let mut errs = Map::new();
    let empty = Map::new();
    let obj = data.as_object().unwrap_or(&empty);
    let mut q = String::new();
    match obj.get("q") {
        None => {
            errs.insert("q".into(), json!(["This field is required."]));
        }
        Some(Value::Null) => {
            errs.insert("q".into(), json!(["This field may not be null."]));
        }
        Some(Value::String(s)) => q = crate::tokenizer::py_strip(s).to_string(),
        Some(Value::Number(n)) => q = crate::server::py_number_str(n),
        Some(_) => {
            errs.insert("q".into(), json!(["Not a valid string."]));
        }
    }
    if !errs.contains_key("q") {
        if q.is_empty() {
            errs.insert("q".into(), json!(["This field may not be blank."]));
        } else if q.chars().count() > CHAT_MAX_QUESTION {
            errs.insert("q".into(), json!([format!("Ensure this field has no more than {CHAT_MAX_QUESTION} characters.")]));
        }
    }
    let mut doc_id = None;
    match obj.get("document_id") {
        None | Some(Value::Null) => {}
        Some(v) => {
            let s = match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(crate::server::py_number_str(n)),
                _ => None,
            };
            // int(re.sub(r"\.0*\s*$", "", str(data)))
            let parsed = s.and_then(|s| {
                if s.chars().count() > 1000 {
                    return None;
                }
                let t = regex::Regex::new(r"\.0*\s*$").ok()?.replace(&s, "").to_string();
                crate::server::py_int(&t)
            });
            match parsed {
                Some(i) => doc_id = Some(i),
                None => {
                    errs.insert("document_id".into(), json!(["A valid integer is required."]));
                }
            }
        }
    }
    if !errs.is_empty() {
        return Err(ApiError::validation(Value::Object(errs)));
    }
    Ok((q, doc_id))
}

/// `request.data` with DRF's default parsers (JSONParser, FormParser, MultiPartParser):
/// JSON as is; a form body reduced to what `ChatStreamingSerializer`'s fields read for HTML input
/// (`q`: last value; `document_id`: last value, `""` → `null` because of `allow_null`); any other
/// content type → 415. Empty body → `{}` (`Request._parse` without stream).
pub(crate) async fn request_data(headers: &HeaderMap, body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let ct_raw = headers.get(CONTENT_TYPE).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    let media = ct_raw.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let pairs: Vec<(String, String)> = match media.as_str() {
        "application/json" => {
            return serde_json::from_slice(body).map_err(|e| ApiError::json_body_parse_error(body, e));
        }
        "application/x-www-form-urlencoded" => pl_core::QueryParams::parse(&String::from_utf8_lossy(body)).pairs().to_vec(),
        "multipart/form-data" => multipart_text_fields(headers, body).await?,
        _ => return Err(ApiError::unsupported_media_type(&ct_raw)),
    };
    let last = |k: &str| pairs.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let mut m = Map::new();
    if let Some(q) = last("q") {
        m.insert("q".into(), Value::String(q));
    }
    if let Some(d) = last("document_id") {
        m.insert("document_id".into(), if d.is_empty() { Value::Null } else { Value::String(d) });
    }
    Ok(Value::Object(m))
}

/// Text fields of a multipart body (file parts skipped), as DRF's `MultiPartParser` delivers them.
async fn multipart_text_fields(headers: &HeaderMap, body: &Bytes) -> Result<Vec<(String, String)>, ApiError> {
    use axum::extract::FromRequest;
    let mut req = axum::http::Request::new(Body::from(body.clone()));
    *req.headers_mut() = headers.clone();
    let mut mp = axum::extract::Multipart::from_request(req, &())
        .await
        .map_err(|_| ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))?;
    let mut out = Vec::new();
    loop {
        match mp.next_field().await {
            Ok(Some(f)) => {
                let name = f.name().unwrap_or("").to_string();
                if f.file_name().is_some() {
                    continue;
                }
                out.push((name, f.text().await.unwrap_or_default()));
            }
            Ok(None) => break,
            Err(_) => return Err(ApiError::parse_error(Some("Multipart form parse error - Invalid multipart body"))),
        }
    }
    Ok(out)
}

/// Handler.
pub async fn chat(State(state): State<AppState>, ctx: RequestContext, auth: MaybeUser, body: Bytes) -> Response {
    let headers = ctx.headers.clone();
    let r = match chat_inner(&state, &ctx, &auth, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    decorate(&state, &headers, r)
}

async fn chat_inner(state: &AppState, ctx: &RequestContext, auth: &MaybeUser, body: &Bytes) -> Result<Response, ApiError> {
    // (IsAuthenticated, ViewDocumentsPermissions)
    let user = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS)?.clone();
    let cfg = super::effective_config(state).await?;
    if !cfg.ai_enabled {
        return Ok(ApiError::django_text(StatusCode::BAD_REQUEST, "AI is required for this feature").into_response());
    }
    let data = request_data(&ctx.headers, body).await?;
    let (q, doc_id) = validate(&data)?;
    let db = state.db();
    let (docs, unrestricted): (Vec<i64>, bool) = match doc_id.filter(|d| *d != 0) {
        Some(id) => {
            let Some(doc) = data::load_doc(db, id).await? else {
                return Ok(ApiError::django_text(StatusCode::BAD_REQUEST, "Document not found").into_response());
            };
            if !has_perms_owner_aware(db, Some(&user), "view_document", &DOCUMENT, ObjRef { id: doc.root_id, owner_id: doc.root_owner_id }).await? {
                return Ok(ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions").into_response());
            }
            (vec![doc.id], false)
        }
        None => (data::permitted_doc_ids(db, Some(&user)).await?, user_is_unrestricted(Some(&user))),
    };
    let ui = data::ui_settings(db, user.id).await?;
    let lang = crate::classifier::get_llm_output_language(cfg.llm_output_language.as_deref(), ui.as_ref());
    let stream = stream_chat(state.clone(), cfg, q, docs, unrestricted, lang);
    let mut r = Response::new(Body::from_stream(stream.map(Ok::<_, std::convert::Infallible>)));
    r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    Ok(r)
}

/// `stream_chat_with_documents`: error → [`CHAT_ERROR_MESSAGE`] as the last chunk.
pub fn stream_chat(
    state: AppState,
    cfg: AiConfig,
    q: String,
    docs: Vec<i64>,
    unrestricted: bool,
    lang: Option<String>,
) -> futures::stream::BoxStream<'static, Bytes> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    tokio::spawn(async move {
        if let Err(e) = run(&state, &cfg, &q, &docs, unrestricted, lang.as_deref(), &tx).await {
            tracing::error!(target: "pl_ai", error = %e, "Failed to stream document chat response");
            let _ = tx.send(Bytes::from_static(CHAT_ERROR_MESSAGE.as_bytes())).await;
        }
    });
    tokio_stream_from(rx)
}

fn tokio_stream_from(mut rx: tokio::sync::mpsc::Receiver<Bytes>) -> futures::stream::BoxStream<'static, Bytes> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx)).boxed()
}

async fn run(
    state: &AppState,
    cfg: &AiConfig,
    q: &str,
    docs: &[i64],
    unrestricted: bool,
    lang: Option<&str>,
    tx: &tokio::sync::mpsc::Sender<Bytes>,
) -> Result<(), String> {
    let send = |b: String| async move { tx.send(Bytes::from(b)).await.map_err(|e| e.to_string()) };
    if docs.is_empty() {
        return send(CHAT_NO_CONTENT_MESSAGE.into()).await;
    }
    let db = state.db();
    // upstream fix 62d729e30 (#14754, after v3.4.2): a restricted user is filtered with the
    // shorter of IN(permitted) / NOT IN(indexed − permitted), chosen under the read lock; over
    // 32700 ids the list goes to SQLite as one JSON parameter (vector_store::build_where). The
    // permitted ids come from PostgreSQL as a query result, never as bind parameters.
    let (trashed, permitted): (Option<Vec<i64>>, Option<std::collections::HashSet<i64>>) = if unrestricted {
        (Some(data::trashed_ids(db).await.map_err(|e| e.status().to_string())?), None)
    } else {
        (None, Some(docs.iter().copied().collect()))
    };
    let dir = super::index::index_dir(state);
    let d2 = dir.clone();
    // Shared read lock only while reading the index (upstream fix a4543c03d / #14674, after
    // v3.4.2): everything after retrieval works on the nodes in memory, so a slow LLM stream
    // never holds the lock (a compaction swap waiting for exclusive access is not starved).
    let top = {
        let _r = tokio::task::spawn_blocking(move || crate::locks::RwGuard::read(&d2)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let st = VecStore::open(&dir, None).map_err(|e| e.to_string())?;
        let filters = match (&trashed, &permitted) {
            (Some(t), _) => vec![DocFilter::NotIn(t.clone())],
            (None, Some(p)) => vec![permitted_documents_filter(&st.indexed_document_ids().map_err(|e| e.to_string())?, p)],
            (None, None) => Vec::new(),
        };
        let emb = Embedder::from_config(cfg).map_err(|e| e.to_string())?;
        let qv = emb.embed_query(q).await.map_err(|e| e.to_string())?;
        let mut top = st.query(&qv, CHAT_RETRIEVER_TOP_K, &filters).map_err(|e| e.to_string())?;
        drop(st);
        if let Some(p) = &permitted {
            // an exclusion filter fails open for a document indexed after the snapshot
            retain_permitted(&mut top, p);
        }
        top
    };
    if top.is_empty() {
        tracing::warn!(target: "pl_ai", "No nodes found for the given documents.");
        return send(CHAT_NO_CONTENT_MESSAGE.into()).await;
    }
    // PaperRust: own system text (template `chat_system`), otherwise LLM_SYSTEM_PROMPT
    let system = super::store::load_prompts(db).await.custom(crate::prompt_store::PromptKind::ChatSystem).map(str::to_string);
    let client = LlmClient::from_config(cfg).map_err(|e| e.to_string())?.with_system_text(system.as_deref());
    let cand: Vec<i64> = top.iter().filter_map(|h| h.node.document_id()).collect();
    let allowed_ids: Vec<i64> = cand.iter().copied().filter(|c| docs.contains(c)).collect();
    let allowed = data::doc_titles(db, &allowed_ids).await.map_err(|e| e.status().to_string())?;
    let node_refs: Vec<(Option<i64>, Option<String>)> =
        top.iter().map(|h| (h.node.document_id(), h.node.metadata.get("title").and_then(Value::as_str).map(str::to_string))).collect();
    let refs = document_references(&node_refs, &allowed);
    // response_synthesizer.synthesize(query_str, top_nodes): one retrieval per question (upstream
    // fix 45a2ac487 / #14673, after v3.4.2; v3.4.2 retrieved a second time in RetrieverQueryEngine).
    let chunks: Vec<String> = top.iter().map(|h| h.node.llm_content()).collect();
    if chunks.is_empty() {
        return send("Empty Response".into()).await;
    }
    let helper = PromptHelper::rag(cfg.llm_embedding_chunk_size as usize, cfg.llm_context_size as usize);
    let qa = crate::prompts::chat_qa(lang);
    let rf = crate::prompts::chat_refine(lang);
    let stream = crate::synth::compact_and_refine(&client, &helper, &qa, &rf, q, &chunks).await.map_err(|e| e.to_string())?;
    match stream {
        None => send("Empty Response".into()).await?,
        Some(mut s) => {
            // Security wave 3 AI-N6: defuse delimiter in the LLM output
            let mut guard = crate::chat::TrailerGuard::default();
            while let Some(d) = s.next().await {
                let d = d.map_err(|e| e.to_string())?;
                let d = guard.push(&d);
                if !d.is_empty() {
                    send(d).await?;
                }
            }
            let rest = guard.finish();
            if !rest.is_empty() {
                send(rest).await?;
            }
        }
    }
    if !refs.is_empty() {
        send(format_metadata_trailer(&refs)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod request_data_tests {
    use super::*;

    fn h(ct: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(ct) {
            m.insert(CONTENT_TYPE, v);
        }
        m
    }

    /// DRF default parsers for the chat view (formerly "not supported" → 404).
    #[tokio::test]
    async fn parsers_like_drf() {
        let d = request_data(&h("application/json"), &Bytes::from_static(br#"{"q":"hi"}"#)).await.ok();
        assert_eq!(d, Some(json!({"q": "hi"})));
        let d = request_data(&h("application/x-www-form-urlencoded"), &Bytes::from_static(b"q=a&q=b&document_id=")).await.ok();
        assert_eq!(d, Some(json!({"q": "b", "document_id": null})));
        let body = "--B\r\nContent-Disposition: form-data; name=\"q\"\r\n\r\nquestion\r\n--B\r\nContent-Disposition: form-data; name=\"document_id\"\r\n\r\n7\r\n--B--\r\n";
        let d = request_data(&h("multipart/form-data; boundary=B"), &Bytes::from(body)).await.ok();
        assert_eq!(d, Some(json!({"q": "question", "document_id": "7"})));
        assert_eq!(request_data(&h(""), &Bytes::new()).await.ok(), Some(json!({})));
        let e = request_data(&h("text/plain"), &Bytes::from_static(b"x")).await.err().map(|e| e.status());
        assert_eq!(e, Some(StatusCode::UNSUPPORTED_MEDIA_TYPE));
        let e = request_data(&h("application/json"), &Bytes::from_static(b"{")).await.err().map(|e| e.status());
        assert_eq!(e, Some(StatusCode::BAD_REQUEST));
    }
}
