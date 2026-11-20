//! Production backend of the MCP server: talks to the **REST API of paperless** (via
//! PaperRust or the Django proxy) with the agent's token.
//!
//! Why via the API and not directly into the DB?
//! * Same permission check, validation, signals, audit log (django-auditlog), Celery tasks
//!   (search index, LLM index), tag tree (django-treenode) as with any UI change – without duplicating this
//!   logic in pl-ai and without a cross-dependency on pl-objects/pl-documents-write
//!   (ARCHITECTURE.md: no cross-dependencies between domain crates).
//! * Works regardless of whether a route already runs in Rust or still in Django.
//!
//! Own data (aliases, AI audit) lives in a [`SideStore`] (memory or PostgreSQL).
//!
//! Endpoints used (API version 10): `GET /api/ui_settings/` (user + permissions),
//! `GET /api/{tags,correspondents,document_types,storage_paths,custom_fields}/`,
//! `GET /api/<kind>/<id>/`, `POST /api/<kind>/`, `GET /api/documents/?query=…`,
//! `GET /api/documents/<id>/`, `PATCH /api/documents/<id>/`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use super::backend::*;
use crate::alias::MemoryAliasStore;
use crate::taxonomy::{Alias, Principal, TaxonomyEntry, TaxonomyKind};

/// Storage for aliases and AI audit.
pub trait SideStore: Send + Sync + 'static {
    /// All aliases of a kind (visibility is filtered by the caller via the targets).
    fn aliases(&self, kind: TaxonomyKind) -> BoxFuture<'_, BResult<Vec<Alias>>>;
    fn add_alias(&self, alias: Alias, actor_id: Option<i64>) -> BoxFuture<'_, BResult<bool>>;
    fn audit(&self, entry: AuditEntry) -> BoxFuture<'_, BResult<()>>;
    /// Scopes of a token (`token_key` = SHA-256 hex, see [`super::scopes::token_key`]), otherwise
    /// of the user; `None` = no entry.
    fn token_scopes(&self, _token_key: &str, _user_id: Option<i64>) -> BoxFuture<'_, BResult<Option<Vec<String>>>> {
        Box::pin(async { Ok(None) })
    }
}

/// In-memory storage (audit additionally into the log `pl_ai::audit`).
#[derive(Default)]
pub struct MemorySideStore {
    aliases: MemoryAliasStore,
    pub audit: Mutex<Vec<AuditEntry>>,
}

impl SideStore for MemorySideStore {
    fn aliases(&self, kind: TaxonomyKind) -> BoxFuture<'_, BResult<Vec<Alias>>> {
        Box::pin(async move { Ok(self.aliases.all(kind)) })
    }
    fn add_alias(&self, alias: Alias, _actor_id: Option<i64>) -> BoxFuture<'_, BResult<bool>> {
        Box::pin(async move { self.aliases.add(alias).map_err(|_| BackendError::Internal) })
    }
    fn audit(&self, entry: AuditEntry) -> BoxFuture<'_, BResult<()>> {
        Box::pin(async move {
            tracing::info!(target: "pl_ai::audit", entry = %serde_json::to_string(&entry).unwrap_or_default(), "ai audit");
            self.audit.lock().map_err(|_| BackendError::Internal)?.push(entry);
            Ok(())
        })
    }
}

/// Way to the REST API: via HTTP with the agent's token ([`HttpTransport`], MCP) or in
/// process with an already verified login context (`/rs/api/v2/ask`, pl-api-v2). Same
/// v1 endpoints, same permission check.
pub trait Transport: Send + Sync + 'static {
    /// One request; `token` = credentials of the actor (HTTP only). → (status, body)
    fn send<'a>(
        &'a self,
        token: &'a str,
        method: reqwest::Method,
        path: &'a str,
        content_type: Option<&'a str>,
        body: Vec<u8>,
    ) -> BoxFuture<'a, BResult<(u16, Vec<u8>)>>;
}

/// HTTP against `base` with `Authorization: Token …` (production path of the MCP server).
pub struct HttpTransport {
    base: String,
    http: reqwest::Client,
}

impl HttpTransport {
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            // Security review 2026-11-19 F11: own API, never via HTTP(S)_PROXY
            .no_proxy()
            .build()
            .unwrap_or_default();
        Self { base: base.trim_end_matches('/').to_string(), http }
    }
}

impl Transport for HttpTransport {
    fn send<'a>(
        &'a self,
        token: &'a str,
        method: reqwest::Method,
        path: &'a str,
        content_type: Option<&'a str>,
        body: Vec<u8>,
    ) -> BoxFuture<'a, BResult<(u16, Vec<u8>)>> {
        Box::pin(async move {
            let mut req = self
                .http
                .request(method, format!("{}{}", self.base, path))
                .header("authorization", format!("Token {token}"))
                .header("accept", "application/json; version=10");
            if let Some(ct) = content_type {
                req = req.header("content-type", ct).body(body);
            }
            let resp = req.send().await.map_err(|e| {
                tracing::warn!(target: "pl_ai::mcp", error = %e.without_url(), "PaperRust API not reachable");
                BackendError::Internal
            })?;
            let status = resp.status().as_u16();
            // Security wave 3 AI-N5: limit the response size
            let raw = crate::llm::read_body_limited(resp, crate::llm::MAX_RESPONSE_BYTES).await.map_err(|_| BackendError::Internal)?;
            Ok((status, raw.to_vec()))
        })
    }
}

/// REST-Backend.
pub struct ApiBackend {
    transport: Arc<dyn Transport>,
    store: Arc<dyn SideStore>,
}

fn kind_path(kind: TaxonomyKind) -> &'static str {
    match kind {
        TaxonomyKind::Correspondent => "correspondents",
        TaxonomyKind::Tag => "tags",
        TaxonomyKind::DocumentType => "document_types",
        TaxonomyKind::StoragePath => "storage_paths",
        TaxonomyKind::CustomField => "custom_fields",
    }
}

/// App label of the paperless models (UiSettingsView returns bare codenames).
const MAIL_MODELS: [&str; 3] = ["mailaccount", "mailrule", "processedmail"];

fn qualify(codename: &str) -> String {
    let model = codename.split_once('_').map(|(_, m)| m).unwrap_or("");
    if MAIL_MODELS.contains(&model) {
        format!("paperless_mail.{codename}")
    } else if ["user", "group", "permission"].contains(&model) {
        format!("auth.{codename}")
    } else {
        format!("documents.{codename}")
    }
}

fn entry_from(kind: TaxonomyKind, v: &Value) -> Option<TaxonomyEntry> {
    Some(TaxonomyEntry {
        kind,
        id: v.get("id")?.as_i64()?,
        name: v.get("name")?.as_str()?.to_string(),
        owner_id: v.get("owner").and_then(Value::as_i64),
        extra: match kind {
            TaxonomyKind::StoragePath => v.get("path").and_then(Value::as_str).map(str::to_string),
            TaxonomyKind::CustomField => v.get("data_type").and_then(Value::as_str).map(str::to_string),
            _ => None,
        },
        document_count: v.get("document_count").and_then(Value::as_i64),
    })
}

fn summary_from(v: &Value) -> Option<DocumentSummary> {
    Some(DocumentSummary {
        id: v.get("id")?.as_i64()?,
        title: v.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        created: v.get("created").and_then(Value::as_str).map(str::to_string),
        correspondent_id: v.get("correspondent").and_then(Value::as_i64),
        document_type_id: v.get("document_type").and_then(Value::as_i64),
        storage_path_id: v.get("storage_path").and_then(Value::as_i64),
        tag_ids: v.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
        archive_serial_number: v.get("archive_serial_number").and_then(Value::as_i64),
        added: v.get("added").and_then(Value::as_str).map(str::to_string),
    })
}

fn client_error(status: u16, body: &Value) -> BackendError {
    match status {
        404 => BackendError::NotFound,
        401 | 403 => BackendError::PermissionDenied(body.get("detail").and_then(Value::as_str).unwrap_or("forbidden").to_string()),
        400 => BackendError::Invalid(serde_json::to_string(body).unwrap_or_default().chars().take(500).collect()),
        _ => BackendError::Internal,
    }
}

const SUMMARY_FIELDS: &str = "id,title,created,added,correspondent,document_type,storage_path,tags,archive_serial_number";

fn query_string(q: &[(String, String)]) -> String {
    q.iter().map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v))).collect::<Vec<_>>().join("&")
}

/// List from a DRF response (paginated or bare list).
fn list_of(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        _ => v.get("results").and_then(Value::as_array).cloned().unwrap_or_default(),
    }
}

fn note_from(v: &Value) -> Option<NoteEntry> {
    Some(NoteEntry {
        id: v.get("id")?.as_i64()?,
        note: v.get("note").and_then(Value::as_str).unwrap_or("").to_string(),
        created: v.get("created").and_then(Value::as_str).map(str::to_string),
        user: match v.get("user") {
            Some(Value::Object(u)) => u.get("username").and_then(Value::as_str).map(str::to_string),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        },
    })
}

fn view_from(v: &Value) -> Option<SavedViewInfo> {
    Some(SavedViewInfo {
        id: v.get("id")?.as_i64()?,
        name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        sort_field: v.get("sort_field").and_then(Value::as_str).map(str::to_string),
        sort_reverse: v.get("sort_reverse").and_then(Value::as_bool).unwrap_or(false),
        filter_rules: v
            .get("filter_rules")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        let t = r.get("rule_type")?.as_i64()?;
                        let val = match r.get("value") {
                            Some(Value::String(s)) => Some(s.clone()),
                            Some(Value::Null) | None => None,
                            Some(other) => Some(other.to_string()),
                        };
                        Some((t, val))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Compact task status from `GET /api/tasks/` (old and new field names).
fn task_from(v: &Value) -> Value {
    let pick = |a: &str, b: &str| v.get(a).filter(|x| !x.is_null()).or_else(|| v.get(b)).cloned().unwrap_or(Value::Null);
    json!({
        "task_id": v.get("task_id"),
        "status": v.get("status"),
        "file_name": pick("task_file_name", "input_data"),
        "result": pick("result", "result_data"),
        "related_document": pick("related_document", "related_document_ids"),
        "duplicate_documents": v.get("duplicate_documents"),
        "date_created": v.get("date_created"),
        "date_done": v.get("date_done"),
    })
}

/// Value for a multipart header (no quotation marks/line breaks).
fn header_safe(s: &str) -> String {
    s.chars().map(|c| if c == '"' || c == '\r' || c == '\n' || c == '\\' { '_' } else { c }).collect()
}

/// `multipart/form-data` body (fields, then the file as `document`).
pub fn multipart_body(boundary: &str, fields: &[(String, String)], file: (&str, &str, &[u8])) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in fields {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n", header_safe(k)).as_bytes());
        b.extend_from_slice(v.as_bytes());
        b.extend_from_slice(b"\r\n");
    }
    let (name, mime, data) = file;
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"document\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            header_safe(name),
            header_safe(mime)
        )
        .as_bytes(),
    );
    b.extend_from_slice(data);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

impl ApiBackend {
    /// `base` e.g. `http://127.0.0.1:8000` (PaperRust itself or Django).
    pub fn new(base: &str, store: Arc<dyn SideStore>) -> Self {
        Self::with_transport(Arc::new(HttpTransport::new(base)), store)
    }

    /// With its own way to the API (e.g. in process, pl-api-v2 `/ask`).
    pub fn with_transport(transport: Arc<dyn Transport>, store: Arc<dyn SideStore>) -> Self {
        Self { transport, store }
    }

    async fn send(&self, token: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> BResult<(u16, Value)> {
        let (ct, bytes) = match body {
            Some(b) => (Some("application/json"), serde_json::to_vec(b).map_err(|_| BackendError::Internal)?),
            None => (None, Vec::new()),
        };
        let (status, raw) = self.transport.send(token, method, path, ct, bytes).await?;
        let v = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        Ok((status, v))
    }

    async fn get_ok(&self, actor: &Actor, path: &str) -> BResult<Value> {
        let (s, v) = self.send(actor.credential.expose(), reqwest::Method::GET, path, None).await?;
        if s == 200 { Ok(v) } else { Err(client_error(s, &v)) }
    }

    async fn all_pages(&self, actor: &Actor, first: &str) -> BResult<Vec<Value>> {
        let mut out = Vec::new();
        let mut path = first.to_string();
        for _ in 0..50 {
            let v = self.get_ok(actor, &path).await?;
            if let Some(r) = v.get("results").and_then(Value::as_array) {
                out.extend(r.iter().cloned());
            }
            match v.get("next").and_then(Value::as_str) {
                Some(next) => {
                    // absolute URL → path + query
                    let p = reqwest::Url::parse(next).map(|u| format!("{}?{}", u.path(), u.query().unwrap_or(""))).map_err(|_| BackendError::Internal)?;
                    path = p;
                }
                None => break,
            }
        }
        Ok(out)
    }

    async fn document_list(&self, actor: &Actor, q: &[(String, String)]) -> BResult<(i64, Vec<DocumentSummary>)> {
        let v = match self.get_ok(actor, &format!("/api/documents/?{}", query_string(q))).await {
            Ok(v) => v,
            Err(BackendError::NotFound) => return Ok((0, Vec::new())), // "Invalid page."
            Err(e) => return Err(e),
        };
        let count = v.get("count").and_then(Value::as_i64).unwrap_or(0);
        let docs = v.get("results").and_then(Value::as_array).map(|a| a.iter().filter_map(summary_from).collect()).unwrap_or_default();
        Ok((count, docs))
    }

    async fn document_raw(&self, actor: &Actor, id: i64) -> BResult<Option<Value>> {
        match self.get_ok(actor, &format!("/api/documents/{id}/")).await {
            Ok(v) => Ok(Some(v)),
            Err(BackendError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn doc_state(v: &Value) -> Value {
    json!({
        "title": v.get("title"), "created": v.get("created"), "correspondent_id": v.get("correspondent"),
        "document_type_id": v.get("document_type"), "storage_path_id": v.get("storage_path"),
        "tag_ids": v.get("tags"), "custom_fields": v.get("custom_fields"),
    })
}

impl Backend for ApiBackend {
    async fn authenticate(&self, token: &str) -> BResult<Option<Actor>> {
        let (s, v) = self.send(token, reqwest::Method::GET, "/api/ui_settings/", None).await?;
        if s == 401 {
            return Ok(None);
        }
        if s == 403 {
            // Token valid, but the user may not even read /api/ui_settings/.
            return Err(BackendError::PermissionDenied("user lacks the permissions required for MCP".into()));
        }
        if s != 200 {
            return Err(BackendError::Internal);
        }
        let u = &v["user"];
        let Some(id) = u.get("id").and_then(Value::as_i64) else { return Ok(None) };
        let perms: BTreeSet<String> =
            v["permissions"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(qualify).collect()).unwrap_or_default();
        Ok(Some(Actor {
            principal: Principal {
                user_id: Some(id),
                is_active: true, // DRF already rejects inactive users with 401/403
                is_superuser: u.get("is_superuser").and_then(Value::as_bool).unwrap_or(false),
                group_ids: u.get("groups").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
            },
            username: u.get("username").and_then(Value::as_str).unwrap_or("").to_string(),
            perms,
            token_hint: format!("{}…", token.chars().take(4).collect::<String>()),
            rate_key: token_rate_key(token),
            credential: Credential::new(token),
        }))
    }

    async fn list_taxonomy(&self, actor: &Actor, kind: TaxonomyKind) -> BResult<Vec<TaxonomyEntry>> {
        let rows = self.all_pages(actor, &format!("/api/{}/?page_size=100000&ordering=name", kind_path(kind))).await?;
        Ok(rows.iter().filter_map(|r| entry_from(kind, r)).collect())
    }

    async fn aliases(&self, actor: &Actor, kind: TaxonomyKind) -> BResult<Vec<Alias>> {
        let all = self.store.aliases(kind).await?;
        if all.is_empty() {
            return Ok(all);
        }
        let visible: BTreeSet<i64> = self.list_taxonomy(actor, kind).await?.into_iter().map(|e| e.id).collect();
        Ok(all.into_iter().filter(|a| visible.contains(&a.target_id)).collect())
    }

    async fn aliases_within(&self, kind: TaxonomyKind, visible: &[TaxonomyEntry]) -> BResult<Vec<Alias>> {
        let all = self.store.aliases(kind).await?;
        let ids: BTreeSet<i64> = visible.iter().filter(|e| e.kind == kind).map(|e| e.id).collect();
        Ok(all.into_iter().filter(|a| ids.contains(&a.target_id)).collect())
    }

    async fn can_object(&self, actor: &Actor, kind: TaxonomyKind, id: i64, action: &str) -> BResult<bool> {
        let v = match self.get_ok(actor, &format!("/api/{}/{id}/", kind_path(kind))).await {
            Ok(v) => v,
            Err(BackendError::NotFound) | Err(BackendError::PermissionDenied(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        Ok(match action {
            "view" => true,
            // Custom fields have no object permissions (no owner) → the model permission decides.
            _ if kind == TaxonomyKind::CustomField => actor.has_kind_perm(action, kind),
            _ => v.get("user_can_change").and_then(Value::as_bool).unwrap_or(false) && actor.has_kind_perm(action, kind),
        })
    }

    async fn create_taxonomy(&self, actor: &Actor, spec: &CreateSpec) -> BResult<TaxonomyEntry> {
        let mut body = Map::new();
        body.insert("name".into(), json!(spec.name));
        match spec.kind {
            TaxonomyKind::StoragePath => {
                body.insert("path".into(), json!(spec.extra));
            }
            TaxonomyKind::CustomField => {
                body.insert("data_type".into(), json!(spec.extra));
            }
            _ => {}
        }
        let (s, v) = self
            .send(actor.credential.expose(), reqwest::Method::POST, &format!("/api/{}/", kind_path(spec.kind)), Some(&Value::Object(body)))
            .await?;
        if s != 201 {
            return Err(client_error(s, &v));
        }
        entry_from(spec.kind, &v).ok_or(BackendError::Internal)
    }

    async fn add_alias(&self, actor: &Actor, alias: &Alias) -> BResult<bool> {
        self.store.add_alias(alias.clone(), actor.user_id()).await
    }

    async fn search_documents(&self, actor: &Actor, p: &SearchParams) -> BResult<(i64, Vec<DocumentSummary>)> {
        let limit = p.limit.unwrap_or(25).clamp(1, 100);
        let page = p.offset.unwrap_or(0) / limit + 1;
        let mut q: Vec<(String, String)> = vec![
            ("page_size".into(), limit.to_string()),
            ("page".into(), page.to_string()),
            ("fields".into(), SUMMARY_FIELDS.into()),
        ];
        if let Some(s) = p.query.as_deref().filter(|s| !s.trim().is_empty()) {
            q.push(("query".into(), s.to_string()));
        } else {
            q.push(("ordering".into(), p.ordering.clone().unwrap_or_else(|| "-created".into())));
        }
        if let Some(c) = p.correspondent_id {
            q.push(("correspondent__id".into(), c.to_string()));
        }
        if let Some(c) = p.document_type_id {
            q.push(("document_type__id".into(), c.to_string()));
        }
        if let Some(c) = p.storage_path_id {
            q.push(("storage_path__id".into(), c.to_string()));
        }
        if !p.tag_ids.is_empty() {
            q.push(("tags__id__all".into(), p.tag_ids.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",")));
        }
        self.document_list(actor, &q).await
    }

    async fn get_document(&self, actor: &Actor, id: i64) -> BResult<Option<DocumentDetail>> {
        let Some(v) = self.document_raw(actor, id).await? else { return Ok(None) };
        let Some(summary) = summary_from(&v) else { return Ok(None) };
        Ok(Some(DocumentDetail {
            summary,
            original_filename: v.get("original_file_name").and_then(Value::as_str).map(str::to_string),
            owner_id: v.get("owner").and_then(Value::as_i64),
            content: v.get("content").and_then(Value::as_str).unwrap_or("").to_string(),
            custom_fields: v
                .get("custom_fields")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| Some((c.get("field")?.as_i64()?, c.get("value").cloned().unwrap_or(Value::Null)))).collect())
                .unwrap_or_default(),
            notes_count: v.get("notes").and_then(Value::as_array).map(|a| a.len() as i64).unwrap_or(0),
        }))
    }

    async fn can_change_document(&self, actor: &Actor, id: i64) -> BResult<bool> {
        Ok(self
            .document_raw(actor, id)
            .await?
            .is_some_and(|v| v.get("user_can_change").and_then(Value::as_bool).unwrap_or(false)))
    }

    async fn update_document(&self, actor: &Actor, id: i64, p: &DocumentPatch) -> BResult<(Value, Value)> {
        let cur = self.document_raw(actor, id).await?.ok_or(BackendError::NotFound)?;
        let before = doc_state(&cur);
        let mut body = Map::new();
        if let Some(t) = &p.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(c) = &p.created {
            body.insert("created".into(), json!(c));
        }
        for (k, v) in [("correspondent", p.correspondent_id), ("document_type", p.document_type_id), ("storage_path", p.storage_path_id)] {
            if let Some(v) = v {
                body.insert(k.into(), json!(v));
            }
        }
        if !p.add_tag_ids.is_empty() || !p.remove_tag_ids.is_empty() {
            let mut tags: Vec<i64> = cur.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            for t in &p.add_tag_ids {
                if !tags.contains(t) {
                    tags.push(*t);
                }
            }
            tags.retain(|t| !p.remove_tag_ids.contains(t));
            body.insert("tags".into(), json!(tags));
        }
        if !p.custom_fields.is_empty() {
            // Serializer removes fields that were not sent → send the complete list.
            let mut list: Vec<(i64, Value)> = cur
                .get("custom_fields")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| Some((c.get("field")?.as_i64()?, c.get("value").cloned().unwrap_or(Value::Null)))).collect())
                .unwrap_or_default();
            for (f, v) in &p.custom_fields {
                list.retain(|(x, _)| x != f);
                if !v.is_null() {
                    list.push((*f, v.clone()));
                }
            }
            body.insert("custom_fields".into(), Value::Array(list.into_iter().map(|(f, v)| json!({"field": f, "value": v})).collect()));
        }
        let (s, v) = self
            .send(actor.credential.expose(), reqwest::Method::PATCH, &format!("/api/documents/{id}/"), Some(&Value::Object(body)))
            .await?;
        if s != 200 {
            return Err(client_error(s, &v));
        }
        Ok((before, doc_state(&v)))
    }

    async fn audit(&self, entry: &AuditEntry) -> BResult<()> {
        self.store.audit(entry.clone()).await
    }

    async fn submit_document(&self, actor: &Actor, r: &IngestRequest) -> BResult<String> {
        let mut fields: Vec<(String, String)> = Vec::new();
        if let Some(t) = &r.title {
            fields.push(("title".into(), t.clone()));
        }
        if let Some(c) = &r.created {
            fields.push(("created".into(), c.clone()));
        }
        for (k, v) in [("correspondent", r.correspondent_id), ("document_type", r.document_type_id), ("storage_path", r.storage_path_id)] {
            if let Some(v) = v {
                fields.push((k.into(), v.to_string()));
            }
        }
        for t in &r.tag_ids {
            fields.push(("tags".into(), t.to_string()));
        }
        if !r.custom_fields.is_empty() {
            let m: Map<String, Value> = r.custom_fields.iter().map(|(f, v)| (f.to_string(), v.clone())).collect();
            fields.push(("custom_fields".into(), Value::Object(m).to_string()));
        }
        let boundary = format!("paperrust-mcp-{}", uuid::Uuid::new_v4().simple());
        let body = multipart_body(&boundary, &fields, (&r.filename, &r.mime_type, &r.bytes));
        let ct = format!("multipart/form-data; boundary={boundary}");
        let (status, raw) = self
            .transport
            .send(actor.credential.expose(), reqwest::Method::POST, "/api/documents/post_document/", Some(&ct), body)
            .await?;
        let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        if status != 200 {
            return Err(client_error(status, &v));
        }
        match &v {
            Value::String(s) => Ok(s.clone()),
            other => other.get("task_id").and_then(Value::as_str).map(str::to_string).ok_or(BackendError::Internal),
        }
    }

    async fn task_status(&self, actor: &Actor, task_id: &str) -> BResult<Option<Value>> {
        let v = self.get_ok(actor, &format!("/api/tasks/?task_id={}", percent_encode(task_id))).await?;
        Ok(list_of(&v).first().map(task_from))
    }

    async fn list_notes(&self, actor: &Actor, document_id: i64) -> BResult<Vec<NoteEntry>> {
        let v = self.get_ok(actor, &format!("/api/documents/{document_id}/notes/")).await?;
        Ok(list_of(&v).iter().filter_map(note_from).collect())
    }

    async fn add_note(&self, actor: &Actor, document_id: i64, note: &str) -> BResult<Vec<NoteEntry>> {
        let (s, v) = self
            .send(actor.credential.expose(), reqwest::Method::POST, &format!("/api/documents/{document_id}/notes/"), Some(&json!({"note": note})))
            .await?;
        if s != 200 && s != 201 {
            return Err(client_error(s, &v));
        }
        Ok(list_of(&v).iter().filter_map(note_from).collect())
    }

    async fn list_saved_views(&self, actor: &Actor) -> BResult<Vec<SavedViewInfo>> {
        let rows = self.all_pages(actor, "/api/saved_views/?page_size=100000").await?;
        Ok(rows.iter().filter_map(view_from).collect())
    }

    async fn query_documents(&self, actor: &Actor, params: &[(String, String)], limit: u32, offset: u32) -> BResult<(i64, Vec<DocumentSummary>)> {
        let limit = limit.clamp(1, 100);
        let mut q: Vec<(String, String)> =
            params.iter().filter(|(k, _)| !["page", "page_size", "fields", "format"].contains(&k.as_str())).cloned().collect();
        q.push(("page_size".into(), limit.to_string()));
        q.push(("page".into(), (offset / limit + 1).to_string()));
        q.push(("fields".into(), SUMMARY_FIELDS.into()));
        self.document_list(actor, &q).await
    }

    async fn token_scopes(&self, actor: &Actor) -> BResult<Option<Vec<String>>> {
        self.store.token_scopes(&super::scopes::token_key(actor.credential.expose()), actor.user_id()).await
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Key for the rate limit: FNV-1a over the token (no plaintext in the in-memory index).
pub fn token_rate_key(token: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("t{h:016x}")
}

impl MemoryAliasStore {
    /// All aliases of a kind (without visibility filter).
    pub fn all(&self, kind: TaxonomyKind) -> Vec<Alias> {
        self.visible_unfiltered(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualify_perms() {
        assert_eq!(qualify("add_tag"), "documents.add_tag");
        assert_eq!(qualify("view_mailrule"), "paperless_mail.view_mailrule");
        assert_eq!(qualify("change_user"), "auth.change_user");
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
        assert_ne!(token_rate_key("a"), token_rate_key("b"));
    }
}
