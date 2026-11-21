// Workflows on ingestion: trigger "Consumption Started" (acts on the overrides) and
// "Document Added" (acts on the saved document).
//
// upstream: src/documents/signals/handlers.py@v3.4.2 (run_workflows), src/documents/workflows/
//           utils.py@v3.4.2 (get_workflows_for_trigger), src/documents/workflows/mutations.py@v3.4.2
//           (apply_assignment_to_overrides/_document, apply_removal_to_overrides/_document),
//           src/documents/workflows/actions.py@v3.4.2 (execute_move_to_trash_action; e-mail/webhook/
//           password → `WorkflowEffects`), src/documents/models.py@v3.4.2 (WorkflowAction.WorkflowActionType,
//           WorkflowRun), src/documents/matching.py@v3.4.2 (document_matches_workflow → pl-matching)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use pl_core::sqlbind::SqlxBinder as _;
use pl_matching::workflow::{
    CustomFieldQueryEvaluator, ExistingDocument, InvalidCustomFieldQuery, ObjRef, TRIGGER_CONSUMPTION, WorkflowDocument, WorkflowTrigger,
    document_matches_workflow,
};
use sea_query::{Expr, Order, PostgresQueryBuilder, Query};
use serde_json::Value;
use sqlx::{PgConnection, Row};

use crate::audit::AuditLog;
use crate::data::{ConsumableDocument, DocumentMetadataOverrides};
use crate::db::{self, a, col, now_micros};
use crate::docops::{self, DocInstance};
use crate::settings::ConsumerSettings;
use crate::stages::{ActionContext, ActionInput, BoxFuture, WorkflowEffects};
use crate::workflow_ext::{self, Decision, DeferredRun, ExtOptions, MatchState, RunRecord};

pub const ACTION_ASSIGNMENT: i64 = 1;
pub const ACTION_REMOVAL: i64 = 2;
pub const ACTION_EMAIL: i64 = 3;
pub const ACTION_WEBHOOK: i64 = 4;
pub const ACTION_PASSWORD_REMOVAL: i64 = 5;
pub const ACTION_MOVE_TO_TRASH: i64 = 6;
pub const ACTION_REMOTE_OCR: i64 = 7;
pub const ACTION_APPLY_AI_SUGGESTIONS: i64 = 8;

/// `WorkflowAction` with all relations.
#[derive(Debug, Clone, Default)]
pub struct ActionDef {
    pub id: i64,
    pub action_type: i64,
    pub assign_title: Option<String>,
    pub assign_tags: Vec<i64>,
    pub assign_correspondent: Option<i64>,
    pub assign_document_type: Option<i64>,
    pub assign_storage_path: Option<i64>,
    pub assign_owner: Option<i64>,
    pub assign_view_users: Vec<i64>,
    pub assign_view_groups: Vec<i64>,
    pub assign_change_users: Vec<i64>,
    pub assign_change_groups: Vec<i64>,
    pub assign_custom_fields: Vec<i64>,
    pub assign_custom_fields_values: Option<Value>,
    pub remove_all_tags: bool,
    pub remove_tags: Vec<i64>,
    pub remove_all_correspondents: bool,
    pub remove_correspondents: Vec<i64>,
    pub remove_all_document_types: bool,
    pub remove_document_types: Vec<i64>,
    pub remove_all_storage_paths: bool,
    pub remove_storage_paths: Vec<i64>,
    pub remove_all_owners: bool,
    pub remove_owners: Vec<i64>,
    pub remove_all_permissions: bool,
    pub remove_view_users: Vec<i64>,
    pub remove_view_groups: Vec<i64>,
    pub remove_change_users: Vec<i64>,
    pub remove_change_groups: Vec<i64>,
    pub remove_all_custom_fields: bool,
    pub remove_custom_fields: Vec<i64>,
}

impl ActionDef {
    fn has_assign_perms(&self) -> bool {
        !(self.assign_view_users.is_empty()
            && self.assign_view_groups.is_empty()
            && self.assign_change_users.is_empty()
            && self.assign_change_groups.is_empty())
    }
    fn has_remove_perms(&self) -> bool {
        !(self.remove_view_users.is_empty()
            && self.remove_view_groups.is_empty()
            && self.remove_change_users.is_empty()
            && self.remove_change_groups.is_empty())
    }
    /// `str(action)`
    pub fn display(&self) -> String {
        format!("WorkflowAction {}", self.id)
    }
    /// `assign_custom_fields_values.get(str(field.pk), None)`
    fn cf_value(&self, field: i64) -> Value {
        self.assign_custom_fields_values.as_ref().and_then(|v| v.get(field.to_string())).cloned().unwrap_or(Value::Null)
    }
}

/// `Workflow` with triggers and actions.
#[derive(Debug, Clone, Default)]
pub struct WorkflowDef {
    pub id: i64,
    pub name: String,
    pub triggers: Vec<WorkflowTrigger>,
    pub actions: Vec<ActionDef>,
    /// PaperRust: "else" branch (`paperrust_workflow_else_action`, [`crate::workflow_ext`]).
    pub else_actions: Vec<ActionDef>,
    /// PaperRust: AI condition (`paperrust_workflow_ext`).
    pub ai_condition: Option<crate::workflow_ext::AiConditionDef>,
}

impl WorkflowDef {
    /// Does the workflow have PaperRust extensions (else branch, AI condition)?
    pub fn has_ext(&self) -> bool {
        !self.else_actions.is_empty() || self.ai_condition.is_some()
    }
    /// Does the workflow have a trigger of this type ("trigger has fired")?
    pub fn fires_on(&self, trigger_type: i64) -> bool {
        self.triggers.iter().any(|t| t.trigger_type == trigger_type)
    }
}

/// M2M targets of many owners in **one** query: `owner → [target]` (ascending).
async fn m2m_many(
    conn: &mut PgConnection,
    table: &str,
    owner_col: &str,
    target_col: &str,
    owners: &[i64],
) -> Result<HashMap<i64, Vec<i64>>, sqlx::Error> {
    let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
    if owners.is_empty() {
        return Ok(out);
    }
    let q = Query::select()
        .column(a(owner_col))
        .column(a(target_col))
        .from(a(table))
        .and_where(Expr::col(a(owner_col)).is_in(owners.iter().copied()))
        .order_by(a(owner_col), Order::Asc)
        .order_by(a(target_col), Order::Asc)
        .to_owned();
    for r in db::fetch_all(conn, &q).await? {
        let o = i64::from(r.try_get::<i32, _>(0)?);
        out.entry(o).or_default().push(i64::from(r.try_get::<i32, _>(1)?));
    }
    Ok(out)
}

/// Named M2M targets of many triggers in one query: `trigger → [ObjRef]` (by name).
async fn m2m_named_many(
    conn: &mut PgConnection,
    table: &str,
    target_table: &str,
    target_col: &str,
    triggers: &[i64],
) -> Result<HashMap<i64, Vec<ObjRef>>, sqlx::Error> {
    let mut out: HashMap<i64, Vec<ObjRef>> = HashMap::new();
    if triggers.is_empty() {
        return Ok(out);
    }
    let q = Query::select()
        .column((a(table), a("workflowtrigger_id")))
        .column((a("o"), a("id")))
        .column((a("o"), a("name")))
        .from_as(a(target_table), a("o"))
        .inner_join(a(table), col(table, target_col).equals((a("o"), a("id"))))
        .and_where(col(table, "workflowtrigger_id").is_in(triggers.iter().copied()))
        .order_by((a("o"), a("name")), Order::Asc)
        .order_by((a("o"), a("id")), Order::Asc)
        .to_owned();
    for r in db::fetch_all(conn, &q).await? {
        let t = i64::from(r.try_get::<i32, _>(0)?);
        out.entry(t).or_default().push(ObjRef { id: i64::from(r.try_get::<i32, _>(1)?), name: r.try_get(2)? });
    }
    Ok(out)
}

/// Names of many objects of a table in one query.
async fn names_many(conn: &mut PgConnection, table: &str, ids: &[i64]) -> Result<HashMap<i64, String>, sqlx::Error> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let q = Query::select()
        .column(a("id"))
        .column(a("name"))
        .from(a(table))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .to_owned();
    for r in db::fetch_all(conn, &q).await? {
        out.insert(i64::from(r.try_get::<i32, _>(0)?), r.try_get(1)?);
    }
    Ok(out)
}

fn take<T>(m: &mut HashMap<i64, Vec<T>>, id: i64) -> Vec<T> {
    m.remove(&id).unwrap_or_default()
}

/// Trigger fields with M2M (order = fields in [`WorkflowTrigger`]).
const TRIGGER_M2M: [(&str, &str, &str); 9] = [
    ("tags", "documents_tag", "tag_id"),
    ("all_tags", "documents_tag", "tag_id"),
    ("not_tags", "documents_tag", "tag_id"),
    ("any_correspondents", "documents_correspondent", "correspondent_id"),
    ("not_correspondents", "documents_correspondent", "correspondent_id"),
    ("any_document_types", "documents_documenttype", "documenttype_id"),
    ("not_document_types", "documents_documenttype", "documenttype_id"),
    ("any_storage_paths", "documents_storagepath", "storagepath_id"),
    ("not_storage_paths", "documents_storagepath", "storagepath_id"),
];

/// Action fields with M2M.
const ACTION_M2M: [(&str, &str); 16] = [
    ("assign_tags", "tag_id"),
    ("assign_view_users", "user_id"),
    ("assign_view_groups", "group_id"),
    ("assign_change_users", "user_id"),
    ("assign_change_groups", "group_id"),
    ("assign_custom_fields", "customfield_id"),
    ("remove_tags", "tag_id"),
    ("remove_correspondents", "correspondent_id"),
    ("remove_document_types", "documenttype_id"),
    ("remove_storage_paths", "storagepath_id"),
    ("remove_owners", "user_id"),
    ("remove_view_users", "user_id"),
    ("remove_view_groups", "group_id"),
    ("remove_change_users", "user_id"),
    ("remove_change_groups", "group_id"),
    ("remove_custom_fields", "customfield_id"),
];

fn opt_fk(r: &sqlx::postgres::PgRow, c: &str) -> Result<Option<i64>, sqlx::Error> {
    Ok(r.try_get::<Option<i32>, _>(c)?.map(i64::from))
}

fn named(names: &HashMap<i64, String>, id: Option<i64>) -> Option<ObjRef> {
    let id = id?;
    names.get(&id).map(|n| ObjRef { id, name: n.clone() })
}

/// All triggers of the workflows, M2M and names bundled: `workflow_id → [WorkflowTrigger]` (by id).
async fn load_triggers(conn: &mut PgConnection, wf_ids: &[i64]) -> Result<HashMap<i64, Vec<WorkflowTrigger>>, sqlx::Error> {
    let tq = Query::select()
        .column(("documents_workflow_triggers", "workflow_id"))
        .expr(Expr::col((a("t"), sea_query::Asterisk)))
        .from_as(a("documents_workflowtrigger"), a("t"))
        .inner_join(a("documents_workflow_triggers"), col("documents_workflow_triggers", "workflowtrigger_id").equals((a("t"), a("id"))))
        .and_where(col("documents_workflow_triggers", "workflow_id").is_in(wf_ids.iter().copied()))
        .order_by(("documents_workflow_triggers", "workflow_id"), Order::Asc)
        .order_by((a("t"), a("id")), Order::Asc)
        .to_owned();
    let rows = db::fetch_all(conn, &tq).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i32, _>("id").map(i64::from)).collect::<Result<_, _>>()?;
    let mut m2m: Vec<HashMap<i64, Vec<ObjRef>>> = Vec::with_capacity(TRIGGER_M2M.len());
    for (suffix, target_table, target_col) in TRIGGER_M2M {
        m2m.push(m2m_named_many(conn, &format!("documents_workflowtrigger_filter_has_{suffix}"), target_table, target_col, &ids).await?);
    }
    let fk_ids = |c: &str| -> Result<Vec<i64>, sqlx::Error> {
        let mut v = Vec::new();
        for r in &rows {
            v.extend(opt_fk(r, c)?);
        }
        Ok(v)
    };
    let (c_ids, d_ids, s_ids) =
        (fk_ids("filter_has_correspondent_id")?, fk_ids("filter_has_document_type_id")?, fk_ids("filter_has_storage_path_id")?);
    let corr = names_many(conn, "documents_correspondent", &c_ids).await?;
    let dtypes = names_many(conn, "documents_documenttype", &d_ids).await?;
    let spaths = names_many(conn, "documents_storagepath", &s_ids).await?;
    let mut out: HashMap<i64, Vec<WorkflowTrigger>> = HashMap::new();
    for (r, &id) in rows.iter().zip(&ids) {
        let wf = i64::from(r.try_get::<i32, _>("workflow_id")?);
        let sources: String = r.try_get("sources")?;
        let mut f: Vec<Vec<ObjRef>> = m2m.iter_mut().map(|m| take(m, id)).collect();
        f.reverse();
        let mut next = || f.pop().unwrap_or_default();
        let t = WorkflowTrigger {
            id,
            trigger_type: i64::from(r.try_get::<i16, _>("type")?),
            sources: sources.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
            filter_filename: r.try_get("filter_filename")?,
            filter_path: r.try_get("filter_path")?,
            filter_mailrule_id: opt_fk(r, "filter_mailrule_id")?,
            matching_algorithm: i64::from(r.try_get::<i16, _>("matching_algorithm")?),
            match_text: r.try_get("match")?,
            is_insensitive: r.try_get("is_insensitive")?,
            filter_has_tags: next(),
            filter_has_all_tags: next(),
            filter_has_not_tags: next(),
            filter_has_any_correspondents: next(),
            filter_has_correspondent: named(&corr, opt_fk(r, "filter_has_correspondent_id")?),
            filter_has_not_correspondents: next(),
            filter_has_any_document_types: next(),
            filter_has_document_type: named(&dtypes, opt_fk(r, "filter_has_document_type_id")?),
            filter_has_not_document_types: next(),
            filter_has_any_storage_paths: next(),
            filter_has_storage_path: named(&spaths, opt_fk(r, "filter_has_storage_path_id")?),
            filter_has_not_storage_paths: next(),
            filter_custom_field_query: r.try_get("filter_custom_field_query")?,
        };
        out.entry(wf).or_default().push(t);
    }
    Ok(out)
}

/// All actions of the workflows bundled: `workflow_id → [ActionDef]` (by `order`, id).
async fn load_actions(conn: &mut PgConnection, wf_ids: &[i64]) -> Result<HashMap<i64, Vec<ActionDef>>, sqlx::Error> {
    load_actions_via(conn, "documents_workflow_actions", wf_ids).await
}

/// Like [`load_actions`] over another link table (`workflow_id`, `workflowaction_id`):
/// `documents_workflow_actions` (main actions) or `paperrust_workflow_else_action` (else branch).
async fn load_actions_via(
    conn: &mut PgConnection,
    link: &'static str,
    wf_ids: &[i64],
) -> Result<HashMap<i64, Vec<ActionDef>>, sqlx::Error> {
    let aq = Query::select()
        .column((a(link), a("workflow_id")))
        .expr(Expr::col((a("x"), sea_query::Asterisk)))
        .from_as(a("documents_workflowaction"), a("x"))
        .inner_join(a(link), col(link, "workflowaction_id").equals((a("x"), a("id"))))
        .and_where(col(link, "workflow_id").is_in(wf_ids.iter().copied()))
        .order_by((a(link), a("workflow_id")), Order::Asc)
        .order_by((a("x"), a("order")), Order::Asc)
        .order_by((a("x"), a("id")), Order::Asc)
        .to_owned();
    let rows = db::fetch_all(conn, &aq).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.try_get::<i32, _>("id").map(i64::from)).collect::<Result<_, _>>()?;
    let mut m2m: Vec<HashMap<i64, Vec<i64>>> = Vec::with_capacity(ACTION_M2M.len());
    for (field, target_col) in ACTION_M2M {
        m2m.push(m2m_many(conn, &format!("documents_workflowaction_{field}"), "workflowaction_id", target_col, &ids).await?);
    }
    let mut out: HashMap<i64, Vec<ActionDef>> = HashMap::new();
    for (r, &id) in rows.iter().zip(&ids) {
        let wf = i64::from(r.try_get::<i32, _>("workflow_id")?);
        let mut f: Vec<Vec<i64>> = m2m.iter_mut().map(|m| take(m, id)).collect();
        f.reverse();
        let mut next = || f.pop().unwrap_or_default();
        let x = ActionDef {
            id,
            action_type: i64::from(r.try_get::<i16, _>("type")?),
            assign_title: r.try_get("assign_title")?,
            assign_correspondent: opt_fk(r, "assign_correspondent_id")?,
            assign_document_type: opt_fk(r, "assign_document_type_id")?,
            assign_storage_path: opt_fk(r, "assign_storage_path_id")?,
            assign_owner: opt_fk(r, "assign_owner_id")?,
            assign_custom_fields_values: r.try_get("assign_custom_fields_values")?,
            remove_all_tags: r.try_get("remove_all_tags")?,
            remove_all_correspondents: r.try_get("remove_all_correspondents")?,
            remove_all_document_types: r.try_get("remove_all_document_types")?,
            remove_all_storage_paths: r.try_get("remove_all_storage_paths")?,
            remove_all_owners: r.try_get("remove_all_owners")?,
            remove_all_permissions: r.try_get("remove_all_permissions")?,
            remove_all_custom_fields: r.try_get("remove_all_custom_fields")?,
            assign_tags: next(),
            assign_view_users: next(),
            assign_view_groups: next(),
            assign_change_users: next(),
            assign_change_groups: next(),
            assign_custom_fields: next(),
            remove_tags: next(),
            remove_correspondents: next(),
            remove_document_types: next(),
            remove_storage_paths: next(),
            remove_owners: next(),
            remove_view_users: next(),
            remove_view_groups: next(),
            remove_change_users: next(),
            remove_change_groups: next(),
            remove_custom_fields: next(),
        };
        out.entry(wf).or_default().push(x);
    }
    Ok(out)
}

/// `get_workflows_for_trigger(trigger_type)`: active workflows with a trigger of the type, order `order`.
///
/// PaperRust (PERF-SCALE §11): triggers, actions and all M2M fields are loaded bundled for **all**
/// workflows (≈ 30 queries instead of ≈ 30 per workflow); content and order
/// as before.
pub async fn workflows_for_trigger(conn: &mut PgConnection, trigger_type: i64) -> Result<Vec<WorkflowDef>, sqlx::Error> {
    let q = Query::select()
        .distinct()
        .column((a("w"), a("id")))
        .column((a("w"), a("name")))
        .column((a("w"), a("order")))
        .from_as(a("documents_workflow"), a("w"))
        .inner_join(a("documents_workflow_triggers"), col("documents_workflow_triggers", "workflow_id").equals((a("w"), a("id"))))
        .inner_join(
            a("documents_workflowtrigger"),
            col("documents_workflowtrigger", "id").equals(("documents_workflow_triggers", "workflowtrigger_id")),
        )
        .and_where(col("w", "enabled").eq(true))
        .and_where(col("documents_workflowtrigger", "type").eq(trigger_type as i16))
        .order_by((a("w"), a("order")), Order::Asc)
        .order_by((a("w"), a("id")), Order::Asc)
        .to_owned();
    let rows = db::fetch_all(conn, &q).await?;
    let heads: Vec<(i64, String)> =
        rows.iter().map(|r| Ok((i64::from(r.try_get::<i32, _>(0)?), r.try_get(1)?))).collect::<Result<_, sqlx::Error>>()?;
    if heads.is_empty() {
        return Ok(Vec::new());
    }
    let wf_ids: Vec<i64> = heads.iter().map(|(id, _)| *id).collect();
    load_defs(conn, heads, &wf_ids).await
}

/// Load triggers, main and else actions and AI conditions bundled.
async fn load_defs(conn: &mut PgConnection, heads: Vec<(i64, String)>, wf_ids: &[i64]) -> Result<Vec<WorkflowDef>, sqlx::Error> {
    let mut triggers = load_triggers(conn, wf_ids).await?;
    let mut actions = load_actions(conn, wf_ids).await?;
    crate::workflow_ext::ensure_conn(conn).await?;
    let mut else_actions = load_actions_via(conn, "paperrust_workflow_else_action", wf_ids).await?;
    let mut ai = crate::workflow_ext::load_ai_conditions(conn, wf_ids).await?;
    Ok(heads
        .into_iter()
        .map(|(id, name)| WorkflowDef {
            id,
            name,
            triggers: take(&mut triggers, id),
            actions: take(&mut actions, id),
            else_actions: take(&mut else_actions, id),
            ai_condition: ai.remove(&id),
        })
        .collect())
}

/// `Workflow.objects.get(id=…)` with triggers and actions (manual run,
/// `run_workflows_for_documents`). Like upstream without checking `enabled` – the API checks that
/// when enqueueing. `None` if the workflow does not (or no longer) exist.
pub async fn workflow_by_id(conn: &mut PgConnection, workflow_id: i64) -> Result<Option<WorkflowDef>, sqlx::Error> {
    let Ok(id32) = i32::try_from(workflow_id) else { return Ok(None) };
    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM documents_workflow WHERE id = $1").bind(id32).fetch_optional(&mut *conn).await?;
    match name {
        Some(name) => Ok(Some(load_workflow_def(conn, workflow_id, name).await?)),
        None => Ok(None),
    }
}

/// Load triggers (by id) and actions (by `order`, id) of a workflow – via the same
/// bundled loaders as `workflows_for_trigger`.
async fn load_workflow_def(conn: &mut PgConnection, id: i64, name: String) -> Result<WorkflowDef, sqlx::Error> {
    let mut v = load_defs(conn, vec![(id, name.clone())], &[id]).await?;
    Ok(v.pop().unwrap_or(WorkflowDef { id, name, ..Default::default() }))
}

/// `WorkflowRun.objects.create(workflow=, type=, document=)`.
pub async fn record_run(conn: &mut PgConnection, workflow_id: i64, trigger_type: i64, document_id: Option<i64>) -> Result<(), sqlx::Error> {
    let q = Query::insert()
        .into_table(a("documents_workflowrun"))
        .columns([a("type"), a("run_at"), a("document_id"), a("workflow_id")])
        .values_panic([
            (trigger_type as i16).into(),
            now_micros().into(),
            document_id.map(|i| i as i32).into(),
            (workflow_id as i32).into(),
        ])
        .to_owned();
    let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    Ok(())
}

fn union(a: &Option<Vec<i64>>, b: &[i64]) -> Option<Vec<i64>> {
    let mut v = a.clone().unwrap_or_default();
    v.extend_from_slice(b);
    v.sort_unstable();
    v.dedup();
    Some(v)
}

/// `apply_assignment_to_overrides`.
pub async fn apply_assignment_to_overrides(
    conn: &mut PgConnection,
    action: &ActionDef,
    o: &mut DocumentMetadataOverrides,
) -> Result<(), sqlx::Error> {
    if !action.assign_tags.is_empty() {
        let add = db::with_ancestors(conn, &action.assign_tags).await?;
        o.tag_ids = union(&o.tag_ids, &add);
    }
    if let Some(c) = action.assign_correspondent {
        o.correspondent_id = Some(c);
    }
    if let Some(t) = action.assign_document_type {
        o.document_type_id = Some(t);
    }
    if let Some(s) = action.assign_storage_path {
        o.storage_path_id = Some(s);
    }
    if let Some(u) = action.assign_owner {
        o.owner_id = Some(u);
    }
    if let Some(t) = action.assign_title.as_ref().filter(|t| !t.is_empty()) {
        o.title = Some(t.clone());
    }
    if action.has_assign_perms() {
        o.view_users = union(&o.view_users, &action.assign_view_users);
        o.view_groups = union(&o.view_groups, &action.assign_view_groups);
        o.change_users = union(&o.change_users, &action.assign_change_users);
        o.change_groups = union(&o.change_groups, &action.assign_change_groups);
    }
    if !action.assign_custom_fields.is_empty() {
        let m = o.custom_fields.get_or_insert_with(BTreeMap::new);
        for f in &action.assign_custom_fields {
            m.insert(*f, action.cf_value(*f));
        }
    }
    Ok(())
}

/// `apply_removal_to_overrides`.
pub async fn apply_removal_to_overrides(
    conn: &mut PgConnection,
    action: &ActionDef,
    o: &mut DocumentMetadataOverrides,
) -> Result<(), sqlx::Error> {
    if action.remove_all_tags {
        o.tag_ids = None;
    } else if o.tag_ids.as_ref().is_some_and(|t| !t.is_empty()) {
        let mut remove: Vec<i64> = Vec::new();
        for t in &action.remove_tags {
            remove.push(*t);
            remove.extend(db::tag_descendants(conn, *t).await?);
        }
        if let Some(t) = o.tag_ids.as_mut() {
            t.retain(|x| !remove.contains(x));
        }
    }
    if action.remove_all_correspondents || o.correspondent_id.is_some_and(|c| action.remove_correspondents.contains(&c)) {
        o.correspondent_id = None;
    }
    if action.remove_all_document_types || o.document_type_id.is_some_and(|c| action.remove_document_types.contains(&c)) {
        o.document_type_id = None;
    }
    if action.remove_all_storage_paths || o.storage_path_id.is_some_and(|c| action.remove_storage_paths.contains(&c)) {
        o.storage_path_id = None;
    }
    if action.remove_all_owners || o.owner_id.is_some_and(|c| action.remove_owners.contains(&c)) {
        o.owner_id = None;
    }
    if action.remove_all_permissions {
        o.view_users = None;
        o.view_groups = None;
        o.change_users = None;
        o.change_groups = None;
    } else if action.has_remove_perms() {
        for (list, rm) in [
            (&mut o.view_users, &action.remove_view_users),
            (&mut o.change_users, &action.remove_change_users),
            (&mut o.view_groups, &action.remove_view_groups),
            (&mut o.change_groups, &action.remove_change_groups),
        ] {
            if let Some(l) = list.as_mut() {
                l.retain(|x| !rm.contains(x));
            }
        }
    }
    if action.remove_all_custom_fields {
        o.custom_fields = None;
    } else if !action.remove_custom_fields.is_empty()
        && let Some(m) = o.custom_fields.as_mut()
    {
        for f in &action.remove_custom_fields {
            m.remove(f);
        }
    }
    Ok(())
}

/// Result of the consumption trigger.
pub struct ConsumptionResult {
    pub overrides: DocumentMetadataOverrides,
    pub message: String,
    /// `StopConsumeTaskError` through "move to trash".
    pub stop: Option<String>,
}

/// Action input for the ingestion trigger: `build_workflow_action_context(input_doc, overrides)` with
/// the state of the overrides at the time of the action.
fn consumption_input(action: &ActionDef, overrides: &DocumentMetadataOverrides, input: &ConsumableDocument) -> ActionInput {
    ActionInput {
        ctx: ActionContext {
            workflow_action_id: action.id,
            action_type: action.action_type,
            document_id: None,
            title: overrides.title.clone().unwrap_or_default(),
            original_file: input.original_file.clone(),
            trigger_type: TRIGGER_CONSUMPTION,
        },
        overrides: Some(overrides.clone()),
        document: None,
        mime_type: input.mime_type.clone(),
    }
}

/// Action input for an existing document: `build_workflow_action_context(document, None)`
/// with the in-memory instance (incl. not yet saved assignments of this workflow).
fn document_input(action: &ActionDef, doc: &DocInstance, original_file: &Path, trigger_type: i64) -> ActionInput {
    ActionInput {
        ctx: ActionContext {
            workflow_action_id: action.id,
            action_type: action.action_type,
            document_id: Some(doc.row.id),
            title: doc.row.title.clone(),
            original_file: original_file.to_path_buf(),
            trigger_type,
        },
        overrides: None,
        document: Some(doc.row.clone()),
        mime_type: doc.row.mime_type.clone(),
    }
}

/// `run_workflows(CONSUMPTION, document=input_doc, overrides=DocumentMetadataOverrides())`.
pub async fn run_consumption(
    conn: &mut PgConnection,
    input: &ConsumableDocument,
    effects: &dyn WorkflowEffects,
) -> Result<ConsumptionResult, sqlx::Error> {
    let mut overrides = DocumentMetadataOverrides::default();
    let mut messages = Vec::new();
    let doc = input.as_matching();
    for wf in workflows_for_trigger(conn, TRIGGER_CONSUMPTION).await? {
        let state = match document_matches_workflow(WorkflowDocument::Consumable(&doc), &wf.name, &wf.triggers, TRIGGER_CONSUMPTION, None) {
            Ok(true) => MatchState::Matched,
            Ok(false) => MatchState::NotMatched { reason: None },
            Err(e) => {
                tracing::error!(target: "paperless.handlers", "Error matching workflow {}: {e:?}", wf.name);
                MatchState::Error(format!("{e:?}"))
            }
        };
        // PaperRust: else branch / AI condition (ignored on ingestion, marked)
        let decision = workflow_ext::decide(&wf, TRIGGER_CONSUMPTION, state, None, ExtOptions::default()).await;
        let branch = match &decision {
            Decision::Run { branch, .. } => *branch,
            _ => {
                if let Some(mut r) = workflow_ext::record_for(&wf, TRIGGER_CONSUMPTION, None, &decision) {
                    r.source = Some(doc.source as i64);
                    workflow_ext::insert_run(conn, &r).await?;
                }
                continue;
            }
        };
        let mut trash = false;
        for action in workflow_ext::branch_actions(&wf, branch) {
            messages.push(format!("Applying {} from Workflow: {}", action.display(), wf.name));
            match action.action_type {
                ACTION_ASSIGNMENT => apply_assignment_to_overrides(conn, action, &mut overrides).await?,
                ACTION_REMOVAL => apply_removal_to_overrides(conn, action, &mut overrides).await?,
                ACTION_EMAIL => effects.email_in(conn, &consumption_input(action, &overrides, input)).await,
                ACTION_WEBHOOK => effects.webhook_in(conn, &consumption_input(action, &overrides, input)).await,
                ACTION_PASSWORD_REMOVAL => effects.password_removal_in(conn, &consumption_input(action, &overrides, input)).await,
                ACTION_MOVE_TO_TRASH => trash = true,
                ACTION_REMOTE_OCR => overrides.remote_ocr = true,
                ACTION_APPLY_AI_SUGGESTIONS => tracing::debug!(
                    target: "paperless.handlers",
                    "Apply AI suggestions action does not apply to consumption triggers, ignoring"
                ),
                _ => {}
            }
        }
        record_run(conn, wf.id, TRIGGER_CONSUMPTION, None).await?;
        if let Some(mut r) = workflow_ext::record_for(&wf, TRIGGER_CONSUMPTION, None, &decision) {
            r.source = Some(doc.source as i64);
            r.changed_fields = workflow_ext::touched(workflow_ext::branch_actions(&wf, branch)).into_iter().map(String::from).collect();
            workflow_ext::insert_run(conn, &r).await?;
        }
        if trash {
            if input.original_file.exists() {
                let _ = std::fs::remove_file(&input.original_file);
            }
            tracing::info!(
                target: "paperless.workflows",
                "Workflow move to trash action triggered during consumption, deleting file {}",
                input.original_file.display()
            );
            return Ok(ConsumptionResult {
                overrides,
                message: messages.join("\n"),
                stop: Some("Document deleted by workflow action during consumption".into()),
            });
        }
    }
    Ok(ConsumptionResult { overrides, message: messages.join("\n"), stop: None })
}

/// `ExistingDocument` for the trigger check.
/// [`ExistingDocument`] from already loaded parts (bundled follow-up); same fields as
/// [`existing`].
pub fn existing_from_parts(
    r: &db::DocRow,
    correspondent: Option<ObjRef>,
    document_type: Option<ObjRef>,
    storage_path: Option<ObjRef>,
    tags: Vec<ObjRef>,
    effective_content: String,
) -> ExistingDocument {
    let display =
        crate::audit::document_repr(&crate::audit::CreatedAttr::Date(r.created), correspondent.as_ref().map(|c| c.name.as_str()), &r.title);
    ExistingDocument {
        id: r.id,
        effective_content: Some(effective_content),
        display,
        tags,
        correspondent,
        document_type,
        storage_path,
        original_filename: r.original_filename.clone(),
    }
}

/// Index of the first workflow in `workflows` that matches `ex` (same check and logs as in
/// [`run_workflows`]). As long as no workflow matches, `run_workflows` changes nothing on the document;
/// all previous workflows therefore see the same state – that is why the bundled follow-up may
/// only take the single path ([`run_workflow_list`]) from this index on.
pub fn first_matching_workflow(
    workflows: &[WorkflowDef],
    trigger_type: i64,
    ex: &ExistingDocument,
    cfq: &CustomFieldQueries,
) -> Option<usize> {
    // PaperRust: a non-matching workflow with an else branch also does something → single path from it on
    workflows.iter().position(|wf| match match_existing(wf, trigger_type, ex, cfq, false) {
        MatchState::Matched => true,
        MatchState::NotMatched { .. } => !wf.else_actions.is_empty(),
        _ => false,
    })
}

/// Can the bundled follow-up reproduce the workflow? Only without PaperRust extensions and with
/// only [simple](is_simple_action) actions.
pub fn is_simple_workflow(wf: &WorkflowDef) -> bool {
    !wf.has_ext() && wf.actions.iter().all(is_simple_action)
}

async fn existing(conn: &mut PgConnection, d: &DocInstance) -> Result<ExistingDocument, sqlx::Error> {
    let r = &d.row;
    let named = |table: &'static str, id: Option<i64>| (table, id);
    let mut refs = Vec::new();
    for (table, id) in [
        named("documents_correspondent", r.correspondent_id),
        named("documents_documenttype", r.document_type_id),
        named("documents_storagepath", r.storage_path_id),
    ] {
        refs.push(match id {
            Some(id) => db::object_name(conn, table, id).await?.map(|n| ObjRef { id, name: n }),
            None => None,
        });
    }
    let tags = db::document_tags(conn, r.id).await?.into_iter().map(|(id, name)| ObjRef { id, name }).collect();
    let content = effective_content(conn, r).await?;
    Ok(ExistingDocument {
        id: r.id,
        effective_content: Some(content),
        display: d.repr(conn).await?,
        tags,
        correspondent: refs[0].clone(),
        document_type: refs[1].clone(),
        storage_path: refs[2].clone(),
        original_filename: r.original_filename.clone(),
    })
}

/// `get_effective_content()`: content of the latest version, otherwise own content.
pub async fn effective_content(conn: &mut PgConnection, r: &db::DocRow) -> Result<String, sqlx::Error> {
    let q = Query::select()
        .column(a("content"))
        .from(a("documents_document"))
        .and_where(Expr::col(a("root_document_id")).eq(r.id))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .order_by_expr(Expr::col(a("version_index")).into(), Order::Desc)
        .order_by(a("id"), Order::Desc)
        .limit(1)
        .to_owned();
    Ok(match db::fetch_optional(conn, &q).await? {
        Some(row) => row.try_get(0)?,
        None => r.content.clone(),
    })
}

/// `document.delete()` of an existing document (workflow action "move to trash").
///
/// Upstream: `Document.delete()` (versions with the same `transaction_id`) + django-softdelete
/// (related soft-delete objects, barcodes hard). [`NativeSoftDelete`] reproduces the cascade without
/// django-auditlog entries of the related notes/field values; the worker plugs in the full
/// implementation from pl-documents-write (`softdelete::soft_delete_document`).
pub trait DocumentDeleter: Send + Sync {
    fn soft_delete<'a>(&'a self, conn: &'a mut PgConnection, audit: &'a AuditLog, doc_id: i64) -> BoxFuture<'a, Result<(), sqlx::Error>>;
}

/// Soft-delete cascade without audit of the related objects (see [`DocumentDeleter`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeSoftDelete;

impl DocumentDeleter for NativeSoftDelete {
    fn soft_delete<'a>(&'a self, conn: &'a mut PgConnection, audit: &'a AuditLog, doc_id: i64) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(native_soft_delete(conn, audit, doc_id, uuid::Uuid::new_v4()))
    }
}

async fn native_soft_delete(conn: &mut PgConnection, audit: &AuditLog, doc_id: i64, tid: uuid::Uuid) -> Result<(), sqlx::Error> {
    let Some(row) = db::load_document(conn, doc_id).await? else { return Ok(()) };
    if row.root_document_id.is_none() {
        let versions: Vec<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id",
        )
        .bind(doc_id as i32)
        .fetch_all(&mut *conn)
        .await?;
        for v in versions {
            Box::pin(native_soft_delete(conn, audit, i64::from(v), tid)).await?;
        }
    }
    let now = now_micros();
    for table in ["documents_note", "documents_sharelink", "documents_customfieldinstance", "documents_workflowrun"] {
        let sql = format!(
            "UPDATE {table} SET deleted_at = $2, restored_at = NULL, transaction_id = $3 WHERE document_id = $1 AND deleted_at IS NULL"
        );
        sqlx::query(&sql).bind(doc_id as i32).bind(now).bind(tid).execute(&mut *conn).await?;
    }
    sqlx::query("DELETE FROM documents_documentbarcode WHERE document_id = $1").bind(doc_id as i32).execute(&mut *conn).await?;
    let mut doc = DocInstance::from_db(row);
    doc.row.deleted_at = Some(now);
    doc.row.restored_at = None;
    doc.row.transaction_id = Some(tid);
    docops::save(conn, audit, &mut doc, Some(&["deleted_at", "restored_at", "transaction_id"])).await
}

/// Environment of [`run_workflows`] on an existing document.
pub struct RunEnv<'a> {
    pub audit: &'a AuditLog,
    pub effects: &'a dyn WorkflowEffects,
    pub time_zone: chrono_tz::Tz,
    /// `original_file` of the caller (ingestion: working copy); `None` → `document.source_path`.
    pub original_file: Option<&'a Path>,
    /// Paths/templates: `source_path` and the post_save receiver `update_filename_and_move_files`
    /// after `document.save()`. `None` on ingestion (the files are not there yet, upstream
    /// returns immediately without `filename`).
    pub settings: Option<&'a ConsumerSettings>,
    pub deleter: &'a dyn DocumentDeleter,
    /// PaperRust: AI condition and history ([`crate::workflow_ext`]).
    pub ext: ExtOptions,
}

/// Result of [`run_workflows`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    /// `apply_ai_suggestions.delay_on_commit(action_id=, document_id=)` – to be enqueued by the caller after the
    /// commit ([`enqueue_ai_suggestions`]).
    pub ai_suggestions: Vec<(i64, i64)>,
    /// PaperRust: runs of this call (also written to the history `paperrust_workflow_run`).
    pub runs: Vec<RunRecord>,
    /// PaperRust: runs to be caught up after the commit (AI condition, [`ExtOptions::ai_mode`] = Defer).
    pub deferred: Vec<DeferredRun>,
}

impl RunOutcome {
    pub fn extend(&mut self, other: RunOutcome) {
        self.ai_suggestions.extend(other.ai_suggestions);
        self.runs.extend(other.runs);
        self.deferred.extend(other.deferred);
    }
}

/// Task name of `apply_ai_suggestions`.
pub const APPLY_AI_SUGGESTIONS_TASK: &str = "documents.tasks.apply_ai_suggestions";

/// kwargs of `apply_ai_suggestions` (`{"action_id": …, "document_id": …}`).
pub fn ai_suggestions_kwargs(action_id: i64, document_id: i64) -> serde_json::Map<String, Value> {
    let mut kw = serde_json::Map::new();
    kw.insert("action_id".into(), Value::from(action_id));
    kw.insert("document_id".into(), Value::from(document_id));
    kw
}

/// kwargs of a caught-up run (task `run_workflows_for_documents`, PaperRust extension
/// `match_trigger`).
pub fn deferred_kwargs(d: &DeferredRun) -> serde_json::Map<String, Value> {
    let mut kw = serde_json::Map::new();
    kw.insert("document_ids".into(), Value::from(vec![d.document_id]));
    kw.insert("workflow_id".into(), Value::from(d.workflow_id));
    kw.insert("filter_documents".into(), Value::Bool(true));
    kw.insert("match_trigger".into(), Value::from(d.trigger_type));
    kw
}

/// Enqueue the collected `apply_ai_suggestions` and runs to be caught up (after the commit).
/// Error → log (upstream: `delay_on_commit` in the on_commit hook).
pub async fn enqueue_ai_suggestions(queue: Option<&dyn pl_queue::TaskQueue>, outcome: &RunOutcome) {
    for d in &outcome.deferred {
        let Some(q) = queue else {
            tracing::warn!(
                target: "paperless.handlers",
                "No task queue available, deferred workflow {} for document {} (AI condition) not queued",
                d.workflow_id,
                d.document_id
            );
            continue;
        };
        if let Err(e) = q.enqueue(crate::manual::RUN_WORKFLOWS_FOR_DOCUMENTS, deferred_kwargs(d), pl_queue::EnqueueOptions::default()).await
        {
            tracing::error!(target: "paperless.handlers", "Could not queue deferred workflow {} for document {}: {e}", d.workflow_id, d.document_id);
        }
    }
    for (action_id, document_id) in &outcome.ai_suggestions {
        let Some(q) = queue else {
            tracing::warn!(
                target: "paperless.handlers",
                "No task queue available, apply_ai_suggestions for document {document_id} (action {action_id}) not queued"
            );
            continue;
        };
        if let Err(e) =
            q.enqueue(APPLY_AI_SUGGESTIONS_TASK, ai_suggestions_kwargs(*action_id, *document_id), pl_queue::EnqueueOptions::default()).await
        {
            tracing::error!(target: "paperless.handlers", "Could not queue apply_ai_suggestions for document {document_id}: {e}");
        }
    }
}

/// `run_workflows(DOCUMENT_ADDED, document, original_file=…)` (Signal `document_consumption_finished`).
pub async fn run_added(
    conn: &mut PgConnection,
    audit: &AuditLog,
    doc: &mut DocInstance,
    original_file: &Path,
    effects: &dyn WorkflowEffects,
    time_zone: chrono_tz::Tz,
) -> Result<RunOutcome, sqlx::Error> {
    // AI condition: the new document is not yet committed → catch up the run after the commit
    let ext = ExtOptions { ai_mode: workflow_ext::AiMode::Defer, log_skips: false };
    let env = RunEnv { audit, effects, time_zone, original_file: Some(original_file), settings: None, deleter: &NativeSoftDelete, ext };
    run_workflows(conn, &env, pl_matching::workflow::TRIGGER_DOCUMENT_ADDED, doc, None).await
}

/// Pre-evaluated `filter_custom_field_query` of triggers for a fixed document set
/// (final security review K-N1). The evaluation needs the database (async), the matching in
/// pl-matching is synchronous – hence first [`eval_custom_field_queries`], then this evaluator.
#[derive(Debug, Default)]
pub struct CustomFieldQueries(HashMap<String, Option<HashSet<i64>>>);

impl CustomFieldQueryEvaluator for CustomFieldQueries {
    fn matches(&self, query: &str, document_id: i64) -> Result<bool, InvalidCustomFieldQuery> {
        match self.0.get(query) {
            Some(Some(ids)) => Ok(ids.contains(&document_id)),
            // invalid (ValidationError) or not pre-evaluated → no hits
            _ => Err(InvalidCustomFieldQuery),
        }
    }
}

impl CustomFieldQueries {
    #[cfg(test)]
    pub(crate) fn from_parts(parts: Vec<(&str, Option<Vec<i64>>)>) -> Self {
        CustomFieldQueries(parts.into_iter().map(|(q, ids)| (q.to_string(), ids.map(|v| v.into_iter().collect()))).collect())
    }
}

/// `CustomFieldQueryParser("filter_custom_field_query").parse(...)` + `.filter(...)` for each non-empty
/// query of the `triggers`, via `conn` (sees open transactions). Without queries no
/// database access. Unhandled parser errors (upstream: exception) → error log and no
/// hits (fail-closed); database errors are passed on.
pub async fn eval_custom_field_queries(
    conn: &mut PgConnection,
    triggers: &[&WorkflowTrigger],
    ids: &[i64],
    tz: chrono_tz::Tz,
) -> Result<CustomFieldQueries, sqlx::Error> {
    let mut out = CustomFieldQueries::default();
    for t in triggers.iter().copied() {
        let Some(q) = t.filter_custom_field_query.as_deref().filter(|q| !q.is_empty()) else { continue };
        if out.0.contains_key(q) {
            continue;
        }
        let r = match pl_docfilter::custom_field_query_matching_ids(conn, q, ids, tz).await {
            Ok(pl_docfilter::CustomFieldQueryMatch::Ids(s)) => Some(s),
            Ok(pl_docfilter::CustomFieldQueryMatch::Invalid) => None,
            Err(pl_docfilter::FilterError::Db(e)) => return Err(e),
            Err(e) => {
                tracing::error!(target: "paperless.matching", "Error evaluating custom field query of trigger {}: {e:?}", t.id);
                None
            }
        };
        out.0.insert(q.to_string(), r);
    }
    Ok(out)
}

/// `run_workflows(trigger_type, document, workflow_to_run=…)` without overrides (document added,
/// document updated, scheduled). `workflow_to_run` skips the query (scheduled runs).
pub async fn run_workflows(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    trigger_type: i64,
    doc: &mut DocInstance,
    workflow_to_run: Option<&WorkflowDef>,
) -> Result<RunOutcome, sqlx::Error> {
    let outcome = RunOutcome::default();
    if doc.row.root_document_id.is_some() {
        tracing::debug!(target: "paperless.handlers", "Skipping workflow execution for version document {}", doc.row.id);
        return Ok(outcome);
    }
    let original_file: PathBuf = match (env.original_file, env.settings) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some(s)) => crate::files::source_path(s, &doc.row),
        (None, None) => PathBuf::new(),
    };
    let workflows = match workflow_to_run {
        Some(w) => vec![w.clone()],
        None => workflows_for_trigger(conn, trigger_type).await?,
    };
    run_workflow_list_inner(conn, env, trigger_type, doc, &workflows, original_file, outcome, MatchMode::Evaluate).await
}

/// Like [`run_workflows`], but with already loaded workflows (bundled follow-up of
/// `bulk_update_documents`: the list is loaded once per task instead of per document; the
/// caller may omit workflows at the front that demonstrably do not match, see
/// [`first_matching_workflow`]).
pub async fn run_workflow_list(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    trigger_type: i64,
    doc: &mut DocInstance,
    workflows: &[WorkflowDef],
) -> Result<RunOutcome, sqlx::Error> {
    let outcome = RunOutcome::default();
    if doc.row.root_document_id.is_some() {
        tracing::debug!(target: "paperless.handlers", "Skipping workflow execution for version document {}", doc.row.id);
        return Ok(outcome);
    }
    let original_file: PathBuf = match (env.original_file, env.settings) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some(s)) => crate::files::source_path(s, &doc.row),
        (None, None) => PathBuf::new(),
    };
    run_workflow_list_inner(conn, env, trigger_type, doc, workflows, original_file, outcome, MatchMode::Evaluate).await
}

/// Manual run of a workflow on an existing document (trigger type `MANUAL`).
/// `skip_matching = true` applies the actions without checking the filters of the triggers
/// (user chose "apply to all selected documents").
/// Idea: paperless-ngx dev `3dbeca603` (#14704, `run_workflows(..., skip_matching=)`).
pub async fn run_workflow_manual(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    doc: &mut DocInstance,
    workflow: &WorkflowDef,
    skip_matching: bool,
) -> Result<RunOutcome, sqlx::Error> {
    run_workflow_as(conn, env, doc, workflow, skip_matching, pl_matching::workflow::TRIGGER_MANUAL).await
}

/// PaperRust: like [`run_workflow_manual`], but with the filters of another trigger type
/// (caught-up runs with an AI condition, "apply retroactively" from the test run). The run counts
/// in the ngx `WorkflowRun` and in the history as this trigger type.
pub async fn run_workflow_as(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    doc: &mut DocInstance,
    workflow: &WorkflowDef,
    skip_matching: bool,
    trigger_type: i64,
) -> Result<RunOutcome, sqlx::Error> {
    let outcome = RunOutcome::default();
    if doc.row.root_document_id.is_some() {
        tracing::debug!(target: "paperless.handlers", "Skipping workflow execution for version document {}", doc.row.id);
        return Ok(outcome);
    }
    let original_file: PathBuf = match (env.original_file, env.settings) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some(s)) => crate::files::source_path(s, &doc.row),
        (None, None) => PathBuf::new(),
    };
    let mode = if skip_matching { MatchMode::Bypass } else { MatchMode::Evaluate };
    run_workflow_list_inner(conn, env, trigger_type, doc, std::slice::from_ref(workflow), original_file, outcome, mode).await
}

/// How the filter matching in [`run_workflow_list_inner`] is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchMode {
    /// Check the filters of the triggers of the type (normal case).
    Evaluate,
    /// Skip the filters (manual "apply to all").
    Bypass,
    /// Given result (test run of an ingestion trigger on an existing document:
    /// matching ran on the reconstructed ingestion document).
    Given(MatchState),
}

/// Core of all runs on existing documents (added, updated via the worker, schedule,
/// manual, caught up) **and** of the test run ([`crate::workflow_dryrun`], in a transaction
/// that is rolled back). One implementation; the guard test in `workflow_dryrun` checks that.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_workflow_list_inner(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    trigger_type: i64,
    doc: &mut DocInstance,
    workflows: &[WorkflowDef],
    original_file: PathBuf,
    mut outcome: RunOutcome,
    mode: MatchMode,
) -> Result<RunOutcome, sqlx::Error> {
    for wf in workflows {
        // document.refresh_from_db()
        let Some(fresh) = db::load_document(conn, doc.row.id).await? else {
            tracing::info!(target: "paperless.handlers", "Document no longer exists, skipping remaining workflows");
            break;
        };
        *doc = DocInstance::from_db(fresh);
        if doc.row.deleted_at.is_some() {
            tracing::info!(target: "paperless.handlers", "Document was moved to trash, skipping remaining workflows");
            break;
        }
        let doc_id = doc.row.id;
        if let Err(e) = run_one_workflow(conn, env, trigger_type, doc, wf, &original_file, &mut outcome, &mode).await {
            // PaperRust: note the error in the history (best effort, see workflow_ext::insert_error)
            workflow_ext::insert_error(conn, wf.id, trigger_type, Some(doc_id), &e).await;
            return Err(e);
        }
    }
    Ok(outcome)
}

/// Filter matching of a workflow on an existing document (same check and logs as
/// upstream `document_matches_workflow`); it determines the reason for "does not match" only if
/// `want_reason` (history/test run).
pub(crate) fn match_existing(
    wf: &WorkflowDef,
    trigger_type: i64,
    ex: &ExistingDocument,
    cfq: &CustomFieldQueries,
    want_reason: bool,
) -> MatchState {
    let fired = wf.fires_on(trigger_type);
    match document_matches_workflow(WorkflowDocument::Existing(ex), &wf.name, &wf.triggers, trigger_type, Some(cfq)) {
        Ok(true) => MatchState::Matched,
        Ok(false) if !fired => MatchState::NotFired,
        Ok(false) => {
            let reason = if want_reason {
                wf.triggers
                    .iter()
                    .filter(|t| t.trigger_type == trigger_type)
                    .filter_map(|t| pl_matching::workflow::existing_document_matches_workflow(ex, t, Some(cfq)).ok())
                    .filter_map(|(_, r)| r)
                    .next_back()
            } else {
                None
            };
            MatchState::NotMatched { reason }
        }
        Err(e) => {
            tracing::error!(target: "paperless.handlers", "Error matching workflow {}: {e:?}", wf.name);
            MatchState::Error(format!("{e:?}"))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_one_workflow(
    conn: &mut PgConnection,
    env: &RunEnv<'_>,
    trigger_type: i64,
    doc: &mut DocInstance,
    wf: &WorkflowDef,
    original_file: &Path,
    outcome: &mut RunOutcome,
    mode: &MatchMode,
) -> Result<(), sqlx::Error> {
    let state = match mode {
        MatchMode::Bypass => MatchState::Bypassed,
        MatchMode::Given(s) => s.clone(),
        MatchMode::Evaluate => {
            let ex = existing(conn, doc).await?;
            let selected: Vec<&WorkflowTrigger> = wf.triggers.iter().filter(|t| t.trigger_type == trigger_type).collect();
            let cfq = eval_custom_field_queries(conn, &selected, &[doc.row.id], env.time_zone).await?;
            match_existing(wf, trigger_type, &ex, &cfq, env.ext.log_skips)
        }
    };
    let decision = workflow_ext::decide(wf, trigger_type, state, Some(doc.row.id), env.ext).await;
    let branch = match &decision {
        Decision::Run { branch, .. } => *branch,
        Decision::Defer => {
            outcome.deferred.push(DeferredRun { workflow_id: wf.id, document_id: doc.row.id, trigger_type });
            return Ok(());
        }
        Decision::Skip { code, .. } => {
            // Schedule: "AI says no" counts as done (ngx WorkflowRun), otherwise every
            // check round would ask the model again (Known difference, docs/contract/automation.md).
            if trigger_type == pl_matching::workflow::TRIGGER_SCHEDULED && *code == "ai_not_met" {
                record_run(conn, wf.id, trigger_type, Some(doc.row.id)).await?;
            }
            if let Some(r) = workflow_ext::record_for(wf, trigger_type, Some(doc.row.id), &decision) {
                workflow_ext::insert_run(conn, &r).await?;
                outcome.runs.push(r);
            }
            return Ok(());
        }
    };
    let actions = workflow_ext::branch_actions(wf, branch);
    let before = doc.row.clone();
    let mut trash = false;
    for action in actions {
        tracing::info!(target: "paperless.handlers", "Applying {} from Workflow: {}", action.display(), wf.name);
        match action.action_type {
            ACTION_ASSIGNMENT => apply_assignment_to_document(conn, env.audit, action, doc, env.time_zone).await?,
            ACTION_REMOVAL => apply_removal_to_document(conn, env.audit, action, doc).await?,
            ACTION_EMAIL => env.effects.email_in(conn, &document_input(action, doc, original_file, trigger_type)).await,
            ACTION_WEBHOOK => env.effects.webhook_in(conn, &document_input(action, doc, original_file, trigger_type)).await,
            ACTION_PASSWORD_REMOVAL => {
                env.effects.password_removal_in(conn, &document_input(action, doc, original_file, trigger_type)).await;
            }
            ACTION_MOVE_TO_TRASH => trash = true,
            ACTION_REMOTE_OCR => tracing::debug!(
                target: "paperless.handlers",
                "Remote OCR action only applies to consumption triggers, ignoring"
            ),
            // Queued rather than run sync (delay_on_commit)
            ACTION_APPLY_AI_SUGGESTIONS => outcome.ai_suggestions.push((action.id, doc.row.id)),
            _ => {}
        }
    }
    doc.row.title = doc.row.title.chars().take(128).collect();
    docops::save(conn, env.audit, doc, Some(&["title", "correspondent", "document_type", "storage_path", "owner", "modified"])).await?;
    // post_save → update_filename_and_move_files
    if let Some(s) = env.settings {
        crate::files::update_filename_and_move_files(conn, s, doc.row.id).await?;
    }
    record_run(conn, wf.id, trigger_type, Some(doc.row.id)).await?;
    if let Some(mut r) = workflow_ext::record_for(wf, trigger_type, Some(doc.row.id), &decision) {
        r.changed_fields = workflow_ext::changed_fields(&before, Some(&doc.row), actions);
        workflow_ext::insert_run(conn, &r).await?;
        outcome.runs.push(r);
    }
    if trash {
        env.deleter.soft_delete(conn, env.audit, doc.row.id).await?;
        tracing::debug!(target: "paperless.workflows", "Moved document {} to trash", doc.row.id);
    }
    Ok(())
}

/// `apply_assignment_to_document`.
async fn apply_assignment_to_document(
    conn: &mut PgConnection,
    audit: &AuditLog,
    action: &ActionDef,
    doc: &mut DocInstance,
    time_zone: chrono_tz::Tz,
) -> Result<(), sqlx::Error> {
    if !action.assign_tags.is_empty() {
        // on a freshly loaded instance (Document.objects.get(pk=...).add_nested_tags)
        if let Some(fresh) = db::load_document(conn, doc.row.id).await? {
            docops::add_nested_tags(conn, audit, &DocInstance::from_db(fresh), &action.assign_tags).await?;
        }
    }
    if let Some(c) = action.assign_correspondent {
        doc.row.correspondent_id = Some(c);
    }
    if let Some(t) = action.assign_document_type {
        doc.row.document_type_id = Some(t);
    }
    if let Some(s) = action.assign_storage_path {
        doc.row.storage_path_id = Some(s);
    }
    if let Some(u) = action.assign_owner {
        doc.row.owner_id = Some(u);
    }
    if !action.assign_custom_fields.is_empty() {
        let defs = db::custom_fields(conn, &action.assign_custom_fields).await?;
        for f in defs {
            let value = action.cf_value(f.id);
            let existing = existing_cf_instance(conn, doc.row.id, f.id).await?;
            let empty = value.is_null() || value.as_str() == Some("");
            match existing {
                Some(inst_id) if !empty => {
                    if let Some(col_name) = db::value_field_name(&f.data_type)
                        && let Ok(expr) = db::custom_value_expr(&f.data_type, &value)
                    {
                        // W7: `instance.save()` → auditlog `log_update` (old value from the DB)
                        let old: Option<Value> =
                            sqlx::query_scalar(&format!("SELECT to_jsonb({col_name}) FROM documents_customfieldinstance WHERE id = $1"))
                                .bind(inst_id)
                                .fetch_optional(&mut *conn)
                                .await?
                                .flatten();
                        let q = Query::update()
                            .table(a("documents_customfieldinstance"))
                            .value(a(col_name), expr)
                            .and_where(Expr::col(a("id")).eq(inst_id))
                            .to_owned();
                        let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
                        sqlx::query_with(&sql, v).execute(&mut *conn).await?;
                        let repr =
                            format!("{} : {}", f.name, crate::audit::custom_value_for_search(&f.data_type, &value, f.extra_data.as_ref()));
                        audit
                            .custom_field_instance_updated(
                                conn,
                                inst_id,
                                col_name,
                                old.and_then(|o| crate::audit::custom_value_smart_str(&f.data_type, &o)),
                                crate::audit::custom_value_smart_str(&f.data_type, &value),
                                &repr,
                            )
                            .await?;
                    }
                }
                Some(_) => {}
                None => {
                    crate::consumer::create_custom_field_instance(conn, audit, doc.row.id, &f, &value).await?;
                }
            }
        }
    }
    if let Some(t) = action.assign_title.as_ref().filter(|t| !t.is_empty()) {
        let corr = match doc.row.correspondent_id {
            Some(id) => db::object_name(conn, "documents_correspondent", id).await?,
            None => None,
        };
        let dtype = match doc.row.document_type_id {
            Some(id) => db::object_name(conn, "documents_documenttype", id).await?,
            None => None,
        };
        let owner = match doc.row.owner_id {
            Some(id) => db::username(conn, id).await?,
            None => None,
        };
        let p = pl_templating::WorkflowPlaceholders {
            correspondent_name: Some(corr.unwrap_or_default()),
            doc_type_name: Some(dtype.unwrap_or_default()),
            owner_username: Some(owner.unwrap_or_default()),
            local_added: doc.row.added.with_timezone(&time_zone).fixed_offset(),
            original_filename: doc.row.original_filename.clone().unwrap_or_default(),
            filename: doc.row.filename.clone().unwrap_or_default(),
            created: Some(doc.row.created),
            doc_title: Some(String::new()),
            doc_url: Some(String::new()),
            doc_id: Some(doc.row.id),
            custom_fields: crate::templating::custom_field_instances(conn, doc.row.id).await?,
        };
        if let Some(title) = pl_templating::render_workflow_title(t, &p) {
            doc.row.title = title;
        }
    }
    if action.has_assign_perms() {
        let spec = pl_authz::owned::PermissionsSpec {
            view: Some(pl_authz::owned::GranteesUpdate {
                users: Some(action.assign_view_users.clone()),
                groups: Some(action.assign_view_groups.clone()),
            }),
            change: Some(pl_authz::owned::GranteesUpdate {
                users: Some(action.assign_change_users.clone()),
                groups: Some(action.assign_change_groups.clone()),
            }),
        };
        pl_authz::owned::set_permissions_for_objects(conn, &spec, &pl_authz::models::DOCUMENT, &[doc.row.id], true)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("{e:?}")))?;
    }
    Ok(())
}

async fn existing_cf_instance(conn: &mut PgConnection, doc_id: i64, field_id: i64) -> Result<Option<i64>, sqlx::Error> {
    let q = Query::select()
        .column(a("id"))
        .from(a("documents_customfieldinstance"))
        .and_where(Expr::col(a("document_id")).eq(doc_id))
        .and_where(Expr::col(a("field_id")).eq(field_id))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .order_by(a("id"), Order::Asc)
        .limit(1)
        .to_owned();
    Ok(db::ids(conn, &q).await?.into_iter().next())
}

/// Action that the bundled follow-up can reproduce without caching the document row
/// ([`apply_simple_action`]): assigning/removing tags, correspondent, document type,
/// storage path and owner. Title (template), permissions, custom fields, mail, webhook, trash,
/// AI → no (then exact single path).
pub fn is_simple_action(action: &ActionDef) -> bool {
    match action.action_type {
        ACTION_ASSIGNMENT => {
            action.assign_custom_fields.is_empty() && action.assign_title.as_deref().is_none_or(str::is_empty) && !action.has_assign_perms()
        }
        ACTION_REMOVAL => {
            !action.remove_all_permissions
                && !action.has_remove_perms()
                && !action.remove_all_custom_fields
                && action.remove_custom_fields.is_empty()
        }
        _ => false,
    }
}

/// [`apply_assignment_to_document`] / [`apply_removal_to_document`] for a
/// [simple](is_simple_action) action, where `stored` replaces the state that upstream reads
/// fresh from the DB with `Document.objects.get(pk=…)` (the bundled follow-up writes the
/// document row only at the end). Tag changes and their audit are written immediately as
/// there; same order, same functions.
pub(crate) async fn apply_simple_action(
    conn: &mut PgConnection,
    audit: &AuditLog,
    action: &ActionDef,
    doc: &mut DocInstance,
    stored: &db::DocRow,
) -> Result<(), sqlx::Error> {
    let fresh = || DocInstance::from_db(stored.clone());
    match action.action_type {
        ACTION_ASSIGNMENT => {
            if !action.assign_tags.is_empty() {
                docops::add_nested_tags(conn, audit, &fresh(), &action.assign_tags).await?;
            }
            if let Some(c) = action.assign_correspondent {
                doc.row.correspondent_id = Some(c);
            }
            if let Some(t) = action.assign_document_type {
                doc.row.document_type_id = Some(t);
            }
            if let Some(s) = action.assign_storage_path {
                doc.row.storage_path_id = Some(s);
            }
            if let Some(u) = action.assign_owner {
                doc.row.owner_id = Some(u);
            }
        }
        ACTION_REMOVAL => {
            if action.remove_all_tags {
                docops::clear_tags(conn, audit, &fresh()).await?;
            } else {
                let mut remove = Vec::new();
                for t in &action.remove_tags {
                    remove.push(*t);
                    remove.extend(db::tag_descendants(conn, *t).await?);
                }
                remove.sort_unstable();
                remove.dedup();
                if !remove.is_empty() {
                    docops::remove_tags(conn, audit, &fresh(), &remove).await?;
                }
            }
            let r = &mut doc.row;
            if action.remove_all_correspondents || r.correspondent_id.is_some_and(|c| action.remove_correspondents.contains(&c)) {
                r.correspondent_id = None;
            }
            if action.remove_all_document_types || r.document_type_id.is_some_and(|c| action.remove_document_types.contains(&c)) {
                r.document_type_id = None;
            }
            if action.remove_all_storage_paths || r.storage_path_id.is_some_and(|c| action.remove_storage_paths.contains(&c)) {
                r.storage_path_id = None;
            }
            if action.remove_all_owners || r.owner_id.is_some_and(|c| action.remove_owners.contains(&c)) {
                r.owner_id = None;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `apply_removal_to_document`.
async fn apply_removal_to_document(
    conn: &mut PgConnection,
    audit: &AuditLog,
    action: &ActionDef,
    doc: &mut DocInstance,
) -> Result<(), sqlx::Error> {
    if action.remove_all_tags {
        if let Some(fresh) = db::load_document(conn, doc.row.id).await? {
            docops::clear_tags(conn, audit, &DocInstance::from_db(fresh)).await?;
        }
    } else {
        let mut remove = Vec::new();
        for t in &action.remove_tags {
            remove.push(*t);
            remove.extend(db::tag_descendants(conn, *t).await?);
        }
        remove.sort_unstable();
        remove.dedup();
        if !remove.is_empty()
            && let Some(fresh) = db::load_document(conn, doc.row.id).await?
        {
            docops::remove_tags(conn, audit, &DocInstance::from_db(fresh), &remove).await?;
        }
    }
    let r = &mut doc.row;
    if action.remove_all_correspondents || r.correspondent_id.is_some_and(|c| action.remove_correspondents.contains(&c)) {
        r.correspondent_id = None;
    }
    if action.remove_all_document_types || r.document_type_id.is_some_and(|c| action.remove_document_types.contains(&c)) {
        r.document_type_id = None;
    }
    if action.remove_all_storage_paths || r.storage_path_id.is_some_and(|c| action.remove_storage_paths.contains(&c)) {
        r.storage_path_id = None;
    }
    if action.remove_all_owners || r.owner_id.is_some_and(|c| action.remove_owners.contains(&c)) {
        r.owner_id = None;
    }
    let doc_id = r.id;
    if action.remove_all_permissions {
        let spec = pl_authz::owned::PermissionsSpec {
            view: Some(pl_authz::owned::GranteesUpdate { users: Some(vec![]), groups: Some(vec![]) }),
            change: Some(pl_authz::owned::GranteesUpdate { users: Some(vec![]), groups: Some(vec![]) }),
        };
        pl_authz::owned::set_permissions_for_objects(conn, &spec, &pl_authz::models::DOCUMENT, &[doc_id], false)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("{e:?}")))?;
    }
    if action.has_remove_perms() {
        for (ids, table, ident, codename) in [
            (&action.remove_view_users, "guardian_userobjectpermission", "user_id", "view_document"),
            (&action.remove_change_users, "guardian_userobjectpermission", "user_id", "change_document"),
            (&action.remove_view_groups, "guardian_groupobjectpermission", "group_id", "view_document"),
            (&action.remove_change_groups, "guardian_groupobjectpermission", "group_id", "change_document"),
        ] {
            if ids.is_empty() {
                continue;
            }
            let perm =
                Query::select().column(a("id")).from(a("auth_permission")).and_where(Expr::col(a("codename")).eq(codename)).to_owned();
            let ct = Query::select()
                .column(a("id"))
                .from(a("django_content_type"))
                .and_where(Expr::col(a("app_label")).eq("documents"))
                .and_where(Expr::col(a("model")).eq("document"))
                .to_owned();
            let d = Query::delete()
                .from_table(a(table))
                .and_where(Expr::col(a("object_pk")).eq(doc_id.to_string()))
                .and_where(Expr::col(a("permission_id")).in_subquery(perm))
                .and_where(Expr::col(a("content_type_id")).in_subquery(ct))
                .and_where(Expr::col(a(ident)).is_in(ids.iter().map(|i| *i as i32)))
                .to_owned();
            let (sql, v) = d.build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(&sql, v).execute(&mut *conn).await?;
        }
    }
    if action.remove_all_custom_fields || !action.remove_custom_fields.is_empty() {
        let mut d = Query::delete();
        d.from_table(a("documents_customfieldinstance")).and_where(Expr::col(a("document_id")).eq(doc_id));
        if !action.remove_all_custom_fields {
            d.and_where(Expr::col(a("field_id")).is_in(action.remove_custom_fields.iter().map(|i| *i as i32)));
        }
        let (sql, v) = d.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K-N1: triggers with `filter_custom_field_query` are checked with pre-evaluated hits
    /// (previously never executed without an evaluator); invalid query → no hits.
    #[test]
    fn custom_field_query_evaluator_matches() {
        use pl_matching::workflow::{TRIGGER_DOCUMENT_UPDATED, existing_document_matches_workflow, prefilter_matches};
        let q = r#"["AND", [["f", "exact", "x"]]]"#;
        let trig =
            WorkflowTrigger { trigger_type: TRIGGER_DOCUMENT_UPDATED, filter_custom_field_query: Some(q.into()), ..Default::default() };
        let doc = |id| ExistingDocument { id, ..Default::default() };
        let ev = CustomFieldQueries::from_parts(vec![(q, Some(vec![1]))]);
        assert_eq!(existing_document_matches_workflow(&doc(1), &trig, Some(&ev)).unwrap(), (true, None));
        let (m, r) = existing_document_matches_workflow(&doc(2), &trig, Some(&ev)).unwrap();
        assert!(!m);
        assert_eq!(r.as_deref(), Some("Document custom fields do not match the configured custom field query"));
        assert!(prefilter_matches(&doc(1), &trig, Some(&ev)));
        assert!(!prefilter_matches(&doc(2), &trig, Some(&ev)));
        let invalid = CustomFieldQueries::from_parts(vec![(q, None)]);
        let (m, r) = existing_document_matches_workflow(&doc(1), &trig, Some(&invalid)).unwrap();
        assert!(!m);
        assert_eq!(r.as_deref(), Some("Invalid custom field query configuration"));
        assert!(!prefilter_matches(&doc(1), &trig, Some(&invalid)));
    }

    #[test]
    fn union_sorted() {
        assert_eq!(union(&Some(vec![3, 1]), &[2, 3]), Some(vec![1, 2, 3]));
        assert_eq!(union(&None, &[]), Some(vec![]));
    }

    #[test]
    fn cf_value_lookup() {
        let a = ActionDef { assign_custom_fields_values: Some(serde_json::json!({"3": "x"})), ..Default::default() };
        assert_eq!(a.cf_value(3), Value::from("x"));
        assert_eq!(a.cf_value(4), Value::Null);
    }
}
