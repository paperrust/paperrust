//! LLM index tasks and similarity retrieval – upstream `paperless_ai/indexing.py` (v3.4.2):
//! `update_llm_index(rebuild, document_ids)`, `llm_index_add_or_update_document`,
//! `llm_index_remove_document`, `llm_index_compact`, `retrieve_similar_nodes`,
//! `queue_llm_index_update_if_needed`, `llm_index_exists`.
//!
//! For the worker (W1) the entry points are [`llmindex_index`],
//! [`update_document_in_llm_index`], [`remove_document_from_llm_index`] (Celery names
//! `documents.tasks.llmindex_index` / `update_document_in_llm_index` / `remove_document_from_llm_index`;
//! the last two receive the **document id** in Rust instead of a Python object).

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use pl_core::AppState;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::config::AiConfig;
use crate::embedding::{build_llm_index_text, Embedder};
use crate::llm::LlmError;
use crate::locks::{self, RwGuard, WriteLock, COMPACTION_LOCK_TIMEOUT};
use crate::node::build_document_nodes;
use crate::tokenizer::truncate_embedding_query;
use crate::vector_store::{DocFilter, Hit, Row as VRow, VecStore};

/// `_INDEX_STREAM_CHUNK_SIZE`.
const STREAM_CHUNK: i64 = 1000;

/// Error of an index task (`type(exc).__name__`, `str(exc)`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{error_type}: {message}")]
pub struct AiTaskError {
    pub error_type: String,
    pub message: String,
    /// Falls under `autoretry_for` (only `LLMTimeoutError` for `apply_ai_suggestions`).
    pub autoretry: bool,
}

impl AiTaskError {
    pub fn new(t: &str, m: impl Into<String>) -> Self {
        Self { error_type: t.into(), message: m.into(), autoretry: false }
    }
}

impl From<LlmError> for AiTaskError {
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::Timeout => Self { error_type: "LLMTimeoutError".into(), message: String::new(), autoretry: true },
            LlmError::Provider(_) => Self::new("LLMProviderError", ""),
            LlmError::InvalidConfig(m) => Self::new("ValueError", m),
            // tests/chaos C4: retry connection errors like timeouts (AI service briefly
            // gone, e.g. Ollama restart) – upstream only for LLMTimeoutError.
            LlmError::Transport(m) => Self { error_type: "ConnectError".into(), message: m, autoretry: true },
        }
    }
}

fn store_err(e: impl std::fmt::Display) -> AiTaskError {
    AiTaskError::new("OperationalError", e.to_string())
}

/// `settings.LLM_INDEX_DIR`.
pub fn index_dir(state: &AppState) -> PathBuf {
    locks::index_dir(&state.config().data_dir)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, AiTaskError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AiTaskError::new("RuntimeError", e.to_string()))
}

async fn write_lock(dir: PathBuf) -> Result<WriteLock, AiTaskError> {
    blocking(move || WriteLock::acquire(&dir)).await?.map_err(store_err)
}

async fn read_lock(dir: PathBuf) -> Result<RwGuard, AiTaskError> {
    blocking(move || RwGuard::read(&dir)).await?.map_err(store_err)
}

/// `llm_index_exists()`.
pub async fn llm_index_exists(state: &AppState) -> Result<bool, AiTaskError> {
    let dir = index_dir(state);
    let _g = read_lock(dir.clone()).await?;
    let st = VecStore::open(&dir, None).map_err(store_err)?;
    st.table_exists().map_err(store_err)
}

/// A document with everything `build_document_node` needs.
#[derive(Debug, Clone)]
pub struct IndexDoc {
    pub id: i64,
    pub modified_iso: String,
    pub text: String,
    pub metadata: Map<String, Value>,
}

fn py_float(f: f64) -> String {
    pl_core::json::python_float_repr(f)
}

/// `str(CustomFieldInstance)` = `"<name> : <value_for_search>"`.
fn cf_instance_str(name: &str, data_type: &str, r: &sqlx::postgres::PgRow, extra: &Option<Value>) -> String {
    let none = "None".to_string();
    let v: String = match data_type {
        "string" => r.try_get::<Option<String>, _>("value_text").ok().flatten().unwrap_or(none),
        "url" => r.try_get::<Option<String>, _>("value_url").ok().flatten().unwrap_or(none),
        "longtext" => r.try_get::<Option<String>, _>("value_long_text").ok().flatten().unwrap_or(none),
        "monetary" => r.try_get::<Option<String>, _>("value_monetary").ok().flatten().unwrap_or(none),
        "date" => r.try_get::<Option<NaiveDate>, _>("value_date").ok().flatten().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or(none),
        "boolean" => r.try_get::<Option<bool>, _>("value_bool").ok().flatten().map(|b| if b { "True" } else { "False" }.to_string()).unwrap_or(none),
        "integer" => r.try_get::<Option<i32>, _>("value_int").ok().flatten().map(|i| i.to_string()).unwrap_or(none),
        "float" => r.try_get::<Option<f64>, _>("value_float").ok().flatten().map(py_float).unwrap_or(none),
        "documentlink" => r
            .try_get::<Option<Value>, _>("value_document_ids")
            .ok()
            .flatten()
            .map(|v| crate::node::py_repr(&v))
            .unwrap_or(none),
        "select" => {
            let sel: Option<String> = r.try_get("value_select").ok().flatten();
            match sel {
                None => none,
                Some(id) => extra
                    .as_ref()
                    .and_then(|e| e.get("select_options"))
                    .and_then(Value::as_array)
                    .and_then(|opts| opts.iter().find(|o| o.get("id").and_then(Value::as_str) == Some(id.as_str())))
                    .and_then(|o| o.get("label"))
                    .map(crate::node::py_str)
                    .unwrap_or(none),
            }
        }
        _ => none,
    };
    format!("{name} : {v}")
}

/// Root documents (`Document.objects.filter(root_document__isnull=True)`), optionally only `ids`
/// (already root ids), in blocks from `after_id`.
pub async fn load_index_docs(state: &AppState, ids: Option<&[i64]>, after_id: i64, limit: i64) -> Result<Vec<IndexDoc>, AiTaskError> {
    let db = state.db();
    let ids32: Option<Vec<i32>> = ids.map(|v| v.iter().map(|i| *i as i32).collect());
    let sql = format!(
        "SELECT d.id, d.title, d.filename, d.archive_serial_number, d.created, d.added, d.modified, \
         c.name AS correspondent, dt.name AS document_type, sp.name AS storage_path, ({}) AS eff, \
         COALESCE((SELECT array_agg(t.name ORDER BY t.name, t.id) FROM documents_document_tags x JOIN documents_tag t ON t.id = x.tag_id WHERE x.document_id = d.id), '{{}}') AS tags \
         FROM documents_document d \
         LEFT JOIN documents_correspondent c ON c.id = d.correspondent_id \
         LEFT JOIN documents_documenttype dt ON dt.id = d.document_type_id \
         LEFT JOIN documents_storagepath sp ON sp.id = d.storage_path_id \
         WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL AND d.id > $1 AND ($2::int[] IS NULL OR d.id = ANY($2)) \
         ORDER BY d.id LIMIT $3",
        crate::server::data::EFFECTIVE_CONTENT
    );
    let rows = sqlx::query(&sql).bind(after_id as i32).bind(ids32).bind(limit).fetch_all(db).await.map_err(store_err)?;
    let doc_ids: Vec<i32> = rows.iter().filter_map(|r| r.try_get::<i32, _>("id").ok()).collect();
    // notes (Note.objects, ordering created)
    let notes = sqlx::query("SELECT document_id, note FROM documents_note WHERE document_id = ANY($1) AND deleted_at IS NULL ORDER BY created, id")
        .bind(&doc_ids)
        .fetch_all(db)
        .await
        .map_err(store_err)?;
    let cfs = sqlx::query(
        "SELECT i.document_id, f.name, f.data_type, f.extra_data, i.value_text, i.value_url, i.value_long_text, i.value_monetary, \
         i.value_date, i.value_bool, i.value_int, i.value_float, i.value_document_ids, i.value_select \
         FROM documents_customfieldinstance i JOIN documents_customfield f ON f.id = i.field_id \
         WHERE i.document_id = ANY($1) AND i.deleted_at IS NULL ORDER BY i.created, i.id",
    )
    .bind(&doc_ids)
    .fetch_all(db)
    .await
    .map_err(store_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i32 = r.try_get("id").map_err(store_err)?;
        let n: Vec<String> = notes
            .iter()
            .filter(|x| x.try_get::<i32, _>("document_id").ok() == Some(id))
            .filter_map(|x| x.try_get::<String, _>("note").ok())
            .collect();
        let c: Vec<(String, String)> = cfs
            .iter()
            .filter(|x| x.try_get::<i32, _>("document_id").ok() == Some(id))
            .map(|x| {
                let name: String = x.try_get("name").unwrap_or_default();
                let dt: String = x.try_get("data_type").unwrap_or_default();
                let extra: Option<Value> = x.try_get("extra_data").ok().flatten();
                (name.clone(), cf_instance_str(&name, &dt, x, &extra))
            })
            .collect();
        let content: String = r.try_get::<Option<String>, _>("eff").ok().flatten().unwrap_or_default();
        let text = build_llm_index_text(&n, &c, &content);
        let created: Option<NaiveDate> = r.try_get("created").ok();
        let added: Option<DateTime<Utc>> = r.try_get("added").ok();
        let modified: DateTime<Utc> = r.try_get("modified").map_err(store_err)?;
        let modified_iso = pl_core::datetime::python_isoformat(&modified);
        let mut m = Map::new();
        m.insert("document_id".into(), json!(id.to_string()));
        m.insert("title".into(), json!(r.try_get::<String, _>("title").unwrap_or_default()));
        m.insert("tags".into(), json!(r.try_get::<Vec<String>, _>("tags").unwrap_or_default()));
        m.insert("correspondent".into(), json!(r.try_get::<Option<String>, _>("correspondent").ok().flatten()));
        m.insert("document_type".into(), json!(r.try_get::<Option<String>, _>("document_type").ok().flatten()));
        m.insert("filename".into(), json!(r.try_get::<Option<String>, _>("filename").ok().flatten()));
        m.insert("storage_path".into(), json!(r.try_get::<Option<String>, _>("storage_path").ok().flatten()));
        m.insert("archive_serial_number".into(), json!(r.try_get::<Option<i32>, _>("archive_serial_number").ok().flatten()));
        m.insert("created".into(), json!(created.map(|d| d.format("%Y-%m-%d").to_string())));
        m.insert("added".into(), json!(added.map(|d| pl_core::datetime::python_isoformat(&d))));
        m.insert("modified".into(), json!(modified_iso));
        out.push(IndexDoc { id: i64::from(id), modified_iso, text, metadata: m });
    }
    Ok(out)
}

/// `build_document_node` + `_embed_nodes`.
pub async fn embed_document(cfg: &AiConfig, emb: &Embedder, d: &IndexDoc) -> Result<Vec<VRow>, AiTaskError> {
    let nodes = build_document_nodes(&d.id.to_string(), &d.text, &d.metadata, cfg.llm_embedding_chunk_size as usize, |_| {
        uuid::Uuid::new_v4().to_string()
    })
    .map_err(|e| AiTaskError::new("ValueError", e.0))?;
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    let texts: Vec<String> = nodes.iter().map(|n| n.embed_content()).collect();
    let embs = emb.embed_texts(&texts).await?;
    Ok(nodes.into_iter().zip(embs).map(|(node, embedding)| VRow { node, embedding }).collect())
}

/// Compaction under exclusive access; skip if there are active readers.
fn compact_excl(dir: &std::path::Path, st: &mut VecStore, force: bool) {
    match RwGuard::exclusive(dir, COMPACTION_LOCK_TIMEOUT) {
        Ok(_g) => {
            if let Err(e) = st.compact(force) {
                tracing::warn!(target: "pl_ai", error = %e, "LLM index compaction failed");
            }
        }
        Err(locks::RwError::Timeout) => {
            tracing::info!(target: "pl_ai", "Skipping LLM index compaction: index readers are active; will retry next run.")
        }
        Err(e) => tracing::warn!(target: "pl_ai", error = %e, "LLM index compaction lock failed"),
    }
}

/// upstream `update_llm_index(rebuild=, document_ids=)` → message text.
pub async fn update_llm_index(state: &AppState, cfg: &AiConfig, mut rebuild: bool, document_ids: Option<Vec<i64>>) -> Result<String, AiTaskError> {
    let dir = index_dir(state);
    {
        let _w = write_lock(dir.clone()).await?;
        let st = VecStore::open(&dir, None).map_err(store_err)?;
        if st.has_pending_migration().map_err(store_err)? {
            // upstream runs the structural migration v1→v2; PaperRust rebuilds (contract).
            tracing::warn!(target: "pl_ai", "LLM index has an older schema; forcing rebuild.");
            rebuild = true;
        }
    }
    let any_docs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL)")
        .fetch_one(state.db())
        .await
        .map_err(store_err)?;
    if !any_docs && !rebuild && !llm_index_exists(state).await? {
        tracing::warn!(target: "pl_ai", "No documents found to index.");
        return Ok("No documents found to index.".into());
    }
    let model_name = cfg.configured_embedding_model_name();
    if !rebuild {
        let _r = read_lock(dir.clone()).await?;
        let st = VecStore::open(&dir, None).map_err(store_err)?;
        if st.table_exists().map_err(store_err)? && st.config_mismatch(&model_name).map_err(store_err)? {
            tracing::warn!(target: "pl_ai", "Embedding model changed; forcing LLM index rebuild.");
            rebuild = true;
        }
    }
    if !any_docs {
        tracing::warn!(target: "pl_ai", "No documents found to index.");
    }
    let emb = Embedder::from_config(cfg)?;
    let _w = write_lock(dir.clone()).await?;
    let mut st = VecStore::open(&dir, Some(&model_name)).map_err(store_err)?;
    let msg: String = if rebuild || !st.table_exists().map_err(store_err)? {
        tracing::info!(target: "pl_ai", "Rebuilding LLM index.");
        st.drop_table().map_err(store_err)?;
        let mut after = 0i64;
        loop {
            let docs = load_index_docs(state, None, after, STREAM_CHUNK).await?;
            let Some(last) = docs.last() else { break };
            after = last.id;
            for d in &docs {
                let rows = embed_document(cfg, &emb, d).await?;
                st.add(&rows).map_err(store_err)?;
            }
        }
        "LLM index rebuilt successfully.".to_string()
    } else {
        let scoped: Option<Vec<i64>> = match document_ids {
            None => None,
            Some(ids) => {
                let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
                let r: Vec<i32> = sqlx::query_scalar(
                    "SELECT DISTINCT COALESCE(root_document_id, id) FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL",
                )
                .bind(&ids32)
                .fetch_all(state.db())
                .await
                .map_err(store_err)?;
                Some(r.into_iter().map(i64::from).collect())
            }
        };
        let existing = st.get_modified_times().map_err(store_err)?;
        let mut changed = 0;
        let mut after = 0i64;
        loop {
            let docs = load_index_docs(state, scoped.as_deref(), after, STREAM_CHUNK).await?;
            let Some(last) = docs.last() else { break };
            after = last.id;
            for d in &docs {
                if existing.get(&d.id.to_string()) == Some(&d.modified_iso) {
                    continue;
                }
                let rows = embed_document(cfg, &emb, d).await?;
                st.upsert_document(d.id, &rows).map_err(store_err)?;
                changed += 1;
            }
        }
        if changed > 0 { "LLM index updated successfully.".into() } else { "No changes detected in LLM index.".into() }
    };
    let d2 = dir.clone();
    st = blocking(move || {
        compact_excl(&d2, &mut st, false);
        st
    })
    .await?;
    drop(st);
    Ok(msg)
}

/// Celery `documents.tasks.llmindex_index(rebuild=False)`: only if the index is enabled.
pub async fn llmindex_index(state: &AppState, rebuild: bool) -> Result<Value, AiTaskError> {
    let cfg = crate::server::effective_config(state).await.map_err(|e| AiTaskError::new("ValueError", e.status().to_string()))?;
    if !cfg.llm_index_enabled() {
        tracing::info!(target: "pl_ai", "LLM index is disabled, skipping update.");
        return Ok(Value::Null);
    }
    Ok(Value::String(update_llm_index(state, &cfg, rebuild, None).await?))
}

/// `llm_index_add_or_update_document(document)` (versions via their root document).
pub async fn update_document_in_llm_index(state: &AppState, document_id: i64) -> Result<(), AiTaskError> {
    let cfg = crate::server::effective_config(state).await.map_err(|e| AiTaskError::new("ValueError", e.status().to_string()))?;
    if !cfg.llm_index_enabled() {
        return Ok(());
    }
    let root = crate::server::data::root_id(state.db(), document_id).await.map_err(|e| AiTaskError::new("DatabaseError", e.status().to_string()))?;
    let docs = load_index_docs(state, Some(&[root]), 0, 1).await?;
    let Some(d) = docs.first() else { return Ok(()) };
    let emb = Embedder::from_config(&cfg)?;
    let rows = embed_document(&cfg, &emb, d).await?;
    let dir = index_dir(state);
    let _w = write_lock(dir.clone()).await?;
    let st = VecStore::open(&dir, Some(&cfg.configured_embedding_model_name())).map_err(store_err)?;
    if st.has_pending_migration().map_err(store_err)? {
        tracing::warn!(target: "pl_ai", document = d.id, "Skipping incremental LLM index update: the index requires a rebuild first.");
        return Ok(());
    }
    st.upsert_document(d.id, &rows).map_err(store_err)
}

/// `llm_index_remove_document(document)`.
pub async fn remove_document_from_llm_index(state: &AppState, document_id: i64) -> Result<(), AiTaskError> {
    let cfg = crate::server::effective_config(state).await.map_err(|e| AiTaskError::new("ValueError", e.status().to_string()))?;
    if !cfg.llm_index_enabled() {
        return Ok(());
    }
    let dir = index_dir(state);
    let _w = write_lock(dir.clone()).await?;
    let st = VecStore::open(&dir, None).map_err(store_err)?;
    if st.has_pending_migration().map_err(store_err)? {
        return Ok(());
    }
    st.delete(document_id).map_err(store_err)
}

/// `llm_index_compact()`.
pub async fn llm_index_compact(state: &AppState) -> Result<(), AiTaskError> {
    let dir = index_dir(state);
    let _w = write_lock(dir.clone()).await?;
    let mut st = VecStore::open(&dir, None).map_err(store_err)?;
    blocking(move || compact_excl(&dir, &mut st, true)).await
}

/// Enqueueing an index task (pl-server hooks the PaperRust queue in via
/// `AppState::services`; otherwise the shared dispatch `pl_core::tasks::send_task`).
pub trait AiTaskSink: Send + Sync {
    fn enqueue(&self, name: &str, kwargs: Map<String, Value>, trigger_source: &str) -> futures::future::BoxFuture<'_, Result<(), String>>;
}

/// Hooked-in sink (`state.inner.services`).
#[derive(Clone)]
pub struct AiTasks(pub Arc<dyn AiTaskSink>);

/// Enqueue a task.
pub async fn enqueue(state: &AppState, name: &str, kwargs: Map<String, Value>, trigger_source: &str) -> Result<(), String> {
    if let Some(AiTasks(sink)) = state.inner.services.get::<AiTasks>() {
        return sink.enqueue(name, kwargs, trigger_source).await;
    }
    let mut req = pl_core::tasks::TaskRequest::new(name).trigger_source(trigger_source);
    for (k, v) in kwargs {
        req = req.kwarg(&k, v);
    }
    pl_core::tasks::send_task(state, req).await.map(|_| ()).map_err(|e| e.status().to_string())
}

/// `queue_llm_index_update_if_needed(rebuild=, reason=)`.
pub async fn queue_llm_index_update_if_needed(state: &AppState, rebuild: bool, reason: &str) -> bool {
    let busy: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM documents_paperlesstask WHERE task_type = 'llm_index' AND status IN ('PENDING','STARTED')) \
         OR EXISTS(SELECT 1 FROM documents_paperlesstask WHERE task_type = 'llm_index' AND date_created >= now() - interval '5 minutes')",
    )
    .fetch_one(state.db())
    .await;
    match busy {
        Ok(true) => return false,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(target: "pl_ai", error = %e, "could not check LLM index tasks");
            return false;
        }
    }
    let mut kw = Map::new();
    kw.insert("rebuild".into(), Value::Bool(rebuild));
    match enqueue(state, "documents.tasks.llmindex_index", kw, "system").await {
        Ok(()) => {
            tracing::warn!(target: "pl_ai", "Queued LLM index update{}: {reason}", if rebuild { " (rebuild)" } else { "" });
            true
        }
        Err(e) => {
            tracing::warn!(target: "pl_ai", error = %e, "could not queue LLM index update");
            false
        }
    }
}

/// `retrieve_similar_nodes(document, top_k, document_ids)` – similarity without the document itself.
pub async fn retrieve_similar_nodes(
    state: &AppState,
    cfg: &AiConfig,
    doc_id: i64,
    title: &str,
    content: &str,
    top_k: usize,
    allowed: Option<&[i64]>,
) -> Result<Vec<Hit>, AiTaskError> {
    if allowed.is_some_and(|a| a.is_empty()) {
        return Ok(Vec::new());
    }
    if !llm_index_exists(state).await? {
        queue_llm_index_update_if_needed(state, false, "LLM index not found for similarity query.").await;
        return Ok(Vec::new());
    }
    let mut filters = Vec::new();
    if let Some(a) = allowed {
        filters.push(DocFilter::In(a.to_vec()));
    }
    filters.push(DocFilter::Ne(doc_id));
    let query = truncate_embedding_query(&format!("{title}\n{content}"), cfg.llm_embedding_chunk_size as usize);
    let emb = Embedder::from_config(cfg)?;
    let dir = index_dir(state);
    let _r = read_lock(dir.clone()).await?;
    let st = VecStore::open(&dir, None).map_err(store_err)?;
    let qv = emb.embed_query(&query).await?;
    let hits = st.query(&qv, top_k, &filters).map_err(store_err)?;
    let Some(a) = allowed else { return Ok(hits) };
    Ok(hits.into_iter().filter(|h| h.node.document_id().is_some_and(|d| a.contains(&d))).collect())
}
