// Bundled follow-up of `bulk_update_documents` (docs/PERF-SCALE.md §5 #10a/c, §10).
//
// upstream: src/documents/tasks.py@v3.4.2 (bulk_update_documents), src/documents/signals/handlers.py
//           @v3.4.2 (run_workflows_updated, send_websocket_document_updated,
//           update_filename_and_move_files). The result (DB state, files, file names,
//           workflow runs, order of the file moves) is the same as with the single path
//           [`Consumer::bulk_update_signals_single`]; the only difference is *how often* things are read:
//
// * Load "document updated" workflows once per task instead of per document.
// * Load documents, names (correspondent, type, storage path, owner), tags, latest versions and
//   custom fields in blocks of [`BLOCK`] documents with one query each.
// * Workflow check in memory. Only from the first matching workflow on does the document take the
//   single path (`run_workflow_list` from that workflow on) – until then upstream changes nothing.
// * Render the file name in memory; if the template yields the same name (most common case, e.g. on
//   tag changes), only `modified = now()` remains, collected in one statement per block. Otherwise
//   single path under the media lock in the same order as upstream (`-created`).
// * Load tags/custom fields for the template context only if one of the affected templates
//   mentions them at all (#10c).
// * Delete cache keys per block in one call; load WebSocket grantees per block.

use std::collections::{HashMap, HashSet};

use pl_matching::workflow::{ObjRef, TRIGGER_DOCUMENT_UPDATED};
use pl_templating::{DocumentContext, NamedRef, OwnerRef, StoragePathRef, TagRef};
use sqlx::{PgConnection, Row};

use crate::consumer::Consumer;
use crate::db::{self, DocRow};
use crate::docops::DocInstance;
use crate::workflows::{self, CustomFieldQueries, RunOutcome, WorkflowDef};

/// Documents per load block (one query per kind and block).
pub const BLOCK: usize = 500;

/// Counters of a run (log/measurement).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BulkStats {
    pub documents: usize,
    /// Documents that at least one workflow matched (single path).
    pub workflow_matched: usize,
    /// File name unchanged → only `modified` (collected).
    pub filename_unchanged: usize,
    /// Single path `update_filename_and_move_files` (move, collision, version, error).
    pub filename_slow: usize,
    /// Of those, carried forward in memory after a workflow hit (one row write at the end).
    pub workflow_deferred: usize,
    /// Intermediate moves that were dropped (the file name changed in the middle of the
    /// workflow chain; the move happens only once to the final location).
    pub moves_skipped: usize,
}

/// Result of the file name check in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileStep {
    /// No file name (nothing to do).
    NoFile,
    /// Template yields the same name → upstream only `modified`.
    Unchanged,
    /// Template yields a different name → file would be moved.
    Changed,
    /// Not decidable in memory (context incomplete) → exact path.
    Unknown,
}

/// Rounds of the follow-up: `docs` (order `-created`, without duplicates) per round `r` (0-based)
/// filtered to documents that appear more than `r` times in `requested`. Without workflows exactly one
/// round over all `docs` (see [`Consumer::bulk_update_signals`]).
pub fn rounds(docs: &[i64], requested: &[i64], with_workflows: bool) -> Vec<Vec<i64>> {
    if !with_workflows {
        return vec![docs.to_vec()];
    }
    let mut count: HashMap<i64, usize> = HashMap::new();
    for id in requested {
        *count.entry(*id).or_default() += 1;
    }
    let max = docs.iter().map(|d| count.get(d).copied().unwrap_or(1)).max().unwrap_or(1);
    (0..max).map(|r| docs.iter().copied().filter(|d| count.get(d).copied().unwrap_or(1) > r).collect()).collect()
}

fn ids32(ids: &[i64]) -> Vec<i32> {
    ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect()
}

async fn names(conn: &mut PgConnection, table: &str, col: &str, ids: &HashSet<i64>) -> Result<HashMap<i64, String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let v: Vec<i64> = ids.iter().copied().collect();
    let sql = format!("SELECT id, {col} FROM {table} WHERE id = ANY($1)");
    let rows = sqlx::query(&sql).bind(ids32(&v)).fetch_all(&mut *conn).await?;
    rows.iter().map(|r| Ok((i64::from(r.try_get::<i32, _>(0)?), r.try_get::<String, _>(1)?))).collect()
}

/// What a template needs (#10c): load tags/custom fields only if an affected template
/// *can* mention them. Conservative via the template text: `tag` (tag_list, tag_name_list, tags) or
/// `custom_field` (custom_fields, get_cf_value via custom_fields) or `document` (dynamic
/// access via `document.…`) → load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TemplateNeeds {
    pub tags: bool,
    pub custom_fields: bool,
}

impl TemplateNeeds {
    pub fn of(template: &str) -> Self {
        let doc = template.contains("document");
        TemplateNeeds { tags: doc || template.contains("tag"), custom_fields: doc || template.contains("custom_field") }
    }

    fn or(self, o: Self) -> Self {
        TemplateNeeds { tags: self.tags || o.tags, custom_fields: self.custom_fields || o.custom_fields }
    }
}

/// Preloaded state of a block.
struct BlockData {
    rows: HashMap<i64, DocRow>,
    correspondents: HashMap<i64, String>,
    doctypes: HashMap<i64, String>,
    storage_paths: HashMap<i64, (String, String)>,
    users: HashMap<i64, String>,
    tags: HashMap<i64, Vec<(i64, String)>>,
    /// Content of the latest (non-deleted) version per root document with versions.
    latest_version_content: HashMap<i64, String>,
    custom_fields: HashMap<i64, Vec<pl_templating::CustomFieldInstance>>,
    /// Custom fields were loaded (otherwise no template of the block needs them).
    custom_fields_loaded: bool,
    cfq: CustomFieldQueries,
}

impl BlockData {
    async fn load(
        conn: &mut PgConnection,
        ids: &[i64],
        wfs: &[WorkflowDef],
        filename_format: Option<&str>,
        tz: chrono_tz::Tz,
    ) -> Result<Self, sqlx::Error> {
        let rows: HashMap<i64, DocRow> = db::load_documents(conn, ids).await?.into_iter().map(|r| (r.id, r)).collect();
        let mut corr = HashSet::new();
        let mut dt = HashSet::new();
        let mut sp = HashSet::new();
        let mut users = HashSet::new();
        for r in rows.values() {
            corr.extend(r.correspondent_id);
            dt.extend(r.document_type_id);
            sp.extend(r.storage_path_id);
            users.extend(r.owner_id);
        }
        let correspondents = names(conn, "documents_correspondent", "name", &corr).await?;
        let doctypes = names(conn, "documents_documenttype", "name", &dt).await?;
        let mut storage_paths: HashMap<i64, (String, String)> = HashMap::new();
        if !sp.is_empty() {
            let v: Vec<i64> = sp.iter().copied().collect();
            for r in sqlx::query("SELECT id, name, path FROM documents_storagepath WHERE id = ANY($1)")
                .bind(ids32(&v))
                .fetch_all(&mut *conn)
                .await?
            {
                storage_paths.insert(i64::from(r.try_get::<i32, _>(0)?), (r.try_get(1)?, r.try_get(2)?));
            }
        }
        let users = names(conn, "auth_user", "username", &users).await?;

        // Template need: global format for documents without a storage path, otherwise path of the storage path
        let mut needs = TemplateNeeds::default();
        if rows.values().any(|r| r.storage_path_id.is_none_or(|i| !storage_paths.contains_key(&i))) {
            needs = needs.or(filename_format.map(TemplateNeeds::of).unwrap_or_default());
        }
        for (_, path) in storage_paths.values() {
            needs = needs.or(TemplateNeeds::of(path));
        }
        let need_tags = needs.tags || !wfs.is_empty();

        let ids32v = ids32(ids);
        let mut tags: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
        if need_tags {
            // same order as db::document_tags (t.name, t.id) per document
            for r in sqlx::query(
                "SELECT dt.document_id, t.id, t.name FROM documents_tag t \
                 JOIN documents_document_tags dt ON dt.tag_id = t.id \
                 WHERE dt.document_id = ANY($1) ORDER BY dt.document_id, t.name, t.id",
            )
            .bind(&ids32v)
            .fetch_all(&mut *conn)
            .await?
            {
                tags.entry(i64::from(r.try_get::<i32, _>(0)?)).or_default().push((i64::from(r.try_get::<i32, _>(1)?), r.try_get(2)?));
            }
        }
        // get_effective_content(): latest version (version_index DESC, id DESC like effective_content)
        let mut latest_version_content = HashMap::new();
        for r in sqlx::query(
            "SELECT DISTINCT ON (root_document_id) root_document_id, content FROM documents_document \
             WHERE root_document_id = ANY($1) AND deleted_at IS NULL \
             ORDER BY root_document_id, version_index DESC, id DESC",
        )
        .bind(&ids32v)
        .fetch_all(&mut *conn)
        .await?
        {
            latest_version_content.insert(i64::from(r.try_get::<i32, _>(0)?), r.try_get::<String, _>(1)?);
        }
        let mut custom_fields: HashMap<i64, Vec<pl_templating::CustomFieldInstance>> = HashMap::new();
        if needs.custom_fields {
            // same columns/order as templating::custom_field_instances
            for r in sqlx::query(
                "SELECT i.value_text, i.value_bool, i.value_url, i.value_date, i.value_int, i.value_float, \
                        i.value_monetary, i.value_document_ids, i.value_select, i.value_long_text, \
                        f.name, f.data_type, f.extra_data, i.document_id \
                 FROM documents_customfieldinstance i JOIN documents_customfield f ON f.id = i.field_id \
                 WHERE i.document_id = ANY($1) ORDER BY i.document_id, i.created, i.id",
            )
            .bind(&ids32v)
            .fetch_all(&mut *conn)
            .await?
            {
                let doc = i64::from(r.try_get::<i32, _>(13)?);
                custom_fields.entry(doc).or_default().push(crate::templating::cf_instance_from_row(&r)?);
            }
        }
        let selected: Vec<&pl_matching::workflow::WorkflowTrigger> =
            wfs.iter().flat_map(|w| w.triggers.iter()).filter(|t| t.trigger_type == TRIGGER_DOCUMENT_UPDATED).collect();
        let cfq = workflows::eval_custom_field_queries(conn, &selected, ids, tz).await?;
        Ok(BlockData {
            rows,
            correspondents,
            doctypes,
            storage_paths,
            users,
            tags,
            latest_version_content,
            custom_fields,
            custom_fields_loaded: needs.custom_fields,
            cfq,
        })
    }

    /// Reload names for (newly assigned) correspondents/types/storage paths/owners.
    async fn ensure_names(&mut self, conn: &mut PgConnection, r: &DocRow) -> Result<(), sqlx::Error> {
        if let Some(c) = r.correspondent_id.filter(|c| !self.correspondents.contains_key(c))
            && let Some(n) = db::object_name(conn, "documents_correspondent", c).await?
        {
            self.correspondents.insert(c, n);
        }
        if let Some(c) = r.document_type_id.filter(|c| !self.doctypes.contains_key(c))
            && let Some(n) = db::object_name(conn, "documents_documenttype", c).await?
        {
            self.doctypes.insert(c, n);
        }
        if let Some(c) = r.storage_path_id.filter(|c| !self.storage_paths.contains_key(c))
            && let Some(row) =
                sqlx::query("SELECT name, path FROM documents_storagepath WHERE id = $1").bind(c as i32).fetch_optional(&mut *conn).await?
        {
            self.storage_paths.insert(c, (row.try_get(0)?, row.try_get(1)?));
        }
        if let Some(u) = r.owner_id.filter(|u| !self.users.contains_key(u))
            && let Some(n) = db::username(conn, u).await?
        {
            self.users.insert(u, n);
        }
        Ok(())
    }

    /// Can the template context for `r` be built completely from what was loaded?
    fn context_complete(&self, r: &DocRow, s: &crate::settings::ConsumerSettings) -> bool {
        let template = match r.storage_path_id.and_then(|i| self.storage_paths.get(&i)) {
            Some((_, p)) => Some(p.as_str()),
            None => s.templating.filename_format.as_deref(),
        };
        self.custom_fields_loaded || !template.map(TemplateNeeds::of).unwrap_or_default().custom_fields
    }

    fn obj(map: &HashMap<i64, String>, id: Option<i64>) -> Option<ObjRef> {
        id.and_then(|i| map.get(&i).map(|n| ObjRef { id: i, name: n.clone() }))
    }

    fn existing(&self, r: &DocRow) -> pl_matching::workflow::ExistingDocument {
        let sp = r.storage_path_id.and_then(|i| self.storage_paths.get(&i).map(|(n, _)| ObjRef { id: i, name: n.clone() }));
        let tags = self.tags.get(&r.id).map(|t| t.iter().map(|(id, name)| ObjRef { id: *id, name: name.clone() }).collect());
        workflows::existing_from_parts(
            r,
            Self::obj(&self.correspondents, r.correspondent_id),
            Self::obj(&self.doctypes, r.document_type_id),
            sp,
            tags.unwrap_or_default(),
            self.latest_version_content.get(&r.id).cloned().unwrap_or_else(|| r.content.clone()),
        )
    }

    /// Template context like `templating::document_context` (root documents only).
    fn context(&self, r: &DocRow) -> DocumentContext {
        let mut c = crate::templating::base_context(r);
        let named =
            |m: &HashMap<i64, String>, id: Option<i64>| id.and_then(|i| m.get(&i).map(|n| NamedRef { id: Some(i), name: n.clone() }));
        c.correspondent = named(&self.correspondents, r.correspondent_id);
        c.document_type = named(&self.doctypes, r.document_type_id);
        c.storage_path = r
            .storage_path_id
            .and_then(|i| self.storage_paths.get(&i).map(|(n, p)| StoragePathRef { id: Some(i), name: n.clone(), path: p.clone() }));
        c.owner = r.owner_id.and_then(|o| self.users.get(&o).map(|u| OwnerRef { id: o, username: u.clone() }));
        c.tags = self
            .tags
            .get(&r.id)
            .map(|t| t.iter().map(|(id, name)| TagRef { id: Some(*id), name: name.clone() }).collect())
            .unwrap_or_default();
        c.custom_fields = self.custom_fields.get(&r.id).cloned().unwrap_or_default();
        c
    }
}

/// `get_permission_grantees(Document, pks)[pk]["view"]` for many documents.
/// Document → (users, groups) with `view_document`.
type Grantees = HashMap<i64, (Vec<i64>, Vec<i64>)>;

async fn view_grantees_many(conn: &mut PgConnection, ids: &[i64]) -> Result<Grantees, sqlx::Error> {
    let pks: Vec<String> = ids.iter().map(i64::to_string).collect();
    let mut out: Grantees = HashMap::new();
    for (table, col, users) in [("guardian_userobjectpermission", "user_id", true), ("guardian_groupobjectpermission", "group_id", false)] {
        let sql = format!(
            "SELECT DISTINCT o.object_pk, o.{col} FROM {table} o \
             JOIN auth_permission p ON p.id = o.permission_id \
             JOIN django_content_type c ON c.id = o.content_type_id \
             WHERE o.object_pk = ANY($1) AND p.codename = 'view_document' \
               AND c.app_label = 'documents' AND c.model = 'document' ORDER BY 1, 2"
        );
        for r in sqlx::query(&sql).bind(&pks).fetch_all(&mut *conn).await? {
            let pk: String = r.try_get(0)?;
            let Ok(doc) = pk.parse::<i64>() else { continue };
            let who = i64::from(r.try_get::<i32, _>(1)?);
            let e = out.entry(doc).or_default();
            if users { e.0.push(who) } else { e.1.push(who) }
        }
    }
    Ok(out)
}

impl Consumer {
    /// Bundled signal part of `bulk_update_documents(document_ids)` (see module header).
    /// Same effect as [`Consumer::bulk_update_signals_single`].
    ///
    /// **Coalesced tasks (FINDINGS B6):** A not yet started task can get the ids
    /// of further bulk changes appended (`pl_documents_write::coalesce`); a document
    /// then appears `k` times in `document_ids` – as often as upstream's own tasks would have run.
    /// If there are "document updated" workflows, the follow-up runs for them in `k` rounds (round `r`
    /// over all documents with at least `r` entries): workflows, WebSocket and file name per
    /// document thus exactly as often as with `k` individual tasks. Without such workflows a
    /// repetition has no effect (file name/index depend only on the final state) → one round.
    pub async fn bulk_update_signals(&self, document_ids: &[i64]) -> Result<BulkStats, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let docs: Vec<i64> = sqlx::query_scalar::<_, i32>(
            "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id",
        )
        .bind(ids32(document_ids))
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(i64::from)
        .collect();
        let wfs = workflows::workflows_for_trigger(&mut conn, TRIGGER_DOCUMENT_UPDATED).await?;
        let mut stats = BulkStats { documents: docs.len(), ..Default::default() };
        let rounds = rounds(&docs, document_ids, !wfs.is_empty());
        for round in &rounds {
            for block in round.chunks(BLOCK) {
                self.bulk_block(&mut conn, block, &wfs, &mut stats).await?;
            }
        }
        Ok(stats)
    }

    async fn bulk_block(
        &self,
        conn: &mut PgConnection,
        block: &[i64],
        wfs: &[WorkflowDef],
        stats: &mut BulkStats,
    ) -> Result<(), sqlx::Error> {
        let s = self.settings.as_ref();
        let pre = BlockData::load(conn, block, wfs, s.templating.filename_format.as_deref(), s.time_zone).await?;
        self.caches.clear_many(block).await;
        let grantees = if self.events.is_some() { Some(view_grantees_many(conn, block).await?) } else { None };
        let audit = crate::audit::AuditLog { enabled: s.audit_log_enabled, actor_id: None };
        let mut touch_modified: Vec<i64> = Vec::new();
        let mut pre = pre;
        for &id in block {
            // Deleted/vanished between selection and block: upstream keeps iterating the already
            // evaluated QuerySet; document_updated reloads and finds nothing.
            let Some(row) = pre.rows.get(&id).cloned() else { continue };
            let row = &row;
            // document_updated → run_workflows_updated
            let mut matched = false;
            if !wfs.is_empty() && row.root_document_id.is_none() && row.deleted_at.is_none() {
                let ex = pre.existing(row);
                if let Some(k) = workflows::first_matching_workflow(wfs, TRIGGER_DOCUMENT_UPDATED, &ex, &pre.cfq) {
                    matched = true;
                    stats.workflow_matched += 1;
                    if workflows::is_simple_workflow(&wfs[k]) {
                        stats.workflow_deferred += 1;
                        self.deferred_chain(conn, &mut pre, row, &wfs[k..], &audit, grantees.as_ref(), stats).await?;
                        continue;
                    }
                    let mut doc = DocInstance::from_db(row.clone());
                    let outcome: RunOutcome =
                        workflows::run_workflow_list(conn, &self.run_env(&audit), TRIGGER_DOCUMENT_UPDATED, &mut doc, &wfs[k..]).await?;
                    self.send_websocket_document_updated(conn, id).await?;
                    workflows::enqueue_ai_suggestions(self.queue.as_deref(), &outcome).await;
                }
            } else if row.root_document_id.is_some() {
                tracing::debug!(target: "paperless.handlers", "Skipping workflow execution for version document {id}");
            }
            if !matched {
                // send_websocket_document_updated with the (unchanged) state
                self.websocket_from(row, grantees.as_ref());
            }
            // post_save → update_filename_and_move_files
            if matched || row.root_document_id.is_some() {
                stats.filename_slow += 1;
                crate::files::update_filename_and_move_files(conn, s, id).await?;
                continue;
            }
            if row.filename.as_deref().is_none_or(str::is_empty) {
                continue; // upstream: `if not instance.filename: return`
            }
            let ctx = pre.context(row);
            if crate::files::filename_unchanged(s, row, &ctx) {
                stats.filename_unchanged += 1;
                touch_modified.push(id);
            } else {
                stats.filename_slow += 1;
                let has_versions = pre.latest_version_content.contains_key(&id);
                crate::files::update_filename_with_context(conn, s, row, &ctx, has_versions).await?;
            }
        }
        if !touch_modified.is_empty() {
            // Document.objects.filter(pk=…).update(modified=timezone.now()) – one statement per block,
            // locks in id order (FINDINGS B4); autocommit → simply retry on deadlock.
            let mut attempt = 1;
            loop {
                match db::touch_modified_sorted(conn, &touch_modified, db::now_micros()).await {
                    Ok(_) => break,
                    Err(e) if db::is_retryable(&e) && attempt < db::MAX_ATTEMPTS => {
                        tracing::warn!(target: "paperless.tasks", attempt, "bulk_update_documents: modified update retried after {e}");
                        tokio::time::sleep(db::retry_backoff(attempt)).await;
                        attempt += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// `send_websocket_document_updated` with a known state and grantees loaded per block
    /// (workflows of the bundled path do not change permissions).
    fn websocket_from(&self, row: &DocRow, grantees: Option<&Grantees>) {
        if let (Some(bus), Some(g)) = (&self.events, grantees) {
            let (users, groups) = g.get(&row.id).cloned().unwrap_or_default();
            let modified = pl_core::datetime::format_datetime(&row.modified, &self.settings.time_zone);
            pl_queue::events::DocumentsStatusManager::new(bus.clone()).send_document_updated(
                row.id,
                modified,
                pl_queue::events::ViewPermissions { owner_id: row.owner_id, users_can_view: users, groups_can_view: groups },
            );
        }
    }

    /// `update_filename_and_move_files` on the carried-forward state: `Some(true)` = nothing to
    /// do except `modified`, `Some(false)` = no file name (upstream returns immediately),
    /// `None` = single path needed (move, collision, incomplete context).
    fn filename_step(&self, pre: &BlockData, stored: &DocRow) -> FileStep {
        if stored.filename.as_deref().is_none_or(str::is_empty) {
            return FileStep::NoFile;
        }
        if !pre.context_complete(stored, &self.settings) {
            return FileStep::Unknown;
        }
        if crate::files::filename_unchanged(&self.settings, stored, &pre.context(stored)) { FileStep::Unchanged } else { FileStep::Changed }
    }

    /// Exact single path from a workflow on (state is written): remaining workflows,
    /// WebSocket, AI tasks, follow-up file name.
    async fn exact_rest(
        &self,
        conn: &mut PgConnection,
        id: i64,
        wfs: &[WorkflowDef],
        audit: &crate::audit::AuditLog,
        stats: &mut BulkStats,
    ) -> Result<(), sqlx::Error> {
        let mut outcome = RunOutcome::default();
        if let Some(fresh) = db::load_document(conn, id).await? {
            let mut d = DocInstance::from_db(fresh);
            outcome = workflows::run_workflow_list(conn, &self.run_env(audit), TRIGGER_DOCUMENT_UPDATED, &mut d, wfs).await?;
        }
        self.send_websocket_document_updated(conn, id).await?;
        workflows::enqueue_ai_suggestions(self.queue.as_deref(), &outcome).await;
        stats.filename_slow += 1;
        crate::files::update_filename_and_move_files(conn, self.settings.as_ref(), id).await
    }

    /// Document with a workflow hit, all following actions [simple](workflows::is_simple_action):
    /// same steps as `run_workflow_list` + `post_save` + follow-up file name, but the
    /// document row (`save()` per workflow, `modified` per file name check) is carried forward in memory
    /// and written once at the end (every row version costs several ms because of the indexes
    /// on `documents_document`). Tags, audit entries and workflow runs are written immediately and in the same order as
    /// upstream.
    ///
    /// If a step in the middle of the chain would move the file, the chain only notes that
    /// and moves **once** to the final location at the end (upstream: after every workflow). The final name
    /// is the same: `generate_unique_filename` takes the first candidate that is the own old
    /// name or missing on disk – the own file never counts as a collision, files of
    /// other documents do not change within the chain. If a step cannot be decided in memory
    /// or a workflow with a different action matches, the state is written and from
    /// there the exact single path is taken (which then also moves directly to its location).
    #[allow(clippy::too_many_arguments)]
    async fn deferred_chain(
        &self,
        conn: &mut PgConnection,
        pre: &mut BlockData,
        row: &DocRow,
        wfs: &[WorkflowDef],
        audit: &crate::audit::AuditLog,
        grantees: Option<&Grantees>,
        stats: &mut BulkStats,
    ) -> Result<(), sqlx::Error> {
        const SAVE_FIELDS: &[&str] = &["title", "correspondent", "document_type", "storage_path", "owner", "modified"];
        let s = self.settings.as_ref();
        let id = row.id;
        let mut stored = row.clone();
        let mut dirty = false;
        // File should already have been moved (final location follows at the end)
        let mut pending_move = false;
        for (j, wf) in wfs.iter().enumerate() {
            // document.refresh_from_db()
            let mut doc = DocInstance::from_db(stored.clone());
            let before_wf = stored.clone();
            pre.ensure_names(conn, &stored).await?;
            let ex = pre.existing(&stored);
            // wfs[0] was already recognized as matching by the caller (same state)
            if j > 0 && workflows::first_matching_workflow(std::slice::from_ref(wf), TRIGGER_DOCUMENT_UPDATED, &ex, &pre.cfq).is_none() {
                continue;
            }
            if !workflows::is_simple_workflow(wf) {
                // matching workflow with a not-reproduced action: write state, exact from here on
                if dirty {
                    db::update_document_fields(conn, &stored, SAVE_FIELDS).await?;
                }
                return self.exact_rest(conn, id, &wfs[j..], audit, stats).await;
            }
            for action in &wf.actions {
                tracing::info!(target: "paperless.handlers", "Applying {} from Workflow: {}", action.display(), wf.name);
                workflows::apply_simple_action(conn, audit, action, &mut doc, &stored).await?;
                if !action.assign_tags.is_empty() || !action.remove_tags.is_empty() || action.remove_all_tags {
                    // Tags in model order (t.name, t.id) as in the single path
                    let t = db::document_tags(conn, id).await?;
                    pre.tags.insert(id, t);
                }
            }
            doc.row.title = doc.row.title.chars().take(128).collect();
            // document.save(update_fields=[…]): audit against the (assumed) DB state, modified=now
            doc.row.modified = db::now_micros();
            pre.ensure_names(conn, &doc.row).await?;
            let corr = doc.row.correspondent_id.and_then(|c| pre.correspondents.get(&c).cloned());
            audit.document_updated(conn, &stored, &doc.row, &doc.created_attr, Some(SAVE_FIELDS), corr.as_deref()).await?;
            stored = doc.row;
            dirty = true;
            // post_save → update_filename_and_move_files
            let step = self.filename_step(pre, &stored);
            match step {
                FileStep::Unchanged | FileStep::Changed => {
                    // upstream: moving (or only `modified`) sets `modified = now`
                    if step == FileStep::Changed {
                        if pending_move {
                            stats.moves_skipped += 1;
                        }
                        pending_move = true;
                    }
                    stored.modified = db::now_micros();
                }
                FileStep::NoFile => {}
                FileStep::Unknown => {
                    // exact from here on: write state, single path for the rest
                    db::update_document_fields(conn, &stored, SAVE_FIELDS).await?;
                    crate::files::update_filename_and_move_files(conn, s, id).await?;
                    workflows::record_run(conn, wf.id, TRIGGER_DOCUMENT_UPDATED, Some(id)).await?;
                    record_simple_run(conn, wf, id, &before_wf, &stored).await?;
                    return self.exact_rest(conn, id, &wfs[j + 1..], audit, stats).await;
                }
            }
            workflows::record_run(conn, wf.id, TRIGGER_DOCUMENT_UPDATED, Some(id)).await?;
            record_simple_run(conn, wf, id, &before_wf, &stored).await?;
        }
        // send_websocket_document_updated (refresh_from_db = carried-forward state)
        self.websocket_from(&stored, grantees);
        // bulk_update_documents: post_save → update_filename_and_move_files
        match self.filename_step(pre, &stored) {
            FileStep::Unchanged if !pending_move => {
                stats.filename_unchanged += 1;
                stored.modified = db::now_micros();
                dirty = true;
            }
            FileStep::NoFile => {}
            _ => {
                if dirty {
                    db::update_document_fields(conn, &stored, SAVE_FIELDS).await?;
                }
                stats.filename_slow += 1;
                return crate::files::update_filename_and_move_files(conn, s, id).await;
            }
        }
        if dirty {
            db::update_document_fields(conn, &stored, SAVE_FIELDS).await?;
        }
        Ok(())
    }
}

/// PaperRust history for a run reproduced in the bundled path (only workflows without
/// extensions, main branch; same entries as in the single path).
async fn record_simple_run(conn: &mut PgConnection, wf: &WorkflowDef, id: i64, before: &DocRow, after: &DocRow) -> Result<(), sqlx::Error> {
    let r = crate::workflow_ext::RunRecord {
        workflow_id: wf.id,
        trigger_type: TRIGGER_DOCUMENT_UPDATED,
        document_id: Some(id),
        result: crate::workflow_ext::RunResult::Ok,
        branch: Some(crate::workflow_ext::Branch::Main),
        code: "ok".into(),
        message: String::new(),
        changed_fields: crate::workflow_ext::changed_fields(before, Some(after), &wf.actions),
        source: None,
    };
    crate::workflow_ext::insert_run(conn, &r).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_needs() {
        assert_eq!(TemplateNeeds::of("{{ created_year }}/{{ correspondent }}/{{ title }}"), TemplateNeeds::default());
        assert!(TemplateNeeds::of("{{ tag_list }}").tags);
        assert!(!TemplateNeeds::of("{{ tag_list }}").custom_fields);
        assert!(TemplateNeeds::of("{% for t in tag_name_list %}{{ t }}{% endfor %}").tags);
        assert!(TemplateNeeds::of("{{ custom_fields|get_cf_value('x') }}").custom_fields);
        let d = TemplateNeeds::of("{{ document.title }}");
        assert!(d.tags && d.custom_fields);
        // old {} syntax
        assert!(TemplateNeeds::of("{correspondent}/{tag_list}").tags);
    }

    /// B6: coalesced tasks – workflow rounds per document as often as own tasks.
    #[test]
    fn rounds_follow_multiplicity() {
        let docs = [5, 3, 9];
        // without workflows: one round, even with multiple entries
        assert_eq!(rounds(&docs, &[3, 5, 9, 3, 5, 3], false), vec![vec![5, 3, 9]]);
        // with workflows: 3 three times, 5 twice, 9 once; order stays that of `docs`
        assert_eq!(rounds(&docs, &[3, 5, 9, 3, 5, 3], true), vec![vec![5, 3, 9], vec![5, 3], vec![3]]);
        // simple task: one round
        assert_eq!(rounds(&docs, &[9, 5, 3], true), vec![vec![5, 3, 9]]);
        assert_eq!(rounds(&[], &[], true), vec![Vec::<i64>::new()]);
    }
}
