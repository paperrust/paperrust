//! Workflow action `APPLY_AI_SUGGESTIONS` (type 8) – upstream `documents/workflows/ai.py`
//! (`apply_ai_suggestions_to_document`, `resolve_object`, `resolve_tags`, `resolve_date`) and
//! Celery task `documents.tasks.apply_ai_suggestions(action_id, document_id)`
//! (`autoretry_for=(LLMTimeoutError,)`, `max_retries=3`, backoff 60…600 s with jitter).
//!
//! Write access goes through [`DocWriter`]. Default is [`SqlDocWriter`] (direct SQL, without
//! django-auditlog entries and without file renaming on storage-path change); the worker (W1)
//! hooks in the writer of pl-consumer (`docops::save`/`add_nested_tags`, with audit; custom fields
//! via pl-documents-write `cfi`) – pl-server `worker::ConsumerDocWriter`.

use std::sync::Arc;

use chrono::NaiveDate;
use futures::future::BoxFuture;
use pl_authz::User;
use pl_core::AppState;
use serde_json::{Map, Value};
use sqlx::Row;

use super::data::{self, Cat};
use super::index::{AiTaskError, enqueue};
use crate::compat;
use crate::llm::LlmError;
use crate::schema::{ClassificationSuggestions, TaxonomyChoice};

/// Changes to direct document fields (`None` = do not change).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocChanges {
    pub title: Option<String>,
    pub correspondent_id: Option<Option<i64>>,
    pub document_type_id: Option<Option<i64>>,
    pub storage_path_id: Option<Option<i64>>,
    pub created: Option<NaiveDate>,
}

impl DocChanges {
    pub fn is_empty(&self) -> bool {
        *self == DocChanges::default()
    }
}

/// Write access of the AI paths (workflow action, automation, undo).
pub trait DocWriter: Send + Sync {
    /// `document.save(update_fields=[…, "modified"])`.
    fn save<'a>(&'a self, state: &'a AppState, doc_id: i64, c: &'a DocChanges) -> BoxFuture<'a, Result<(), String>>;
    /// `document.add_nested_tags(tags)` (incl. ancestors).
    fn add_nested_tags<'a>(&'a self, state: &'a AppState, doc_id: i64, tag_ids: &'a [i64]) -> BoxFuture<'a, Result<(), String>>;
    /// `document.tags.remove(*ids)` (for undo only).
    fn remove_tags<'a>(&'a self, state: &'a AppState, doc_id: i64, tag_ids: &'a [i64]) -> BoxFuture<'a, Result<(), String>>;
    /// `Model.objects.get_or_create(name=name[:128], owner=owner)` → (id, created?).
    fn get_or_create<'a>(
        &'a self,
        state: &'a AppState,
        cat: Cat,
        name: &'a str,
        owner: Option<i64>,
    ) -> BoxFuture<'a, Result<(i64, bool), String>>;
    /// PaperRust: `CustomFieldInstance.objects.update_or_create(document, field, defaults={<col>: value})`.
    /// Default: direct SQL ([`sql_set_custom_field`], without django-auditlog and without file name update);
    /// the worker (pl-server `ConsumerDocWriter`) uses pl-documents-write `cfi::update_or_create` (audit, file name).
    fn set_custom_field<'a>(
        &'a self,
        state: &'a AppState,
        doc_id: i64,
        field_id: i64,
        data_type: &'a str,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(sql_set_custom_field(state, doc_id, field_id, data_type, value))
    }
    /// PaperRust: remove custom field (soft delete, for undo only).
    fn remove_custom_field<'a>(&'a self, state: &'a AppState, doc_id: i64, field_id: i64) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(sql_remove_custom_field(state, doc_id, field_id))
    }
}

/// Column per field type (upstream `CustomFieldInstance.TYPE_TO_DATA_STORE_NAME_MAP`, without document link).
pub fn custom_value_column(data_type: &str) -> Option<&'static str> {
    Some(match data_type {
        "string" => "value_text",
        "url" => "value_url",
        "date" => "value_date",
        "boolean" => "value_bool",
        "integer" => "value_int",
        "float" => "value_float",
        "monetary" => "value_monetary",
        "select" => "value_select",
        "longtext" => "value_long_text",
        _ => return None,
    })
}

enum Bound {
    Text(String),
    Bool(bool),
    Date(NaiveDate),
    Int(i32),
    Float(f64),
}

fn bound(data_type: &str, v: &Value) -> Result<Bound, String> {
    let bad = || format!("invalid value for {data_type}");
    Ok(match data_type {
        "boolean" => Bound::Bool(v.as_bool().ok_or_else(bad)?),
        "integer" => Bound::Int(v.as_i64().and_then(|i| i32::try_from(i).ok()).ok_or_else(bad)?),
        "float" => Bound::Float(v.as_f64().ok_or_else(bad)?),
        "date" => Bound::Date(v.as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).ok_or_else(bad)?),
        _ => Bound::Text(v.as_str().ok_or_else(bad)?.to_string()),
    })
}

fn bind_value<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    b: &Bound,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match b {
        Bound::Text(s) => q.bind(s.clone()),
        Bound::Bool(x) => q.bind(*x),
        Bound::Date(d) => q.bind(*d),
        Bound::Int(i) => q.bind(*i),
        Bound::Float(f) => q.bind(*f),
    }
}

/// Sets a custom field via SQL (an existing – also soft-deleted – instance is revived,
/// uniqueness `(document, field)` like upstream).
pub async fn sql_set_custom_field(state: &AppState, doc_id: i64, field_id: i64, data_type: &str, value: &Value) -> Result<(), String> {
    let col = custom_value_column(data_type).ok_or_else(|| format!("data type {data_type} is not supported"))?;
    let b = bound(data_type, value)?;
    let mut tx = state.db().begin().await.map_err(e)?;
    let existing: Option<i32> =
        sqlx::query_scalar("SELECT id FROM documents_customfieldinstance WHERE document_id = $1 AND field_id = $2 LIMIT 1")
            .bind(doc_id as i32)
            .bind(field_id as i32)
            .fetch_optional(&mut *tx)
            .await
            .map_err(e)?;
    match existing {
        Some(id) => {
            let sql = format!("UPDATE documents_customfieldinstance SET {col} = $2, deleted_at = NULL WHERE id = $1");
            bind_value(sqlx::query(&sql).bind(id), &b).execute(&mut *tx).await.map_err(e)?;
        }
        None => {
            let sql =
                format!("INSERT INTO documents_customfieldinstance (created, document_id, field_id, {col}) VALUES (now(), $1, $2, $3)");
            bind_value(sqlx::query(&sql).bind(doc_id as i32).bind(field_id as i32), &b).execute(&mut *tx).await.map_err(e)?;
        }
    }
    tx.commit().await.map_err(e)?;
    Ok(())
}

/// Removes a custom field (soft, `deleted_at`).
pub async fn sql_remove_custom_field(state: &AppState, doc_id: i64, field_id: i64) -> Result<(), String> {
    sqlx::query(
        "UPDATE documents_customfieldinstance SET deleted_at = now() WHERE document_id = $1 AND field_id = $2 AND deleted_at IS NULL",
    )
    .bind(doc_id as i32)
    .bind(field_id as i32)
    .execute(state.db())
    .await
    .map_err(e)?;
    Ok(())
}

/// Current custom field values of a document: `field_id → (data_type, value)` (live instances only).
pub async fn load_custom_values(state: &AppState, doc_id: i64) -> Result<std::collections::BTreeMap<i64, (String, Value)>, String> {
    let rows = sqlx::query(
        "SELECT i.field_id, f.data_type, i.value_text, i.value_bool, i.value_url, i.value_date, i.value_int, i.value_float, \
         i.value_monetary, i.value_select, i.value_long_text FROM documents_customfieldinstance i \
         JOIN documents_customfield f ON f.id = i.field_id WHERE i.document_id = $1 AND i.deleted_at IS NULL",
    )
    .bind(doc_id as i32)
    .fetch_all(state.db())
    .await
    .map_err(e)?;
    let mut out = std::collections::BTreeMap::new();
    for r in rows {
        let Ok(fid) = r.try_get::<i32, _>(0) else { continue };
        let dt: String = r.try_get(1).unwrap_or_default();
        let v = match dt.as_str() {
            "string" => r.try_get::<Option<String>, _>(2).ok().flatten().map(Value::from),
            "boolean" => r.try_get::<Option<bool>, _>(3).ok().flatten().map(Value::from),
            "url" => r.try_get::<Option<String>, _>(4).ok().flatten().map(Value::from),
            "date" => r.try_get::<Option<NaiveDate>, _>(5).ok().flatten().map(|d| Value::from(d.format("%Y-%m-%d").to_string())),
            "integer" => r.try_get::<Option<i32>, _>(6).ok().flatten().map(Value::from),
            "float" => r.try_get::<Option<f64>, _>(7).ok().flatten().map(Value::from),
            "monetary" => r.try_get::<Option<String>, _>(8).ok().flatten().map(Value::from),
            "select" => r.try_get::<Option<String>, _>(9).ok().flatten().map(Value::from),
            "longtext" => r.try_get::<Option<String>, _>(10).ok().flatten().map(Value::from),
            _ => None,
        };
        out.insert(i64::from(fid), (dt, v.unwrap_or(Value::Null)));
    }
    Ok(out)
}

/// Hooked-in writer (`AppState::services`), otherwise [`SqlDocWriter`].
#[derive(Clone)]
pub struct AiDocWriter(pub Arc<dyn DocWriter>);

pub fn writer(state: &AppState) -> Arc<dyn DocWriter> {
    state.inner.services.get::<AiDocWriter>().map(|w| w.0).unwrap_or_else(|| Arc::new(SqlDocWriter))
}

/// Direct SQL (default).
pub struct SqlDocWriter;

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

impl DocWriter for SqlDocWriter {
    fn save<'a>(&'a self, state: &'a AppState, doc_id: i64, c: &'a DocChanges) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            sqlx::query(
                "UPDATE documents_document SET \
                 title = CASE WHEN $2 THEN $3 ELSE title END, \
                 correspondent_id = CASE WHEN $4 THEN $5 ELSE correspondent_id END, \
                 document_type_id = CASE WHEN $6 THEN $7 ELSE document_type_id END, \
                 storage_path_id = CASE WHEN $8 THEN $9 ELSE storage_path_id END, \
                 created = CASE WHEN $10 THEN $11 ELSE created END, \
                 modified = now() WHERE id = $1",
            )
            .bind(doc_id as i32)
            .bind(c.title.is_some())
            .bind(c.title.clone().unwrap_or_default())
            .bind(c.correspondent_id.is_some())
            .bind(c.correspondent_id.flatten().map(|v| v as i32))
            .bind(c.document_type_id.is_some())
            .bind(c.document_type_id.flatten().map(|v| v as i32))
            .bind(c.storage_path_id.is_some())
            .bind(c.storage_path_id.flatten().map(|v| v as i32))
            .bind(c.created.is_some())
            .bind(c.created.unwrap_or_default())
            .execute(state.db())
            .await
            .map_err(e)?;
            Ok(())
        })
    }

    fn add_nested_tags<'a>(&'a self, state: &'a AppState, doc_id: i64, tag_ids: &'a [i64]) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            // Tag.get_hierarchy().with_ancestors(tags) – one snapshot of the parent edges (upstream #14612)
            let mut conn = state.db().acquire().await.map_err(e)?;
            let tree = pl_objects::tagtree::TagTree::load(&mut conn).await.map_err(e)?;
            drop(conn);
            let mut all: Vec<i32> =
                tree.with_ancestors(tag_ids.iter().copied().filter(|t| tree.contains(*t))).into_iter().map(|i| i as i32).collect();
            all.sort_unstable();
            sqlx::query(
                "INSERT INTO documents_document_tags (document_id, tag_id) SELECT $1, t.id FROM documents_tag t WHERE t.id = ANY($2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(doc_id as i32)
            .bind(&all)
            .execute(state.db())
            .await
            .map_err(e)?;
            Ok(())
        })
    }

    fn remove_tags<'a>(&'a self, state: &'a AppState, doc_id: i64, tag_ids: &'a [i64]) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let ids: Vec<i32> = tag_ids.iter().map(|i| *i as i32).collect();
            sqlx::query("DELETE FROM documents_document_tags WHERE document_id = $1 AND tag_id = ANY($2)")
                .bind(doc_id as i32)
                .bind(&ids)
                .execute(state.db())
                .await
                .map_err(e)?;
            Ok(())
        })
    }

    fn get_or_create<'a>(
        &'a self,
        state: &'a AppState,
        cat: Cat,
        name: &'a str,
        owner: Option<i64>,
    ) -> BoxFuture<'a, Result<(i64, bool), String>> {
        Box::pin(async move {
            let table = cat.model().table;
            let name: String = name.chars().take(128).collect();
            let owner32 = owner.map(|o| o as i32);
            let mut tx = state.db().begin().await.map_err(e)?;
            let found: Option<i32> =
                sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE name = $1 AND owner_id IS NOT DISTINCT FROM $2 LIMIT 1"))
                    .bind(&name)
                    .bind(owner32)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(e)?;
            if let Some(id) = found {
                return Ok((i64::from(id), false));
            }
            // MatchingModel default values: match "", matching_algorithm MATCH_ANY (1), is_insensitive True
            let id: i32 = match cat {
                Cat::Tags => sqlx::query_scalar(
                    "INSERT INTO documents_tag (name, match, matching_algorithm, is_insensitive, color, is_inbox_tag, owner_id, tn_parent_id) \
                     VALUES ($1, '', 1, true, '#a6cee3', false, $2, NULL) RETURNING id",
                )
                .bind(&name)
                .bind(owner32)
                .fetch_one(&mut *tx)
                .await
                .map_err(e)?,
                Cat::Correspondents | Cat::DocumentTypes => sqlx::query_scalar(&format!(
                    "INSERT INTO {table} (name, match, matching_algorithm, is_insensitive, owner_id) VALUES ($1, '', 1, true, $2) RETURNING id"
                ))
                .bind(&name)
                .bind(owner32)
                .fetch_one(&mut *tx)
                .await
                .map_err(e)?,
                Cat::StoragePaths => return Err("storage paths are never created from AI suggestions".into()),
            };
            tx.commit().await.map_err(e)?;
            Ok((i64::from(id), true))
        })
    }
}

/// `resolve_date(dates)`: first date in the format `%Y-%m-%d`.
pub fn resolve_date(dates: &[String]) -> Option<NaiveDate> {
    dates.iter().find_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// `resolve_X_ids(existing, user) + match_X_by_name(names, user)` (without deduplication).
pub async fn matched(state: &AppState, user: Option<&User>, cat: Cat, c: &TaxonomyChoice) -> Result<Vec<(i64, String)>, String> {
    let db = state.db();
    let mut out = data::visible_objects(db, user, cat, &c.existing_ids).await.map_err(|x| x.status().to_string())?;
    if !c.new_names.is_empty() {
        let pool = data::permitted_pool(db, user, cat).await.map_err(|x| x.status().to_string())?;
        let refs: Vec<Option<&str>> = c.new_names.iter().map(|n| Some(n.as_str())).collect();
        out.extend(compat::match_names_to_pool(&refs, &pool));
    }
    Ok(out)
}

/// Fields of the action.
#[derive(Debug, Clone, Default)]
pub struct ActionRow {
    pub id: i64,
    pub fields: Vec<String>,
    pub create_missing: bool,
    pub overwrite: bool,
}

async fn load_action(state: &AppState, id: i64) -> Result<Option<ActionRow>, String> {
    let r = sqlx::query(
        "SELECT id, ai_suggestion_fields, ai_create_missing, ai_overwrite_existing FROM documents_workflowaction WHERE id = $1",
    )
    .bind(id as i32)
    .fetch_optional(state.db())
    .await
    .map_err(e)?;
    Ok(r.map(|r| ActionRow {
        id,
        fields: r
            .try_get::<Option<Value>, _>("ai_suggestion_fields")
            .ok()
            .flatten()
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
            .unwrap_or_default(),
        create_missing: r.try_get("ai_create_missing").unwrap_or(false),
        overwrite: r.try_get("ai_overwrite_existing").unwrap_or(false),
    }))
}

/// Current state of the direct fields.
#[derive(Debug, Clone)]
pub struct DocState {
    pub title: String,
    pub content: String,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    pub created: NaiveDate,
    pub owner_id: Option<i64>,
    pub tag_ids: Vec<i64>,
}

pub async fn load_state(state: &AppState, id: i64) -> Result<Option<DocState>, String> {
    let r = sqlx::query(
        "SELECT title, content, correspondent_id, document_type_id, storage_path_id, created, owner_id, \
         COALESCE((SELECT array_agg(tag_id ORDER BY tag_id) FROM documents_document_tags WHERE document_id = d.id), '{}') AS tags \
         FROM documents_document d WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id as i32)
    .fetch_optional(state.db())
    .await
    .map_err(e)?;
    Ok(r.map(|r| {
        let g = |c: &str| r.try_get::<Option<i32>, _>(c).ok().flatten().map(i64::from);
        DocState {
            title: r.try_get("title").unwrap_or_default(),
            content: r.try_get("content").unwrap_or_default(),
            correspondent_id: g("correspondent_id"),
            document_type_id: g("document_type_id"),
            storage_path_id: g("storage_path_id"),
            created: r.try_get("created").unwrap_or_default(),
            owner_id: g("owner_id"),
            tag_ids: r.try_get::<Vec<i32>, _>("tags").unwrap_or_default().into_iter().map(i64::from).collect(),
        }
    }))
}

/// `clear_document_caches(doc)` + `index_document.delay(doc)`.
pub async fn after_change(state: &AppState, doc_id: i64) {
    super::cache::invalidate(state, doc_id).await;
    let keys: Vec<String> = ["suggest", "metadata", "thumbnail_modified"].iter().map(|k| format!("doc_{doc_id}_{k}")).collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    if let Err(e) = state.cache().delete_many(&refs).await {
        tracing::warn!(target: "pl_ai", error = %e.status(), "clearing document caches failed");
    }
    let mut kw = Map::new();
    kw.insert("document_id".into(), Value::from(doc_id));
    if let Err(x) = enqueue(state, "documents.tasks.index_document", kw, "system").await {
        tracing::warn!(target: "pl_ai", error = %x, "could not queue index_document");
    }
}

/// Celery `documents.tasks.apply_ai_suggestions(action_id, document_id)`.
pub async fn apply_ai_suggestions(state: &AppState, action_id: i64, document_id: i64) -> Result<Value, AiTaskError> {
    let Some(action) = load_action(state, action_id).await.map_err(|x| AiTaskError::new("DatabaseError", x))? else {
        tracing::warn!(target: "pl_ai", action_id, document_id, "Workflow action or document no longer exists, not applying AI suggestions");
        return Ok(Value::Null);
    };
    let Some(doc) = data::load_doc(state.db(), document_id).await.map_err(|x| AiTaskError::new("DatabaseError", x.status().to_string()))?
    else {
        tracing::warn!(target: "pl_ai", action_id, document_id, "Workflow action or document no longer exists, not applying AI suggestions");
        return Ok(Value::Null);
    };
    let changed = apply_ai_suggestions_to_document(state, &action, &doc).await?;
    if changed.is_empty() {
        return Ok(Value::Null);
    }
    after_change(state, doc.id).await;
    // if ai_config.llm_index_enabled: update_document_in_llm_index.apply_async(kwargs={"document": document})
    // (Rust: document id instead of a Python object; executable only via the PaperRust queue).
    if super::effective_config(state).await.is_ok_and(|c| c.llm_index_enabled()) {
        let mut kw = Map::new();
        kw.insert("document_id".into(), Value::from(doc.id));
        if let Err(x) = enqueue(state, "documents.tasks.update_document_in_llm_index", kw, "system").await {
            tracing::warn!(target: "pl_ai", error = %x, "could not queue update_document_in_llm_index");
        }
    }
    Ok(Value::Null)
}

/// `apply_ai_suggestions_to_document(action, document)` → changed fields.
pub async fn apply_ai_suggestions_to_document(state: &AppState, action: &ActionRow, doc: &data::AiDoc) -> Result<Vec<String>, AiTaskError> {
    let selected: std::collections::HashSet<&str> = action.fields.iter().map(String::as_str).collect();
    if selected.is_empty() {
        tracing::warn!(target: "pl_ai", action = action.id, "Workflow action has no AI suggestion fields selected, skipping");
        return Ok(Vec::new());
    }
    let cfg = super::effective_config(state).await.map_err(|x| AiTaskError::new("ValueError", x.status().to_string()))?;
    if !cfg.ai_enabled {
        tracing::error!(target: "pl_ai", document = doc.id, "AI is not enabled, cannot apply AI suggestions");
        return Ok(Vec::new());
    }
    let Some(cur) = load_state(state, doc.id).await.map_err(|x| AiTaskError::new("DatabaseError", x))? else { return Ok(Vec::new()) };
    if crate::tokenizer::py_strip(&cur.content).is_empty() {
        tracing::warn!(target: "pl_ai", document = doc.id, action = action.id, "Document has no content, skipping AI suggestions");
        return Ok(Vec::new());
    }
    let owner: Option<User> = match cur.owner_id {
        Some(o) => pl_authz::user::load_user_by_id(state.db(), o).await.map_err(|x| AiTaskError::new("DatabaseError", x.to_string()))?,
        None => None,
    };
    let ui = match &owner {
        Some(u) => data::ui_settings(state.db(), u.id).await.ok().flatten(),
        None => None,
    };
    let lang = crate::classifier::get_llm_output_language(cfg.llm_output_language.as_deref(), ui.as_ref());
    let s: ClassificationSuggestions = match super::suggest::classify(state, &cfg, doc, owner.as_ref(), lang.as_deref()).await {
        Ok(s) => s,
        Err(LlmError::InvalidConfig(m)) => {
            tracing::error!(target: "pl_ai", document = doc.id, error = %m, "Invalid AI configuration, cannot get suggestions");
            return Ok(Vec::new());
        }
        Err(x) => return Err(x.into()),
    };
    let w = writer(state);
    let owner_ref = owner.as_ref();
    let owner_id = owner.as_ref().map(|u| u.id);
    let should = |f: &str, empty: bool| selected.contains(f) && (action.overwrite || empty);
    let mut ch = DocChanges::default();
    let mut updated: Vec<String> = Vec::new();
    if should("title", cur.title.is_empty()) {
        let t = crate::tokenizer::py_strip(&s.title);
        if !t.is_empty() {
            ch.title = Some(t.chars().take(128).collect());
            updated.push("title".into());
        }
    }
    let ws = |x: String| AiTaskError::new("DatabaseError", x);
    for (field, cat, choice, empty) in [
        ("correspondent", Cat::Correspondents, &s.correspondents, cur.correspondent_id.is_none()),
        ("document_type", Cat::DocumentTypes, &s.document_types, cur.document_type_id.is_none()),
        ("storage_path", Cat::StoragePaths, &s.storage_paths, cur.storage_path_id.is_none()),
    ] {
        if !should(field, empty) {
            continue;
        }
        let m = matched(state, owner_ref, cat, choice).await.map_err(ws)?;
        let picked = match m.first() {
            Some((id, _)) => Some(*id),
            None if action.create_missing && cat != Cat::StoragePaths => {
                let names: Vec<&str> = choice.new_names.iter().map(String::as_str).collect();
                let unmatched = compat::extract_unmatched_names::<(i64, String)>(&names, &[]);
                match unmatched.first() {
                    Some(n) => Some(w.get_or_create(state, cat, n, owner_id).await.map_err(ws)?.0),
                    None => None,
                }
            }
            None => None,
        };
        if let Some(id) = picked {
            match cat {
                Cat::Correspondents => ch.correspondent_id = Some(Some(id)),
                Cat::DocumentTypes => ch.document_type_id = Some(Some(id)),
                _ => ch.storage_path_id = Some(Some(id)),
            }
            updated.push(field.into());
        }
    }
    if should("created", false)
        && let Some(d) = resolve_date(&s.dates)
    {
        ch.created = Some(d);
        updated.push("created".into());
    }
    let mut tags: Vec<i64> = Vec::new();
    if selected.contains("tags") {
        let m = matched(state, owner_ref, Cat::Tags, &s.tags).await.map_err(ws)?;
        tags = m.iter().map(|(i, _)| *i).collect();
        if action.create_missing {
            let names: Vec<&str> = s.tags.new_names.iter().map(String::as_str).collect();
            for n in compat::extract_unmatched_names(&names, &m) {
                tags.push(w.get_or_create(state, Cat::Tags, &n, owner_id).await.map_err(ws)?.0);
            }
        }
        if !tags.is_empty() {
            updated.push("tags".into());
        }
    }
    if !updated.is_empty() {
        // save(update_fields=[*direct, "modified"]) – even if only tags are added
        w.save(state, doc.id, &ch).await.map_err(ws)?;
    }
    if !tags.is_empty() {
        w.add_nested_tags(state, doc.id, &tags).await.map_err(ws)?;
    }
    tracing::info!(target: "pl_ai", document = doc.id, fields = ?updated, "Applied AI suggestions");
    Ok(updated)
}
