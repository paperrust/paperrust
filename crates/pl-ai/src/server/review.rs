//! Automatic processing (PaperRust extension, contract 5.3) and review list
//! `/api/x-ai/review/`.
//!
//! Flow per document ([`process_document`]): upstream classification (`get_ai_document_classification`,
//! like the workflow action with the permissions of the document owner or the triggering user),
//! then [`crate::auto::plan`] (matching with duplicate lock, confidence, limits), then:
//! * mode `suggest`: review-list entry only (`paperrust_ai_review`, status `pending`);
//! * mode `apply`: apply changes from the confidence threshold immediately (audit before/after in
//!   `paperrust_ai_audit`), the rest stays for review (`partial`).
//!
//! New objects: only with `PAPERRUST_AI_AUTO_CREATE_MISSING=yes`, at most
//! `…_MAX_NEW_OBJECTS` per run, never storage paths, and **immediately before creating** again
//! [`crate::taxonomy::check_duplicate`] (server-side lock).
//! Undo: only if the current state of the affected fields still equals the state after
//! applying – otherwise 409 with the differing fields. Every partial acceptance of a review entry
//! keeps its audit id (`audit_ids`); undo reverts all of them, newest first.
//!
//! Atomicity (external review, findings 5/6/8): document fields, tags, custom fields, the AI audit
//! entry and the review entry are written in **one** transaction when the hooked-in writer supports
//! it ([`workflow::DocWriter::supports_transaction`]); an audit failure is an error and rolls
//! everything back. With a writer that does not (yet) support it, a failure after the first write
//! restores the previous values (best effort, [`Ops`]). Audit failures are logged with ids and field
//! names only, never with values (finding 7).
//!
//! Security wave 3: manual run respects `PAPERRUST_AI_AUTO_MODE` and is limited per run and per
//! user/hour (M1); new objects only with the `add_<model>` permission of the trigger or the
//! document owner (M2); applying from the review list only with objects that the applying user
//! can see, new objects belong to them (AI-N2); pagination bounded (AI-N3).

use axum::body::Bytes;
use axum::extract::{Path, Query as AxQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_authz::{AuthUser, User};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Map, Value, json};
use sqlx::Row;

use super::data::{self, Cat};
use super::workflow::{self, DocChanges, DocState, DocWriter};
use crate::auto::{self, Field, Plan, PlannedChange, RunLimits, Visible};
use crate::config::AutoMode;
use crate::mcp::api::SideStore;
use crate::pg::PgSideStore;
use crate::schema::ClassificationSuggestions;
use crate::taxonomy::{Alias, MatchOptions, TaxonomyEntry, TaxonomyKind, check_duplicate};

fn kind_of(cat: Cat) -> TaxonomyKind {
    match cat {
        Cat::Tags => TaxonomyKind::Tag,
        Cat::Correspondents => TaxonomyKind::Correspondent,
        Cat::DocumentTypes => TaxonomyKind::DocumentType,
        Cat::StoragePaths => TaxonomyKind::StoragePath,
    }
}

fn cat_of(f: Field) -> Option<Cat> {
    match f {
        Field::Tags => Some(Cat::Tags),
        Field::Correspondent => Some(Cat::Correspondents),
        Field::DocumentType => Some(Cat::DocumentTypes),
        Field::StoragePath => Some(Cat::StoragePaths),
        _ => None,
    }
}

/// `PAPERRUST_AI_AUTO_CREATE_MISSING` (Standard nein).
fn create_missing() -> bool {
    pl_core::config::var("PAPERRUST_AI_AUTO_CREATE_MISSING").and_then(|v| crate::config::str_to_bool(&v).ok()).unwrap_or(false)
}

/// Security wave 3 M2: may `creator` create an object of kind `cat`? (`user.has_perm("documents.add_<model>")`
/// like upstream DRF `PaperlessModelPermissions` for `POST /api/<model>/`). Without a user (system-triggered,
/// document without owner) never.
pub fn may_create(creator: Option<&User>, cat: Cat) -> bool {
    cat != Cat::StoragePaths && creator.is_some_and(|u| u.has_perm(&cat.model().perm("add")))
}

/// Maximum number of documents per manual run `POST /api/x-ai/run/` (security wave 3 M1).
pub const MANUAL_RUN_MAX_DOCUMENTS: usize = 10;

/// `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` – documents per user and hour via `/api/x-ai/run/`
/// (default 100; security wave 3 M1).
fn run_documents_per_hour() -> u32 {
    pl_core::config::var("PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(100)
}

fn run_budget() -> &'static crate::mcp::ratelimit::Budget {
    static B: std::sync::OnceLock<crate::mcp::ratelimit::Budget> = std::sync::OnceLock::new();
    B.get_or_init(|| crate::mcp::ratelimit::Budget::new(run_documents_per_hour(), std::time::Duration::from_secs(3600)))
}

/// Security wave 3 M1: mode of a manual run against `PAPERRUST_AI_AUTO_MODE`.
/// `off`/`suggest` → manual run only as `suggest` (writes suggestions only, changes no
/// documents; triggered by the user); `apply` → both. Coordinator decision 2026-11-08.
pub fn manual_run_mode(requested: AutoMode, configured: AutoMode) -> Result<AutoMode, &'static str> {
    match (configured, requested) {
        (_, AutoMode::Off) => Err("Invalid mode."),
        (AutoMode::Off, AutoMode::Apply) => Err("Automatic AI processing is disabled on this server; use mode \"suggest\"."),
        (AutoMode::Suggest, AutoMode::Apply) => {
            Err("Automatic application of AI changes is disabled on this server; use mode \"suggest\".")
        }
        (_, m) => Ok(m),
    }
}

/// Security wave 3 AI-N2: assigned existing objects of the chosen changes per kind.
fn assigned_ids(plan: &[PlannedChange], which: &[usize]) -> Vec<(Cat, Vec<i64>)> {
    let mut out: Vec<(Cat, Vec<i64>)> = Vec::new();
    for &i in which {
        let Some(c) = plan.get(i) else { continue };
        let (Some(id), Some(cat)) = (c.existing_id, cat_of(c.field)) else { continue };
        match out.iter_mut().find(|(k, _)| *k == cat) {
            Some((_, v)) => {
                if !v.contains(&id) {
                    v.push(id);
                }
            }
            None => out.push((cat, vec![id])),
        }
    }
    out
}

/// Security wave 3 AI-N3: `LIMIT`/`OFFSET` of the review list (`page_size` 1…100, overflow → `None`).
fn page_window(page: Option<i64>, page_size: Option<i64>) -> Option<(i64, i64)> {
    let size = page_size.unwrap_or(25).clamp(1, 100);
    let page = page.unwrap_or(1).max(1);
    Some((size, (page - 1).checked_mul(size)?))
}

/// Visible objects + aliases per kind (for planning and duplicate lock).
pub struct Taxonomy {
    pub entries: Vec<TaxonomyEntry>,
    pub aliases: Vec<Alias>,
}

pub async fn load_taxonomy(state: &AppState, user: Option<&User>) -> Result<Taxonomy, ApiError> {
    let mut entries = Vec::new();
    let mut aliases = Vec::new();
    let store = PgSideStore::new(state.db().clone());
    for cat in Cat::ALL {
        let kind = kind_of(cat);
        let pool = data::permitted_pool(state.db(), user, cat).await?;
        let ids: std::collections::HashSet<i64> = pool.iter().map(|(i, _)| *i).collect();
        entries.extend(pool.into_iter().map(|(id, name)| TaxonomyEntry {
            kind,
            id,
            name,
            owner_id: None,
            extra: None,
            document_count: None,
        }));
        if let Ok(a) = store.aliases(kind).await {
            aliases.extend(a.into_iter().filter(|a| ids.contains(&a.target_id)));
        }
    }
    Ok(Taxonomy { entries, aliases })
}

fn limits(cfg: &crate::config::AiConfig) -> RunLimits {
    RunLimits {
        max_documents: cfg.rs.auto_max_documents_per_run,
        max_new_objects: cfg.rs.auto_max_new_objects,
        max_changes_per_document: 8,
        min_confidence_apply: cfg.rs.auto_min_confidence,
    }
}

const FIELDS: [Field; 6] = [Field::Title, Field::Tags, Field::Correspondent, Field::DocumentType, Field::StoragePath, Field::Created];

/// State of the fields touched by `changes` (for audit and undo).
fn snapshot(cur: &DocState, fields: &[Field]) -> Value {
    let mut m = Map::new();
    for f in fields {
        match f {
            Field::Title => m.insert("title".into(), json!(cur.title)),
            Field::Correspondent => m.insert("correspondent".into(), json!(cur.correspondent_id)),
            Field::DocumentType => m.insert("document_type".into(), json!(cur.document_type_id)),
            Field::StoragePath => m.insert("storage_path".into(), json!(cur.storage_path_id)),
            Field::Created => m.insert("created".into(), json!(cur.created.format("%Y-%m-%d").to_string())),
            Field::Tags => m.insert("tags".into(), json!(cur.tag_ids)),
            Field::CustomFields => None,
        };
    }
    Value::Object(m)
}

/// Field names of an audit snapshot (`title`, `tags`, `custom_fields.<id>`, …) – what may be
/// logged about an audit entry; values (titles, custom field contents) never are (finding 7).
pub fn audit_field_names(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(o) = v.as_object() {
        for (k, x) in o {
            match (k.as_str(), x.as_object()) {
                ("custom_fields", Some(cf)) => out.extend(cf.keys().map(|id| format!("custom_fields.{id}"))),
                _ => out.push(k.clone()),
            }
        }
    }
    out
}

/// AI audit entry on the caller's connection (inside the change's transaction). A failure is an
/// error (the change must not stay without audit) and is logged without values.
#[allow(clippy::too_many_arguments)]
async fn insert_audit(
    conn: &mut sqlx::PgConnection,
    actor: Option<i64>,
    run: uuid::Uuid,
    action: &str,
    doc: i64,
    before: &Value,
    after: &Value,
    conf: Option<f64>,
    cands: Option<Value>,
) -> Result<i64, String> {
    let r = sqlx::query_scalar::<_, i64>(
        "INSERT INTO paperrust_ai_audit (actor_id, channel, run_id, action, object_kind, object_id, candidates, before, after, confidence) \
         VALUES ($1, 'auto', $2, $3, 'document', $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(actor.map(|a| a as i32))
    .bind(run)
    .bind(action)
    .bind(doc as i32)
    .bind(cands)
    .bind(before)
    .bind(after)
    .bind(conf)
    .fetch_one(&mut *conn)
    .await;
    r.map_err(|e| {
        let fields = audit_field_names(after).join(",");
        tracing::warn!(target: "pl_ai::audit", error = %e, document = doc, action, fields = %fields, "ai audit write failed; change not applied");
        "AI audit could not be written".to_string()
    })
}

/// Custom field values `ids` as `{"<id>": value|null}` (for audit and undo).
fn cf_snapshot(values: &std::collections::BTreeMap<i64, (String, Value)>, ids: &[i64]) -> Value {
    let mut m = Map::new();
    for id in ids {
        m.insert(id.to_string(), values.get(id).map(|(_, v)| v.clone()).unwrap_or(Value::Null));
    }
    Value::Object(m)
}

/// Document writes of one AI change (apply, undo, compensation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Writes {
    pub ch: DocChanges,
    pub add_tags: Vec<i64>,
    pub remove_tags: Vec<i64>,
    /// `(field id, data type, value)`
    pub cf_set: Vec<(i64, String, Value)>,
    pub cf_remove: Vec<i64>,
}

impl Writes {
    pub fn is_empty(&self) -> bool {
        *self == Writes::default()
    }
}

enum Step<'a> {
    Save(&'a DocChanges),
    AddTags(&'a [i64]),
    RemoveTags(&'a [i64]),
    SetCf(i64, &'a str, &'a Value),
    RemoveCf(i64),
}

fn steps(w: &Writes) -> Vec<Step<'_>> {
    let mut out = Vec::new();
    if !w.is_empty() {
        // save(update_fields=[…, "modified"]) – also when only tags/custom fields change
        out.push(Step::Save(&w.ch));
    }
    if !w.add_tags.is_empty() {
        out.push(Step::AddTags(&w.add_tags));
    }
    if !w.remove_tags.is_empty() {
        out.push(Step::RemoveTags(&w.remove_tags));
    }
    out.extend(w.cf_set.iter().map(|(f, dt, v)| Step::SetCf(*f, dt.as_str(), v)));
    out.extend(w.cf_remove.iter().map(|f| Step::RemoveCf(*f)));
    out
}

/// Write path of the AI changes: through the caller's transaction when the writer supports it
/// (`atomic`), otherwise through the writer's own (pool) methods with compensation on failure.
pub struct Ops {
    pub w: std::sync::Arc<dyn DocWriter>,
    pub atomic: bool,
}

impl Ops {
    pub fn new(state: &AppState) -> Self {
        let w = workflow::writer(state);
        let atomic = w.supports_transaction();
        Ops { w, atomic }
    }

    async fn step(&self, state: &AppState, conn: Option<&mut sqlx::PgConnection>, doc: i64, s: &Step<'_>) -> Result<(), String> {
        match (self.atomic, conn) {
            (true, Some(c)) => match s {
                Step::Save(ch) => self.w.save_tx(c, doc, ch).await,
                Step::AddTags(t) => self.w.add_nested_tags_tx(c, doc, t).await,
                Step::RemoveTags(t) => self.w.remove_tags_tx(c, doc, t).await,
                Step::SetCf(f, dt, v) => self.w.set_custom_field_tx(c, doc, *f, dt, v).await,
                Step::RemoveCf(f) => self.w.remove_custom_field_tx(c, doc, *f).await,
            },
            (true, None) => Err("internal: transactional write without a transaction".into()),
            (false, _) => match s {
                Step::Save(ch) => self.w.save(state, doc, ch).await,
                Step::AddTags(t) => self.w.add_nested_tags(state, doc, t).await,
                Step::RemoveTags(t) => self.w.remove_tags(state, doc, t).await,
                Step::SetCf(f, dt, v) => self.w.set_custom_field(state, doc, *f, dt, v).await,
                Step::RemoveCf(f) => self.w.remove_custom_field(state, doc, *f).await,
            },
        }
    }

    async fn get_or_create(
        &self,
        state: &AppState,
        conn: &mut sqlx::PgConnection,
        cat: Cat,
        name: &str,
        owner: Option<i64>,
    ) -> Result<(i64, bool), String> {
        if self.atomic {
            self.w.get_or_create_tx(conn, cat, name, owner).await
        } else {
            self.w.get_or_create(state, cat, name, owner).await
        }
    }

    /// Performs `w`. Atomic: on the caller's transaction (the caller rolls back on error). Otherwise a
    /// failure after the first successful write applies `revert` (best effort) before the error is returned.
    pub async fn execute(
        &self,
        state: &AppState,
        mut conn: Option<&mut sqlx::PgConnection>,
        doc: i64,
        w: &Writes,
        revert: Option<&Writes>,
    ) -> Result<(), String> {
        let mut wrote = false;
        for s in steps(w) {
            if let Err(e) = self.step(state, conn.as_deref_mut(), doc, &s).await {
                if wrote && let Some(r) = revert {
                    self.compensate(state, doc, r).await;
                }
                return Err(e);
            }
            wrote = true;
        }
        Ok(())
    }

    /// Non-atomic writers only: restore the previous values after a later failure (each step is
    /// tried, failures are logged without values).
    pub async fn compensate(&self, state: &AppState, doc: i64, r: &Writes) {
        if self.atomic {
            return;
        }
        for s in steps(r) {
            if let Err(e) = self.step(state, None, doc, &s).await {
                tracing::error!(target: "pl_ai", document = doc, error = %e, "restoring the document after a failed AI change failed");
            }
        }
    }

    /// After the commit: the writer's post-commit work (file renaming) and cache/index refresh.
    async fn after_commit(&self, state: &AppState, doc: i64) {
        if self.atomic
            && let Err(e) = self.w.after_commit(state, doc).await
        {
            tracing::warn!(target: "pl_ai", document = doc, error = %e, "post-commit work after an AI change failed");
        }
        workflow::after_change(state, doc).await;
    }
}

/// Writes that bring the snapshot fields of `current` back to `target` (snapshot format of
/// [`snapshot`] + `custom_fields`). `dts`: data type per custom field id.
pub fn restore_writes(target: &Value, current: &Value, dts: &std::collections::BTreeMap<i64, String>) -> Writes {
    let mut w = Writes::default();
    let Some(t) = target.as_object() else { return w };
    let differs = |k: &str| current.get(k) != t.get(k);
    if differs("title")
        && let Some(x) = t.get("title").and_then(Value::as_str)
    {
        w.ch.title = Some(x.to_string());
    }
    if t.contains_key("correspondent") && differs("correspondent") {
        w.ch.correspondent_id = t.get("correspondent").map(Value::as_i64);
    }
    if t.contains_key("document_type") && differs("document_type") {
        w.ch.document_type_id = t.get("document_type").map(Value::as_i64);
    }
    if t.contains_key("storage_path") && differs("storage_path") {
        w.ch.storage_path_id = t.get("storage_path").map(Value::as_i64);
    }
    if differs("created")
        && let Some(d) = t.get("created").and_then(Value::as_str)
    {
        w.ch.created = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    }
    if let Some(tb) = t.get("tags").and_then(Value::as_array) {
        let tb: Vec<i64> = tb.iter().filter_map(Value::as_i64).collect();
        let tc: Vec<i64> =
            current.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
        w.remove_tags = tc.iter().copied().filter(|x| !tb.contains(x)).collect();
        w.add_tags = tb.iter().copied().filter(|x| !tc.contains(x)).collect();
    }
    if let Some(cf) = t.get("custom_fields").and_then(Value::as_object) {
        for (k, v) in cf {
            let Ok(fid) = k.parse::<i64>() else { continue };
            let cur = current.get("custom_fields").and_then(|c| c.get(k)).cloned().unwrap_or(Value::Null);
            if cur == *v {
                continue;
            }
            if v.is_null() {
                w.cf_remove.push(fid);
            } else if let Some(dt) = dts.get(&fid) {
                w.cf_set.push((fid, dt.clone(), v.clone()));
            }
        }
    }
    w
}

fn merge_snapshot(sim: &mut Value, src: &Value) {
    let (Some(dst), Some(src)) = (sim.as_object_mut(), src.as_object()) else { return };
    for (k, v) in src {
        if k == "custom_fields"
            && let Some(cf) = v.as_object()
        {
            let entry = dst.entry(k.clone()).or_insert_with(|| Value::Object(Map::new()));
            if let Some(m) = entry.as_object_mut() {
                for (id, x) in cf {
                    m.insert(id.clone(), x.clone());
                }
            }
        } else {
            dst.insert(k.clone(), v.clone());
        }
    }
}

/// Undo of several audit entries `(before, after)` in apply order (finding 6): checks newest →
/// oldest that each entry's `after` still matches the (simulated) current state and returns the
/// state before the oldest entry; otherwise the conflicting fields.
pub fn plan_undo(now: &Value, audits: &[(Value, Value)]) -> Result<Value, Vec<String>> {
    let mut sim = now.clone();
    let mut conflicts: Vec<String> = Vec::new();
    for (before, after) in audits.iter().rev() {
        if let Some(a) = after.as_object() {
            for (k, v) in a {
                let ok = if k == "custom_fields" {
                    v.as_object().is_none_or(|m| m.iter().all(|(id, x)| sim.get("custom_fields").and_then(|c| c.get(id)) == Some(x)))
                } else {
                    sim.get(k.as_str()) == Some(v)
                };
                if !ok && !conflicts.contains(k) {
                    conflicts.push(k.clone());
                }
            }
        }
        merge_snapshot(&mut sim, before);
    }
    if conflicts.is_empty() { Ok(sim) } else { Err(conflicts) }
}

/// Result of [`apply_changes`].
#[derive(Debug, Default)]
struct Applied {
    done: Vec<usize>,
    audit: Option<i64>,
    /// Writes that undo the change (non-atomic writers: compensation if the caller fails later).
    revert: Writes,
}

/// Applies the marked changes on `conn` (the caller's transaction) incl. the audit entry. New objects
/// are created only by whoever has the `add_<model>` permission (`creator`, [`may_create`]); otherwise
/// the change stays open (suggestion only). The caller commits and then calls [`Ops::after_commit`].
#[allow(clippy::too_many_arguments)]
async fn apply_changes(
    state: &AppState,
    conn: &mut sqlx::PgConnection,
    ops: &Ops,
    doc_id: i64,
    owner_id: Option<i64>,
    creator: Option<&User>,
    actor: Option<i64>,
    run: uuid::Uuid,
    plan: &mut [PlannedChange],
    which: &[usize],
    tax: &Taxonomy,
) -> Result<Applied, String> {
    let Some(cur) = workflow::load_state_conn(conn, doc_id, ops.atomic).await? else { return Err("document not found".into()) };
    let mut w = Writes::default();
    let mut done = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let opts = MatchOptions::default();
    let mut defs: Option<Vec<crate::custom_fields::FieldDef>> = None;
    for &i in which {
        let Some(c) = plan.get(i).cloned() else { continue };
        let id = match (&c.existing_id, &c.new_name, cat_of(c.field)) {
            (Some(id), _, _) => Some(*id),
            (None, Some(name), Some(cat)) if cat != Cat::StoragePaths => {
                // duplicate lock right before creating
                let kind = kind_of(cat);
                let ents: Vec<TaxonomyEntry> = tax.entries.iter().filter(|e| e.kind == kind).cloned().collect();
                let d = check_duplicate(kind, name, &ents, &tax.aliases, &opts);
                if let Some(best) = d.candidates.iter().find(|c| c.above_threshold) {
                    Some(best.id)
                } else if !may_create(creator, cat) {
                    // Security wave 3 M2: without add_<model> permission only a suggestion
                    continue;
                } else {
                    Some(ops.get_or_create(state, conn, cat, name, owner_id).await?.0)
                }
            }
            _ => None,
        };
        match c.field {
            Field::Title => w.ch.title = c.value.clone(),
            Field::Created => w.ch.created = c.value.as_deref().and_then(|v| chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()),
            Field::Correspondent => w.ch.correspondent_id = id.map(Some),
            Field::DocumentType => w.ch.document_type_id = id.map(Some),
            Field::StoragePath => w.ch.storage_path_id = id.map(Some),
            Field::Tags => {
                if let Some(t) = id
                    && !cur.tag_ids.contains(&t)
                    && !w.add_tags.contains(&t)
                {
                    w.add_tags.push(t);
                }
            }
            Field::CustomFields => match (c.existing_id, c.data.clone()) {
                (Some(fid), Some(v)) if !w.cf_set.iter().any(|(f, _, _)| *f == fid) => {
                    if defs.is_none() {
                        defs = Some(super::store::load_field_defs(state.db()).await.map_err(|e| e.status().to_string())?);
                    }
                    let Some(def) = defs.as_ref().and_then(|d| d.iter().find(|d| d.id == fid)) else {
                        return Err("custom field not found".into());
                    };
                    w.cf_set.push((fid, def.data_type.clone(), v));
                }
                _ => continue,
            },
        }
        if !fields.contains(&c.field) {
            fields.push(c.field);
        }
        done.push(i);
    }
    if done.is_empty() {
        return Ok(Applied::default());
    }
    let cf_ids: Vec<i64> = w.cf_set.iter().map(|(f, _, _)| *f).collect();
    let cf_before = if cf_ids.is_empty() { Default::default() } else { workflow::load_custom_values_conn(conn, doc_id).await? };
    let mut before = snapshot(&cur, &fields);
    if !cf_ids.is_empty()
        && let Some(o) = before.as_object_mut()
    {
        o.insert("custom_fields".into(), cf_snapshot(&cf_before, &cf_ids));
    }
    // inverse of `w` (compensation for non-atomic writers)
    let mut revert = Writes::default();
    let c = &w.ch;
    revert.ch = DocChanges {
        title: c.title.as_ref().map(|_| cur.title.clone()),
        correspondent_id: c.correspondent_id.map(|_| cur.correspondent_id),
        document_type_id: c.document_type_id.map(|_| cur.document_type_id),
        storage_path_id: c.storage_path_id.map(|_| cur.storage_path_id),
        created: c.created.map(|_| cur.created),
    };
    revert.remove_tags = w.add_tags.clone();
    for (fid, dt, _) in &w.cf_set {
        match cf_before.get(fid).map(|(_, v)| v).filter(|v| !v.is_null()) {
            Some(v) => revert.cf_set.push((*fid, dt.clone(), v.clone())),
            None => revert.cf_remove.push(*fid),
        }
    }
    ops.execute(state, Some(&mut *conn), doc_id, &w, Some(&revert)).await?;
    let after_res = async {
        let after_state = workflow::load_state_conn(conn, doc_id, false).await?.ok_or("document vanished")?;
        let mut after = snapshot(&after_state, &fields);
        if !cf_ids.is_empty() {
            let cf_after = workflow::load_custom_values_conn(conn, doc_id).await?;
            if let Some(o) = after.as_object_mut() {
                o.insert("custom_fields".into(), cf_snapshot(&cf_after, &cf_ids));
            }
        }
        let conf =
            done.iter().filter_map(|i| plan.get(*i).map(|c| c.confidence)).fold(None, |a: Option<f64>, c| Some(a.map_or(c, |x| x.min(c))));
        insert_audit(conn, actor, run, "update_document", doc_id, &before, &after, conf, None).await
    }
    .await;
    match after_res {
        Ok(audit) => Ok(Applied { done, audit: Some(audit), revert }),
        Err(e) => {
            ops.compensate(state, doc_id, &revert).await;
            Err(e)
        }
    }
}

/// Custom fields via AI (template `custom_fields`): only enabled fields that the document has not
/// filled yet; model error → no suggestions (the classification stays valid).
#[allow(clippy::too_many_arguments)]
async fn custom_field_plan(
    state: &AppState,
    cfg: &crate::config::AiConfig,
    prompts: &crate::prompt_store::PromptSet,
    ai_opts: &crate::options::AiOptions,
    doc: &data::AiDoc,
    owner: Option<&User>,
    mode: AutoMode,
    lim: &RunLimits,
) -> Vec<PlannedChange> {
    use crate::prompt_store::PromptKind;
    let Ok(defs) = super::store::load_field_defs(state.db()).await else { return Vec::new() };
    let fields: Vec<_> = defs
        .into_iter()
        .filter(|f| ai_opts.custom_fields.contains(&f.id) && crate::custom_fields::supported(&f.data_type))
        .take(20)
        .collect();
    if fields.is_empty() {
        return Vec::new();
    }
    let prompt =
        super::store::render_for_document(state, cfg, PromptKind::CustomFields, prompts.get(PromptKind::CustomFields), doc, owner, &fields)
            .await;
    let Ok(client) = crate::llm::LlmClient::from_config(cfg) else { return Vec::new() };
    let answer = match client.complete_json(&prompt, "CustomFields", &crate::custom_fields::response_schema()).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(target: "pl_ai", document = doc.id, error = %e, "custom field extraction failed");
            return Vec::new();
        }
    };
    let values = crate::custom_fields::parse_answer(&fields, &answer);
    let filled: Vec<i64> = workflow::load_custom_values(state, doc.id)
        .await
        .map(|m| m.into_iter().filter(|(_, (_, v))| !v.is_null()).map(|(k, _)| k).collect())
        .unwrap_or_default();
    auto::custom_field_changes(&values, &filled, mode, lim)
}

/// Open suggestions (`pending`/`partial`) over documents that `user` may see.
pub async fn pending_count(state: &AppState, user: &User) -> Result<i64, ApiError> {
    use sea_query::{Alias as A, Expr};
    let mut q = review_list_query(user, None, None);
    q.and_where(Expr::col(A::new("status")).is_in(["pending", "partial"]));
    q.expr(Expr::cust("count(*)"));
    let (sql, values) = q.build_sqlx(sea_query::PostgresQueryBuilder);
    sqlx::query_scalar_with(&sql, values).fetch_one(state.db()).await.map_err(not_installed)
}

/// Result for a document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocResult {
    pub document: i64,
    pub review: Option<i64>,
    pub status: String,
    pub applied: usize,
    pub pending: usize,
    pub error: Option<String>,
}

/// Processes a document (mode from the configuration or given).
pub async fn process_document(
    state: &AppState,
    doc_id: i64,
    mode: AutoMode,
    trigger: &str,
    actor: Option<&User>,
    run: uuid::Uuid,
    new_objects_left: &mut u32,
) -> DocResult {
    let mut res = DocResult { document: doc_id, review: None, status: "skipped".into(), applied: 0, pending: 0, error: None };
    let inner = async {
        let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
        if !cfg.ai_enabled || mode == AutoMode::Off {
            return Err("AI disabled".to_string());
        }
        let doc = data::load_doc(state.db(), doc_id).await.map_err(|e| e.status().to_string())?.ok_or("document not found")?;
        let owner = match (actor, doc.owner_id) {
            (Some(a), _) => Some(a.clone()),
            (None, Some(o)) => pl_authz::user::load_user_by_id(state.db(), o).await.map_err(|e| e.to_string())?,
            (None, None) => None,
        };
        let ui = match &owner {
            Some(u) => data::ui_settings(state.db(), u.id).await.ok().flatten(),
            None => None,
        };
        let lang = crate::classifier::get_llm_output_language(cfg.llm_output_language.as_deref(), ui.as_ref());
        let mut s: ClassificationSuggestions =
            super::suggest::classify(state, &cfg, &doc, owner.as_ref(), lang.as_deref()).await.map_err(|e| e.to_string())?;
        // PaperRust: own title template → own title call; custom fields via AI
        let prompts = super::store::load_prompts(state.db()).await;
        let ai_opts = super::store::load_options(state.db()).await;
        if let Some(t) = prompts.custom(crate::prompt_store::PromptKind::Title) {
            let p =
                super::store::render_for_document(state, &cfg, crate::prompt_store::PromptKind::Title, t, &doc, owner.as_ref(), &[]).await;
            if let Ok(c) = crate::llm::LlmClient::from_config(&cfg)
                && let Ok(answer) = c.with_system_prompt(true).complete_text(None, &p).await
            {
                let title = super::store::clean_title(&answer);
                if !title.is_empty() {
                    s.title = title;
                }
            }
        }
        let tax = load_taxonomy(state, owner.as_ref()).await.map_err(|e| e.status().to_string())?;
        let vis = |_k: TaxonomyKind| Visible { entries: &tax.entries, aliases: &tax.aliases };
        let lim = limits(&cfg);
        let opts = MatchOptions { threshold: cfg.rs.match_threshold, ..MatchOptions::default() };
        let mut p: Plan = auto::plan(&s, &FIELDS, mode, &lim, create_missing(), *new_objects_left, &vis, &opts);
        if ai_opts.custom_fields_enabled && !ai_opts.custom_fields.is_empty() {
            p.changes.extend(custom_field_plan(state, &cfg, &prompts, &ai_opts, &doc, owner.as_ref(), mode, &lim).await);
        }
        // unusual output (many new names) → suggest only (prompt-injection protection)
        if s.tags.new_names.len() > 8 {
            for c in p.changes.iter_mut() {
                c.apply = false;
            }
        }
        *new_objects_left = new_objects_left.saturating_sub(p.new_objects);
        let which: Vec<usize> = p.changes.iter().enumerate().filter(|(_, c)| c.apply).map(|(i, _)| i).collect();
        // finding 5: changes, audit and review entry in one transaction
        let ops = Ops::new(state);
        let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
        let applied = if which.is_empty() {
            Applied::default()
        } else {
            apply_changes(
                state,
                &mut tx,
                &ops,
                doc.id,
                owner.as_ref().map(|u| u.id).or(doc.owner_id),
                owner.as_ref(),
                actor.map(|a| a.id),
                run,
                &mut p.changes,
                &which,
                &tax,
            )
            .await?
        };
        let (done, audit) = (applied.done.clone(), applied.audit);
        let plan_json: Vec<Value> = p
            .changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
                if let Some(o) = v.as_object_mut() {
                    o.insert("applied".into(), json!(done.contains(&i)));
                }
                v
            })
            .collect();
        let status = if p.changes.is_empty() || done.len() == p.changes.len() && !done.is_empty() {
            if done.is_empty() { "rejected" } else { "applied" }
        } else if done.is_empty() {
            "pending"
        } else {
            "partial"
        };
        let inserted: Result<i64, String> = sqlx::query_scalar(
            "INSERT INTO paperrust_ai_review (document_id, run_id, trigger, mode, status, suggestions, plan, audit_id, audit_ids) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, CASE WHEN $8::bigint IS NULL THEN '{}'::bigint[] ELSE ARRAY[$8::bigint] END) RETURNING id",
        )
        .bind(doc.id as i32)
        .bind(run)
        .bind(trigger)
        .bind(if mode == AutoMode::Apply { "apply" } else { "suggest" })
        .bind(status)
        .bind(serde_json::to_value(&s).unwrap_or(Value::Null))
        .bind(Value::Array(plan_json))
        .bind(audit)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            tracing::warn!(target: "pl_ai", error = %e, document = doc.id, "writing the review entry failed");
            "review storage not available".to_string()
        });
        let committed = match inserted {
            Ok(id) => tx.commit().await.map(|_| id).map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        let review = match committed {
            Ok(id) => id,
            Err(e) => {
                // the transaction was rolled back; non-atomic writers: restore the document
                ops.compensate(state, doc.id, &applied.revert).await;
                return Err(e);
            }
        };
        if !done.is_empty() {
            ops.after_commit(state, doc.id).await;
        }
        Ok::<_, String>((Some(review), status.to_string(), done.len(), p.changes.len() - done.len()))
    };
    match inner.await {
        Ok((review, status, applied, pending)) => {
            res.review = review;
            res.status = status;
            res.applied = applied;
            res.pending = pending;
        }
        Err(e) => {
            res.status = "error".into();
            res.error = Some(e);
        }
    }
    res
}

/// A run over several documents (limit `max_documents`).
pub async fn run(state: &AppState, doc_ids: &[i64], mode: AutoMode, trigger: &str, actor: Option<&User>) -> Result<Value, ApiError> {
    let cfg = super::effective_config(state).await?;
    let run = uuid::Uuid::new_v4();
    let mut left = cfg.rs.auto_max_new_objects;
    let mut out = Vec::new();
    for id in doc_ids.iter().take(cfg.rs.auto_max_documents_per_run as usize) {
        out.push(process_document(state, *id, mode, trigger, actor, run, &mut left).await);
    }
    Ok(json!({"run_id": run.to_string(), "mode": if mode == AutoMode::Apply { "apply" } else { "suggest" }, "results": out,
              "skipped": doc_ids.len().saturating_sub(out.len())}))
}

/// Trigger tag (`PAPERRUST_AI_AUTO_TRIGGER_TAG`): process documents with the tag and then
/// remove the tag. For the worker's scheduler (W1).
pub async fn process_trigger_tag(state: &AppState) -> Result<Value, ApiError> {
    let cfg = super::effective_config(state).await?;
    let Some(tag) = cfg.rs.auto_trigger_tag.clone() else { return Ok(Value::Null) };
    if cfg.rs.auto_mode == AutoMode::Off || !cfg.ai_enabled {
        return Ok(Value::Null);
    }
    let rows = sqlx::query(
        "SELECT dt.document_id, dt.tag_id FROM documents_document_tags dt JOIN documents_tag t ON t.id = dt.tag_id \
         JOIN documents_document d ON d.id = dt.document_id WHERE t.name = $1 AND d.deleted_at IS NULL ORDER BY dt.document_id LIMIT $2",
    )
    .bind(&tag)
    .bind(i64::from(cfg.rs.auto_max_documents_per_run))
    .fetch_all(state.db())
    .await
    .map_err(ApiError::internal)?;
    let pairs: Vec<(i64, i64)> =
        rows.iter().filter_map(|r| Some((i64::from(r.try_get::<i32, _>(0).ok()?), i64::from(r.try_get::<i32, _>(1).ok()?)))).collect();
    let opts = super::store::load_options(state.db()).await;
    let mut pairs = pairs;
    if opts.tag_after == crate::options::TagAfter::Keep {
        // tag stays: skip already processed documents (review-list entry present)
        let ids: Vec<i32> = pairs.iter().map(|p| p.0 as i32).collect();
        let seen: Vec<i32> = sqlx::query_scalar("SELECT DISTINCT document_id FROM paperrust_ai_review WHERE document_id = ANY($1)")
            .bind(&ids)
            .fetch_all(state.db())
            .await
            .unwrap_or_default();
        pairs.retain(|p| !seen.contains(&(p.0 as i32)));
    }
    let ids: Vec<i64> = pairs.iter().map(|p| p.0).collect();
    if ids.is_empty() {
        return Ok(Value::Null);
    }
    let summary = run(state, &ids, cfg.rs.auto_mode, "tag", None).await?;
    let ops = Ops::new(state);
    // finding 8: without the replacement tag the trigger tags stay (no document loses both tags)
    let replacement: Option<i64> = match &opts.tag_after {
        crate::options::TagAfter::Replace { tag } => match ops.w.get_or_create(state, Cat::Tags, tag, None).await {
            Ok((id, _)) => Some(id),
            Err(e) => {
                tracing::warn!(target: "pl_ai", error = %e, "could not create replacement tag; trigger tags kept");
                return Err(ApiError::internal("could not create the replacement tag; trigger tags kept"));
            }
        },
        _ => None,
    };
    // always remove the trigger tag (no endless repetition on errors), replacement tag only on success
    let failed: Vec<i64> = summary
        .get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|r| r.get("status").and_then(Value::as_str) == Some("error"))
                .filter_map(|r| r.get("document").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default();
    let mut not_replaced = 0usize;
    for (d, t) in pairs {
        if opts.tag_after == crate::options::TagAfter::Keep {
            continue;
        }
        let r = replacement.filter(|_| !failed.contains(&d));
        if let Err(e) = replace_trigger_tag(state, &ops, d, t, r).await {
            tracing::warn!(target: "pl_ai", document = d, error = %e, "replacing the AI trigger tag failed; trigger tag kept");
            not_replaced += 1;
        }
    }
    if not_replaced > 0 {
        return Err(ApiError::internal(format!("replacing the AI trigger tag failed for {not_replaced} document(s); trigger tag kept")));
    }
    Ok(summary)
}

/// Removes the trigger tag `trigger` and adds `replacement` (if any) as one unit (finding 8):
/// atomic writers in one transaction; otherwise the replacement is added first, so a failure
/// leaves the trigger tag in place. Errors are returned, never dropped.
pub async fn replace_trigger_tag(state: &AppState, ops: &Ops, doc: i64, trigger: i64, replacement: Option<i64>) -> Result<(), String> {
    if ops.atomic {
        let mut tx = state.db().begin().await.map_err(|e| e.to_string())?;
        if let Some(r) = replacement {
            ops.w.add_nested_tags_tx(&mut tx, doc, &[r]).await?;
        }
        ops.w.remove_tags_tx(&mut tx, doc, &[trigger]).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        if let Err(e) = ops.w.after_commit(state, doc).await {
            tracing::warn!(target: "pl_ai", document = doc, error = %e, "post-commit work after the trigger tag change failed");
        }
        return Ok(());
    }
    if let Some(r) = replacement {
        ops.w.add_nested_tags(state, doc, &[r]).await?;
    }
    ops.w.remove_tags(state, doc, &[trigger]).await
}

// ------------------------------------------------------------------ REST /api/x-ai/review/

fn review_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "document": r.try_get::<i32, _>("document_id").unwrap_or_default(),
        "created": r.try_get::<chrono::DateTime<chrono::Utc>, _>("created").map(|d| pl_core::datetime::python_isoformat(&d)).unwrap_or_default(),
        "run_id": r.try_get::<uuid::Uuid, _>("run_id").map(|u| u.to_string()).unwrap_or_default(),
        "trigger": r.try_get::<String, _>("trigger").unwrap_or_default(),
        "mode": r.try_get::<String, _>("mode").unwrap_or_default(),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
        "suggestions": r.try_get::<Value, _>("suggestions").unwrap_or(Value::Null),
        "changes": r.try_get::<Value, _>("plan").unwrap_or(Value::Null),
        "audit": r.try_get::<Option<i64>, _>("audit_id").ok().flatten(),
        "audits": r.try_get::<Vec<i64>, _>("audit_ids").unwrap_or_default(),
        "decided_by": r.try_get::<Option<i32>, _>("decided_by_id").ok().flatten(),
        "decided_at": r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("decided_at").ok().flatten().map(|d| pl_core::datetime::python_isoformat(&d)),
    })
}

fn not_installed(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01")) {
        ApiError::with_detail(StatusCode::SERVICE_UNAVAILABLE, "AI review storage is not installed (migration 0001_paperrust_ai.sql).")
    } else {
        ApiError::internal(e)
    }
}

const REVIEW_COLUMNS: &str =
    "id, document_id, created, run_id, trigger, mode, status, suggestions, plan, audit_id, audit_ids, decided_by_id, decided_at";
const SELECT: &str = "SELECT id, document_id, created, run_id, trigger, mode, status, suggestions, plan, audit_id, audit_ids, decided_by_id, decided_at FROM paperrust_ai_review";

/// `FROM paperrust_ai_review WHERE document_id IN (<visible documents>) [AND status = …]
/// [AND document_id = …]` without a column list. Visibility as a subquery
/// (`permitted_document_ids(user, "view_document")`), not as a bound id array (AI-N3).
fn review_list_query(user: &User, status: Option<&str>, document: Option<i64>) -> sea_query::SelectStatement {
    use sea_query::{Alias as A, Expr};
    let mut q = sea_query::Query::select();
    q.from(A::new("paperrust_ai_review")).and_where(
        Expr::col(A::new("document_id")).in_subquery(pl_authz::objects::permitted_document_ids(Some(user), "view_document", false)),
    );
    match status {
        // PaperRust UI: "open" = pending + partial
        Some("open") => {
            q.and_where(Expr::col(A::new("status")).is_in(["pending", "partial"]));
        }
        Some(s) => {
            q.and_where(Expr::col(A::new("status")).eq(s));
        }
        None => {}
    }
    if let Some(d) = document {
        // int4 column: outside the range no entry fits
        match i32::try_from(d) {
            Ok(d) => q.and_where(Expr::col(A::new("document_id")).eq(d)),
            Err(_) => q.and_where(Expr::cust("FALSE")),
        };
    }
    q
}

/// GET /api/x-ai/review/?status=pending&document=<id>&page=&page_size=
pub async fn list(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    AxQuery(q): AxQuery<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let status = q.get("status").cloned();
    let doc = q.get("document").and_then(|d| super::py_int(d));
    let (page_size, offset) = page_window(q.get("page").and_then(|d| super::py_int(d)), q.get("page_size").and_then(|d| super::py_int(d)))
        .ok_or_else(ApiError::invalid_page)?;
    // Security review final AI-N3: visibility as a subquery instead of a list of all
    // visible document ids (previously bound twice per request)
    let base = review_list_query(user, status.as_deref(), doc);
    let mut count_q = base.clone();
    count_q.expr(sea_query::Expr::cust("count(*)"));
    let (sql, values) = count_q.build_sqlx(sea_query::PostgresQueryBuilder);
    let count: i64 = sqlx::query_scalar_with(&sql, values).fetch_one(state.db()).await.map_err(not_installed)?;
    let mut rows_q = base;
    rows_q
        .expr(sea_query::Expr::cust(REVIEW_COLUMNS))
        .order_by(sea_query::Alias::new("created"), sea_query::Order::Desc)
        .order_by(sea_query::Alias::new("id"), sea_query::Order::Desc)
        .limit(u64::try_from(page_size).unwrap_or(25))
        .offset(u64::try_from(offset).unwrap_or(0));
    let (sql, values) = rows_q.build_sqlx(sea_query::PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(state.db()).await.map_err(not_installed)?;
    let results: Vec<Value> = rows.iter().map(review_json).collect();
    Ok(Drf::ok(json!({"count": count, "results": results})).indent(ctx.negotiation.indent).into_response())
}

async fn load_review(state: &AppState, user: &User, id: &str, perm: &str) -> Result<sqlx::postgres::PgRow, ApiError> {
    let id = super::py_int(id).ok_or_else(ApiError::not_found)?;
    let r = sqlx::query(&format!("{SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_optional(state.db())
        .await
        .map_err(not_installed)?
        .ok_or_else(ApiError::not_found)?;
    let doc_id = i64::from(r.try_get::<i32, _>("document_id").map_err(ApiError::internal)?);
    let doc = data::load_doc(state.db(), doc_id).await?.ok_or_else(ApiError::not_found)?;
    let obj = ObjRef { id: doc.root_id, owner_id: doc.root_owner_id };
    if !has_perms_owner_aware(state.db(), Some(user), "view_document", &DOCUMENT, obj).await? {
        return Err(ApiError::not_found());
    }
    if perm != "view_document" && !has_perms_owner_aware(state.db(), Some(user), perm, &DOCUMENT, obj).await? {
        return Err(ApiError::permission_denied());
    }
    Ok(r)
}

/// GET /api/x-ai/review/{id}/
pub async fn retrieve(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth.require_model_perms(&DOCUMENT)?;
    let r = load_review(&state, user, &id, "view_document").await?;
    Ok(Drf::ok(review_json(&r)).indent(ctx.negotiation.indent).into_response())
}

fn body_json(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|_| ApiError::parse_error(Some("JSON parse error")))
}

/// Validates a change from the UI ("Edit") and inserts it into the planned change.
/// `o`: `{"value": …}` (title, date, custom field) or `{"existing_id": n}` (correspondent, type,
/// storage path, tag). Error = reason.
fn apply_override(c: &mut PlannedChange, o: &Value, defs: &[crate::custom_fields::FieldDef]) -> Result<(), String> {
    match c.field {
        Field::Title => {
            let t: String = o.get("value").and_then(Value::as_str).map(str::trim).unwrap_or("").chars().take(128).collect();
            if t.is_empty() {
                return Err("This field may not be blank.".into());
            }
            c.value = Some(t);
        }
        Field::Created => {
            let d = o.get("value").and_then(Value::as_str).and_then(crate::custom_fields::parse_date).ok_or("Invalid date.")?;
            c.value = Some(d.format("%Y-%m-%d").to_string());
        }
        Field::CustomFields => {
            let fid = c.existing_id.ok_or("Invalid field.")?;
            let def = defs.iter().find(|d| d.id == fid).ok_or("Invalid field.")?;
            let (v, display) = o.get("value").and_then(|v| crate::custom_fields::normalize(def, v)).ok_or("Invalid value.")?;
            c.data = Some(v);
            c.value = Some(display);
        }
        _ => {
            let id = o.get("existing_id").and_then(Value::as_i64).ok_or("Invalid object.")?;
            c.existing_id = Some(id);
            c.new_name = None;
        }
    }
    c.confidence = 1.0;
    c.why = "changed by user".into();
    Ok(())
}

/// Status of a review-list entry from `applied`/`dismissed` per change.
pub fn review_status(plan: &[Value]) -> &'static str {
    let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let applied = plan.iter().filter(|v| flag(v, "applied")).count();
    let decided = plan.iter().filter(|v| flag(v, "applied") || flag(v, "dismissed")).count();
    match (applied, decided == plan.len()) {
        (0, true) => "rejected",
        (_, true) => "applied",
        (0, false) => "pending",
        _ => "partial",
    }
}

/// POST /api/x-ai/review/{id}/apply/ – body optional `{"changes": [<index>…], "overrides":
/// {"<index>": {"value"|"existing_id": …}}, "dismiss": [<index>…]}` (default: apply all open ones).
/// `dismiss` discards individual suggestions ("Discard" per field), `overrides` applies a value
/// changed by the user.
pub async fn apply(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = auth.require_perms_by_method(&[("POST", &["documents.change_document"])])?;
    let r = load_review(&state, user, &id, "change_document").await?;
    let status: String = r.try_get("status").unwrap_or_default();
    if !matches!(status.as_str(), "pending" | "partial") {
        return Err(ApiError::validation(json!({"status": [format!("Review is {status}.")]})));
    }
    let b = body_json(&body)?;
    let mut plan: Vec<PlannedChange> = serde_json::from_value(r.try_get::<Value, _>("plan").unwrap_or(json!([]))).unwrap_or_default();
    let raw_plan: Vec<Value> = r.try_get::<Value, _>("plan").ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if raw_plan.len() != plan.len() {
        return Err(ApiError::internal("review plan unreadable"));
    }
    let decided = |i: usize| ["applied", "dismissed"].iter().any(|k| raw_plan[i].get(*k).and_then(Value::as_bool).unwrap_or(false));
    let dismiss: Vec<usize> = b
        .get("dismiss")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).filter(|i| *i < plan.len() && !decided(*i)).collect())
        .unwrap_or_default();
    let which: Vec<usize> = match b.get("changes").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_u64)
            .map(|x| x as usize)
            .filter(|i| *i < plan.len() && !decided(*i) && !dismiss.contains(i))
            .collect(),
        None if b.get("dismiss").is_some() => Vec::new(),
        None => (0..plan.len()).filter(|i| !decided(*i)).collect(),
    };
    // "Edit": validate and insert the user's values
    let mut overridden: Vec<usize> = Vec::new();
    if let Some(ov) = b.get("overrides").and_then(Value::as_object) {
        let defs =
            if plan.iter().any(|c| c.field == Field::CustomFields) { super::store::load_field_defs(state.db()).await? } else { Vec::new() };
        let mut errs = Map::new();
        for (k, o) in ov {
            let Some(i) = k.parse::<usize>().ok().filter(|i| which.contains(i)) else { continue };
            match apply_override(&mut plan[i], o, &defs) {
                Ok(()) => overridden.push(i),
                Err(e) => {
                    errs.insert(k.clone(), json!([e]));
                }
            }
        }
        if !errs.is_empty() {
            return Err(ApiError::validation(json!({"overrides": errs})));
        }
    }
    // New objects only with add_<model> permission
    for &i in &which {
        if let (Some(name), Some(cat)) = (&plan[i].new_name, cat_of(plan[i].field))
            && !user.has_perm(&cat.model().perm("add"))
        {
            return Err(ApiError::permission_denied_msg(format!("You may not create {} '{name}'.", cat.model().model_name)));
        }
    }
    // Security wave 3 AI-N2: assign only objects that the applying user may see
    for (cat, ids) in assigned_ids(&plan, &which) {
        let visible = data::visible_objects(state.db(), Some(user), cat, &ids).await?;
        if ids.iter().any(|id| !visible.iter().any(|(v, _)| v == id)) {
            return Err(ApiError::permission_denied_msg(format!("You may not assign this {}.", cat.model().model_name)));
        }
    }
    let doc_id = i64::from(r.try_get::<i32, _>("document_id").map_err(ApiError::internal)?);
    let rid: i64 = r.try_get("id").map_err(ApiError::internal)?;
    let run: uuid::Uuid = r.try_get("run_id").map_err(ApiError::internal)?;
    // finding 5: changes, audit and review update in one transaction
    let ops = Ops::new(&state);
    let mut tx = state.db().begin().await.map_err(ApiError::internal)?;
    let applied = if which.is_empty() {
        Applied::default()
    } else {
        let tax = load_taxonomy(&state, Some(user)).await?;
        // new objects belong to the applying user (AI-N2), not to the document owner
        apply_changes(&state, &mut tx, &ops, doc_id, Some(user.id), Some(user), Some(user.id), run, &mut plan, &which, &tax)
            .await
            .map_err(ApiError::internal)?
    };
    let (done, audit) = (applied.done.clone(), applied.audit);
    let mut new_plan = raw_plan.clone();
    for i in &overridden {
        if let (Some(o), Ok(v)) = (new_plan.get_mut(*i).and_then(Value::as_object_mut), serde_json::to_value(&plan[*i])) {
            if let Some(m) = v.as_object() {
                for (k, x) in m {
                    o.insert(k.clone(), x.clone());
                }
            }
            o.insert("changed_by_user".into(), json!(true));
        }
    }
    for i in &done {
        if let Some(o) = new_plan.get_mut(*i).and_then(Value::as_object_mut) {
            o.insert("applied".into(), json!(true));
        }
    }
    for i in &dismiss {
        if let Some(o) = new_plan.get_mut(*i).and_then(Value::as_object_mut) {
            o.insert("dismissed".into(), json!(true));
        }
    }
    let new_status = review_status(&new_plan);
    // finding 6: every partial acceptance keeps its audit id
    let updated = sqlx::query(&format!(
        "UPDATE paperrust_ai_review SET plan = $2, status = $3, audit_id = COALESCE($4, audit_id), \
         audit_ids = CASE WHEN $4::bigint IS NULL THEN audit_ids ELSE array_append(audit_ids, $4::bigint) END, \
         decided_by_id = $5, decided_at = now() WHERE id = $1 AND status IN ('pending', 'partial') RETURNING {REVIEW_COLUMNS}"
    ))
    .bind(rid)
    .bind(Value::Array(new_plan))
    .bind(new_status)
    .bind(audit)
    .bind(user.id as i32)
    .fetch_optional(&mut *tx)
    .await;
    let r = match updated {
        Ok(Some(r)) => match tx.commit().await {
            Ok(()) => r,
            Err(e) => {
                ops.compensate(&state, doc_id, &applied.revert).await;
                return Err(ApiError::internal(e));
            }
        },
        Ok(None) => {
            drop(tx);
            ops.compensate(&state, doc_id, &applied.revert).await;
            return Err(ApiError::validation(json!({"status": ["Review is already decided."]})));
        }
        Err(e) => {
            drop(tx);
            ops.compensate(&state, doc_id, &applied.revert).await;
            return Err(not_installed(e));
        }
    };
    if !done.is_empty() {
        ops.after_commit(&state, doc_id).await;
    }
    Ok(Drf::ok(review_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/review/{id}/reject/
pub async fn reject(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth.require_perms_by_method(&[("POST", &["documents.change_document"])])?;
    let r = load_review(&state, user, &id, "change_document").await?;
    let rid: i64 = r.try_get("id").map_err(ApiError::internal)?;
    let r = sqlx::query(
        "UPDATE paperrust_ai_review SET status = CASE WHEN status = 'partial' THEN 'applied' ELSE 'rejected' END, decided_by_id = $2, decided_at = now() \
         WHERE id = $1 AND status IN ('pending', 'partial') \
         RETURNING id, document_id, created, run_id, trigger, mode, status, suggestions, plan, audit_id, audit_ids, decided_by_id, decided_at",
    )
    .bind(rid)
    .bind(user.id as i32)
    .fetch_optional(state.db())
    .await
    .map_err(not_installed)?
    .ok_or_else(|| ApiError::validation(json!({"status": ["Review is already decided."]})))?;
    Ok(Drf::ok(review_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/review/{id}/undo/ – counter-change from the audit entries (`before`), only
/// without conflict. All audit entries of the review (every partial acceptance) are undone,
/// newest first, in one transaction with the undo audit entries (finding 6).
pub async fn undo(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth.require_perms_by_method(&[("POST", &["documents.change_document"])])?;
    let r = load_review(&state, user, &id, "change_document").await?;
    let rid: i64 = r.try_get("id").map_err(ApiError::internal)?;
    let mut ids: Vec<i64> = r.try_get::<Vec<i64>, _>("audit_ids").unwrap_or_default();
    if ids.is_empty()
        && let Some(a) = r.try_get::<Option<i64>, _>("audit_id").ok().flatten()
    {
        ids.push(a);
    }
    if ids.is_empty() {
        return Err(ApiError::validation(json!({"status": ["Nothing was applied."]})));
    }
    let rows = sqlx::query("SELECT id, before, after, object_id, undone_by_id FROM paperrust_ai_audit WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(state.db())
        .await
        .map_err(not_installed)?;
    // Security review final AI-N2: every audit must belong to the review's document (permissions
    // were checked in `load_review` for `review.document_id`, `object_id` is what is written).
    let review_doc = i64::from(r.try_get::<i32, _>("document_id").map_err(ApiError::internal)?);
    let mut audits: Vec<(i64, Value, Value)> = Vec::new();
    for aid in &ids {
        let Some(a) = rows.iter().find(|a| a.try_get::<i64, _>("id").ok() == Some(*aid)) else { continue };
        let doc_id = a.try_get::<Option<i32>, _>("object_id").ok().flatten().map(i64::from);
        if doc_id != Some(review_doc) {
            return Err(ApiError::validation(json!({"status": ["Nothing was applied."]})));
        }
        if a.try_get::<Option<i64>, _>("undone_by_id").ok().flatten().is_some() {
            continue;
        }
        audits.push((*aid, a.try_get("before").unwrap_or(Value::Null), a.try_get("after").unwrap_or(Value::Null)));
    }
    if audits.is_empty() {
        return Err(if rows.is_empty() { ApiError::not_found() } else { ApiError::validation(json!({"status": ["Already undone."]})) });
    }
    let doc_id = review_doc;
    let ops = Ops::new(&state);
    let mut tx = state.db().begin().await.map_err(ApiError::internal)?;
    let cur = workflow::load_state_conn(&mut tx, doc_id, ops.atomic).await.map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let mut fields: Vec<Field> = Vec::new();
    let mut cf_ids: Vec<i64> = Vec::new();
    for (_, _, after) in &audits {
        for k in after.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
            if let Ok(f) = serde_json::from_value::<Field>(Value::String(k))
                && !fields.contains(&f)
            {
                fields.push(f);
            }
        }
        for id in after
            .get("custom_fields")
            .and_then(Value::as_object)
            .map(|o| o.keys().filter_map(|k| k.parse().ok()).collect::<Vec<i64>>())
            .unwrap_or_default()
        {
            if !cf_ids.contains(&id) {
                cf_ids.push(id);
            }
        }
    }
    let mut now = snapshot(&cur, &fields);
    // Custom fields (PaperRust): current value of the affected fields
    let cf_now = if cf_ids.is_empty() {
        Default::default()
    } else {
        workflow::load_custom_values_conn(&mut tx, doc_id).await.map_err(ApiError::internal)?
    };
    if !cf_ids.is_empty()
        && let Some(o) = now.as_object_mut()
    {
        o.insert("custom_fields".into(), cf_snapshot(&cf_now, &cf_ids));
    }
    let pairs: Vec<(Value, Value)> = audits.iter().map(|(_, b, a)| (b.clone(), a.clone())).collect();
    let target = match plan_undo(&now, &pairs) {
        Ok(t) => t,
        Err(conflicts) => {
            return Err(ApiError::json(
                StatusCode::CONFLICT,
                json!({"detail": "The document was changed after the AI update; not undone.", "fields": conflicts}),
            ));
        }
    };
    let mut dts: std::collections::BTreeMap<i64, String> = cf_now.iter().map(|(k, (dt, _))| (*k, dt.clone())).collect();
    if cf_ids.iter().any(|id| !dts.contains_key(id)) {
        for d in super::store::load_field_defs(state.db()).await? {
            dts.entry(d.id).or_insert(d.data_type);
        }
    }
    let writes = restore_writes(&target, &now, &dts);
    let back = restore_writes(&now, &target, &dts);
    ops.execute(&state, Some(&mut *tx), doc_id, &writes, Some(&back)).await.map_err(ApiError::internal)?;
    let run: uuid::Uuid = r.try_get("run_id").map_err(ApiError::internal)?;
    let finished: Result<Option<sqlx::postgres::PgRow>, String> = async {
        for (aid, before, after) in audits.iter().rev() {
            let undo_id = insert_audit(&mut tx, Some(user.id), run, "undo", doc_id, after, before, None, None).await?;
            sqlx::query("UPDATE paperrust_ai_audit SET undone_by_id = $2 WHERE id = $1")
                .bind(aid)
                .bind(undo_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        }
        sqlx::query(&format!(
            "UPDATE paperrust_ai_review SET status = 'undone', decided_by_id = $2, decided_at = now() WHERE id = $1 AND status <> 'undone' \
             RETURNING {REVIEW_COLUMNS}"
        ))
        .bind(rid)
        .bind(user.id as i32)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())
    }
    .await;
    let r = match finished {
        Ok(Some(r)) => match tx.commit().await {
            Ok(()) => r,
            Err(e) => {
                ops.compensate(&state, doc_id, &back).await;
                return Err(ApiError::internal(e));
            }
        },
        Ok(None) => {
            drop(tx);
            ops.compensate(&state, doc_id, &back).await;
            return Err(ApiError::validation(json!({"status": ["Already undone."]})));
        }
        Err(e) => {
            drop(tx);
            ops.compensate(&state, doc_id, &back).await;
            return Err(ApiError::internal(e));
        }
    };
    ops.after_commit(&state, doc_id).await;
    Ok(Drf::ok(review_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/run/ – `{"documents": [ids], "mode": "suggest"|"apply"}` (manual run).
pub async fn run_view(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = auth.require_perms_by_method(&[("POST", &["documents.change_document"])])?;
    let b = body_json(&body)?;
    let requested = match b.get("mode").and_then(Value::as_str).unwrap_or("suggest") {
        "suggest" => AutoMode::Suggest,
        "apply" => AutoMode::Apply,
        _ => return Err(ApiError::validation(json!({"mode": ["\"mode\" must be \"suggest\" or \"apply\"."]}))),
    };
    // Security wave 3 M1: the admin's configuration also applies to manual runs
    let cfg = super::effective_config(&state).await?;
    let mode = manual_run_mode(requested, cfg.rs.auto_mode).map_err(ApiError::permission_denied_msg)?;
    let ids: Vec<i64> = match b.get("documents").and_then(Value::as_array) {
        Some(a) if a.len() > MANUAL_RUN_MAX_DOCUMENTS => {
            return Err(ApiError::validation(
                json!({"documents": [format!("Ensure this field has no more than {MANUAL_RUN_MAX_DOCUMENTS} elements.")]}),
            ));
        }
        Some(a) if !a.is_empty() => a.iter().filter_map(Value::as_i64).collect(),
        _ => return Err(ApiError::validation(json!({"documents": ["This field is required."]}))),
    };
    let mut allowed = Vec::new();
    for id in &ids {
        let Some(d) = data::load_doc(state.db(), *id).await? else { continue };
        if has_perms_owner_aware(state.db(), Some(user), "change_document", &DOCUMENT, ObjRef { id: d.root_id, owner_id: d.root_owner_id })
            .await?
        {
            allowed.push(*id);
        }
    }
    if allowed.len() != ids.len() {
        return Err(ApiError::permission_denied_msg("Insufficient permissions for some documents."));
    }
    // Security wave 3 M1: quota per user and hour (LLM costs/DoS)
    let n = u32::try_from(allowed.len().min(cfg.rs.auto_max_documents_per_run as usize)).unwrap_or(u32::MAX);
    if let Err(wait) = run_budget().try_take(&format!("u{}", user.id), n) {
        return Err(ApiError::throttled(Some(wait as f64)));
    }
    let v = run(&state, &allowed, mode, "manual", Some(user)).await?;
    Ok(Drf::ok(v).indent(ctx.negotiation.indent).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AI-N3: the review list no longer binds an id list but checks visibility via a
    /// subquery; filters on status/document are kept.
    #[test]
    fn review_list_uses_subquery() {
        let user = user(5, &[], false, true);
        let q = review_list_query(&user, Some("pending"), Some(7));
        let (sql, values) = q.build_sqlx(sea_query::PostgresQueryBuilder);
        assert!(sql.contains(r#""document_id" IN (SELECT"#), "{sql}");
        assert!(!sql.contains("ANY("), "{sql}");
        assert!(sql.contains(r#""status" = "#) && sql.contains(r#""document_id" = "#), "{sql}");
        assert!(values.0.0.len() < 20, "keine ID-Liste gebunden");
        let (sql, _) = review_list_query(&user, None, Some(i64::MAX)).build_sqlx(sea_query::PostgresQueryBuilder);
        assert!(sql.contains("FALSE"), "{sql}");
    }

    fn user(id: i64, perms: &[&str], superuser: bool, active: bool) -> User {
        User {
            id,
            username: format!("u{id}"),
            password: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: active,
            is_staff: false,
            is_superuser: superuser,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: Vec::new(),
            permissions: perms.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn change(field: Field, existing: Option<i64>) -> PlannedChange {
        PlannedChange {
            field,
            existing_id: existing,
            new_name: None,
            value: None,
            confidence: 0.95,
            apply: true,
            why: String::new(),
            data: None,
        }
    }

    /// Security wave 3 M1: `mode=apply` only with `PAPERRUST_AI_AUTO_MODE=apply`, `off` blocks the
    /// manual run entirely; quota per user.
    #[test]
    fn manual_run_respects_auto_mode() {
        use AutoMode::*;
        assert!(manual_run_mode(Apply, Off).is_err());
        assert_eq!(manual_run_mode(Suggest, Off), Ok(Suggest));
        assert!(manual_run_mode(Off, Off).is_err());
        assert!(manual_run_mode(Apply, Suggest).is_err());
        assert_eq!(manual_run_mode(Suggest, Suggest), Ok(Suggest));
        assert_eq!(manual_run_mode(Apply, Apply), Ok(Apply));
        assert_eq!(manual_run_mode(Suggest, Apply), Ok(Suggest));
        assert_eq!(MANUAL_RUN_MAX_DOCUMENTS, 10);
        assert_eq!(run_documents_per_hour(), 100, "Standard ohne Umgebungsvariable");
        let b = run_budget();
        assert!(b.try_take("u-test-m1", 100).is_ok());
        assert!(b.try_take("u-test-m1", 1).is_err(), "101. Dokument in der Stunde → 429");
        assert!(b.try_take("u-test-m1-other", 10).is_ok(), "je Nutzer getrennt");
    }

    /// Security wave 3 M2: creation only with the `add_<model>` permission of the trigger/owner.
    #[test]
    fn create_needs_add_permission() {
        let none = user(1, &["documents.change_document"], false, true);
        let tagger = user(2, &["documents.add_tag"], false, true);
        let admin = user(3, &[], true, true);
        let inactive_admin = user(4, &[], true, false);
        assert!(!may_create(None, Cat::Tags), "systemausgelöst ohne Eigentümer: nie");
        assert!(!may_create(Some(&none), Cat::Tags));
        assert!(may_create(Some(&tagger), Cat::Tags));
        assert!(!may_create(Some(&tagger), Cat::Correspondents));
        assert!(!may_create(Some(&tagger), Cat::DocumentTypes));
        assert!(may_create(Some(&admin), Cat::Correspondents));
        assert!(!may_create(Some(&inactive_admin), Cat::Tags));
        assert!(!may_create(Some(&admin), Cat::StoragePaths), "nie Speicherpfade");
    }

    /// Security wave 3 AI-N2: the assigned existing objects are collected per kind
    /// (basis of the visibility check in `apply`).
    #[test]
    fn assigned_ids_grouped() {
        let plan = vec![
            change(Field::Tags, Some(5)),
            change(Field::Tags, Some(6)),
            change(Field::Correspondent, Some(7)),
            change(Field::Title, None),
            change(Field::Tags, Some(5)),
            change(Field::DocumentType, Some(9)),
        ];
        let got = assigned_ids(&plan, &[0, 1, 2, 3, 4, 99]);
        assert_eq!(got, vec![(Cat::Tags, vec![5, 6]), (Cat::Correspondents, vec![7])]);
    }

    /// Status from applied/dismissed individual suggestions; "Edit" validates values per field.
    #[test]
    fn per_field_decisions() {
        let p = |a: bool, d: bool| json!({"applied": a, "dismissed": d});
        assert_eq!(review_status(&[p(false, false), p(true, false)]), "partial");
        assert_eq!(review_status(&[p(false, true), p(true, false)]), "applied");
        assert_eq!(review_status(&[p(false, true), p(false, true)]), "rejected");
        assert_eq!(review_status(&[p(false, true), p(false, false)]), "pending");
        let defs = vec![crate::custom_fields::FieldDef { id: 9, name: "Betrag".into(), data_type: "monetary".into(), extra_data: None }];
        let mut c = change(Field::Title, None);
        assert!(apply_override(&mut c, &json!({"value": "  "}), &defs).is_err());
        assert!(apply_override(&mut c, &json!({"value": " Neuer Titel "}), &defs).is_ok());
        assert_eq!(c.value.as_deref(), Some("Neuer Titel"));
        assert_eq!(c.confidence, 1.0);
        let mut c = change(Field::Created, None);
        assert!(apply_override(&mut c, &json!({"value": "31.12.2024"}), &defs).is_ok());
        assert_eq!(c.value.as_deref(), Some("2024-12-31"));
        let mut c = change(Field::CustomFields, Some(9));
        assert!(apply_override(&mut c, &json!({"value": "€ 5"}), &defs).is_ok());
        assert_eq!(c.data, Some(json!("EUR5.00")));
        assert!(apply_override(&mut c, &json!({"value": "viel"}), &defs).is_err());
        let mut c = PlannedChange { new_name: Some("Neu".into()), ..change(Field::Correspondent, None) };
        assert!(apply_override(&mut c, &json!({"existing_id": 4}), &defs).is_ok());
        assert_eq!((c.existing_id, c.new_name.clone()), (Some(4), None));
        assert_eq!(assigned_ids(&[c], &[0]), vec![(Cat::Correspondents, vec![4])], "geänderte Objekte werden auf Sichtbarkeit geprüft");
    }

    /// Finding 6: two partial acceptances are both undone (newest first); the target is the
    /// state before the first one. Before the fix only the last audit entry was reverted.
    #[test]
    fn undo_reverts_every_partial_acceptance() {
        let a1 = (json!({"title": "Scan 1", "tags": [1]}), json!({"title": "Invoice", "tags": [1, 5]}));
        let a2 =
            (json!({"correspondent": null, "custom_fields": {"9": null}}), json!({"correspondent": 7, "custom_fields": {"9": "EUR5.00"}}));
        let now = json!({"title": "Invoice", "tags": [1, 5], "correspondent": 7, "custom_fields": {"9": "EUR5.00"}});
        let target = plan_undo(&now, &[a1.clone(), a2.clone()]).unwrap();
        assert_eq!(target, json!({"title": "Scan 1", "tags": [1], "correspondent": null, "custom_fields": {"9": null}}));
        let dts = std::collections::BTreeMap::from([(9, "monetary".to_string())]);
        let w = restore_writes(&target, &now, &dts);
        assert_eq!(w.ch.title.as_deref(), Some("Scan 1"));
        assert_eq!(w.ch.correspondent_id, Some(None));
        assert_eq!(w.remove_tags, vec![5]);
        assert!(w.add_tags.is_empty());
        assert_eq!(w.cf_remove, vec![9]);
        // the inverse brings everything back (compensation of a failed undo)
        let back = restore_writes(&now, &target, &dts);
        assert_eq!(back.ch.title.as_deref(), Some("Invoice"));
        assert_eq!(back.add_tags, vec![5]);
        assert_eq!(back.cf_set, vec![(9, "monetary".to_string(), json!("EUR5.00"))]);
        // conflict in the older entry is detected too
        let changed = json!({"title": "User title", "tags": [1, 5], "correspondent": 7, "custom_fields": {"9": "EUR5.00"}});
        assert_eq!(plan_undo(&changed, &[a1, a2]).unwrap_err(), vec!["title".to_string()]);
    }

    /// Finding 7: only field names of an audit snapshot are logged, never values.
    #[test]
    fn audit_log_has_no_values() {
        let after = json!({"title": "Secret salary letter", "tags": [3], "custom_fields": {"9": "EUR 120000.00"}});
        let names = audit_field_names(&after);
        assert_eq!(names, vec!["title", "tags", "custom_fields.9"]);
        let joined = names.join(",");
        assert!(!joined.contains("Secret") && !joined.contains("120000"), "{joined}");
    }

    /// Fake non-transactional writer (like the worker's writer before it supports transactions):
    /// records calls, fails on the configured operation. Never touches the database.
    #[derive(Default)]
    struct FakeWriter {
        calls: std::sync::Mutex<Vec<String>>,
        fail_on: &'static str,
    }

    impl FakeWriter {
        fn record(&self, op: String) -> Result<(), String> {
            let fail = op.starts_with(self.fail_on) && !self.fail_on.is_empty();
            if let Ok(mut c) = self.calls.lock() {
                c.push(op);
            }
            if fail { Err("simulated failure".into()) } else { Ok(()) }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl DocWriter for FakeWriter {
        fn save<'a>(&'a self, _s: &'a AppState, _d: i64, c: &'a DocChanges) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move { self.record(format!("save:{}", c.title.clone().unwrap_or_default())) })
        }
        fn add_nested_tags<'a>(&'a self, _s: &'a AppState, _d: i64, t: &'a [i64]) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move { self.record(format!("add:{t:?}")) })
        }
        fn remove_tags<'a>(&'a self, _s: &'a AppState, _d: i64, t: &'a [i64]) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move { self.record(format!("remove:{t:?}")) })
        }
        fn get_or_create<'a>(
            &'a self,
            _s: &'a AppState,
            _c: Cat,
            _n: &'a str,
            _o: Option<i64>,
        ) -> futures::future::BoxFuture<'a, Result<(i64, bool), String>> {
            Box::pin(async { Ok((1, false)) })
        }
        fn set_custom_field<'a>(
            &'a self,
            _s: &'a AppState,
            _d: i64,
            f: i64,
            _dt: &'a str,
            v: &'a Value,
        ) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move { self.record(format!("setcf:{f}={v}")) })
        }
        fn remove_custom_field<'a>(&'a self, _s: &'a AppState, _d: i64, f: i64) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move { self.record(format!("rmcf:{f}")) })
        }
    }

    /// State without database (lazy pool on port 1, never connected).
    fn offline_state() -> AppState {
        let env = pl_core::config::Env::from_pairs([
            ("PAPERLESS_DBHOST", "127.0.0.1"),
            ("PAPERLESS_DBPORT", "1"),
            ("PAPERLESS_SECRET_KEY", "test-only-secret-key-for-pl-ai-review"),
        ]);
        AppState::new(pl_core::Config::from_env_map(env).unwrap()).unwrap()
    }

    /// Finding 8: if adding the replacement tag fails, the trigger tag is not removed and the
    /// error is returned (before the fix the trigger tag was removed first and the error dropped).
    #[tokio::test]
    async fn trigger_tag_kept_when_replacement_fails() {
        let state = offline_state();
        let fake = std::sync::Arc::new(FakeWriter { fail_on: "add", ..Default::default() });
        let ops = Ops { w: fake.clone(), atomic: false };
        let res = replace_trigger_tag(&state, &ops, 42, 7, Some(8)).await;
        assert!(res.is_err());
        assert_eq!(fake.calls(), vec!["add:[8]".to_string()], "trigger tag 7 must stay");
        let ok = std::sync::Arc::new(FakeWriter::default());
        let ops = Ops { w: ok.clone(), atomic: false };
        assert!(replace_trigger_tag(&state, &ops, 42, 7, Some(8)).await.is_ok());
        assert_eq!(ok.calls(), vec!["add:[8]".to_string(), "remove:[7]".to_string()]);
        let rm = std::sync::Arc::new(FakeWriter { fail_on: "remove", ..Default::default() });
        let ops = Ops { w: rm.clone(), atomic: false };
        assert!(replace_trigger_tag(&state, &ops, 42, 7, None).await.is_err(), "remove errors are returned too");
    }

    /// Finding 5 (non-transactional writer): a failure after the first write restores the earlier
    /// values and returns the error – the title does not stay changed.
    #[tokio::test]
    async fn partial_write_is_compensated() {
        let state = offline_state();
        let fake = std::sync::Arc::new(FakeWriter { fail_on: "setcf:9", ..Default::default() });
        let ops = Ops { w: fake.clone(), atomic: false };
        let w = Writes {
            ch: DocChanges { title: Some("AI title".into()), ..Default::default() },
            add_tags: vec![5],
            cf_set: vec![(9, "monetary".into(), json!("EUR5.00"))],
            ..Default::default()
        };
        let revert = Writes {
            ch: DocChanges { title: Some("Old title".into()), ..Default::default() },
            remove_tags: vec![5],
            cf_remove: vec![9],
            ..Default::default()
        };
        let res = ops.execute(&state, None, 42, &w, Some(&revert)).await;
        assert!(res.is_err());
        assert_eq!(
            fake.calls(),
            vec!["save:AI title", "add:[5]", "setcf:9=\"EUR5.00\"", "save:Old title", "remove:[5]", "rmcf:9"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        // a transactional writer is never driven without the caller's transaction
        let ops = Ops { w: std::sync::Arc::new(workflow::SqlDocWriter), atomic: true };
        assert!(ops.execute(&state, None, 42, &w, None).await.is_err());
        assert!(workflow::SqlDocWriter.supports_transaction());
        assert!(!FakeWriter::default().supports_transaction());
    }

    /// Security wave 3 AI-N3: `page_size` at most 100, no overflow with a huge `page`.
    #[test]
    fn pagination_bounded() {
        assert_eq!(page_window(None, None), Some((25, 0)));
        assert_eq!(page_window(Some(3), Some(100_000)), Some((100, 200)));
        assert_eq!(page_window(Some(-5), Some(0)), Some((1, 0)));
        assert_eq!(page_window(Some(i64::MAX), Some(100)), None);
        assert_eq!(page_window(Some(i64::MAX / 100 + 1), Some(100)), Some((100, (i64::MAX / 100) * 100)));
    }
}
