//! Dry run (`POST /dry-run`) and "apply retroactively" (`POST /workflows/{id}/apply`).
//!
//! The dry run saves a draft (if sent along) with the same functions as the real
//! save in a transaction, loads it with the loader of the real execution
//! (`pl_consumer::workflows::workflow_by_id`) and runs each document through the same core
//! (`pl_consumer::workflow_dryrun::dry_run`). At the end the transaction is rolled back – nothing
//! remains, nothing goes outside.

use std::collections::BTreeSet;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use pl_authz::User;
use pl_consumer::workflow_dryrun::{DocState, DryRunDoc, DryRunOptions};
use pl_consumer::workflow_ext::{self, AiMode};
use pl_consumer::workflows::WorkflowDef;
use pl_core::AppState;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};

use crate::common::{self, ADD_WORKFLOW, CHANGE_DOCUMENT, CHANGE_WORKFLOW, NameKind, Names, VIEW_DOCUMENT, VIEW_WORKFLOW, Who};
use crate::error::{AErr, AResult, json as json_resp};

/// Default and maximum number of checked documents.
pub const DEFAULT_LIMIT: u64 = 50;
pub const MAX_LIMIT: u64 = 200;
/// Maximum number with AI condition (`with_ai`: one model call per document).
pub const MAX_WITH_AI: usize = 10;

/// Trigger type for dry run/apply: requested (must be on the workflow) or the first trigger
/// that is not an ingestion trigger, otherwise ingestion.
pub fn pick_trigger(wf: &WorkflowDef, requested: Option<i64>) -> AResult<i64> {
    if wf.triggers.is_empty() {
        return Err(AErr::field("trigger_type", "The workflow has no trigger."));
    }
    match requested {
        Some(t) if wf.fires_on(t) => Ok(t),
        Some(_) => Err(AErr::field("trigger_type", "The workflow has no trigger of this type.")),
        None => Ok(wf.triggers.iter().map(|t| t.trigger_type).find(|t| *t != 1).unwrap_or(1)),
    }
}

/// POST /dry-run
pub async fn dry_run(State(st): State<AppState>, who: Who, headers: HeaderMap, body: Bytes) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW, VIEW_DOCUMENT])?;
    let b = common::body_object(&headers, &body)?;
    let draft = match b.get("workflow") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) => Some(m.clone()),
        Some(_) => return Err(AErr::field("workflow", "Expected an object.")),
    };
    let wf_id = common::opt_int(&b, "workflow_id")?;
    if draft.is_some() {
        who.require_any(&[ADD_WORKFLOW, CHANGE_WORKFLOW])?;
    } else if wf_id.is_none() {
        return Err(AErr::field("workflow_id", "Either workflow_id or workflow is required."));
    }
    let limit = common::opt_int(&b, "limit")?.unwrap_or(DEFAULT_LIMIT as i64);
    if !(1..=MAX_LIMIT as i64).contains(&limit) {
        return Err(AErr::field("limit", &format!("Ensure this value is between 1 and {MAX_LIMIT}.")));
    }
    let explicit = common::int_list(&b, "document_ids")?;
    if explicit.as_ref().is_some_and(|v| v.len() > MAX_LIMIT as usize) {
        return Err(AErr::field("document_ids", &format!("At most {MAX_LIMIT} documents.")));
    }
    let with_ai = common::opt_bool(&b, "with_ai")?.unwrap_or(false);
    let source = common::opt_int(&b, "source")?;
    if source.is_some_and(|s| !(1..=4).contains(&s)) {
        return Err(AErr::field("source", "\"source\" is not a valid choice."));
    }
    let mail_rule = common::opt_int(&b, "mail_rule")?;
    let requested_trigger = common::opt_int(&b, "trigger_type")?;

    workflow_ext::ensure_pool(st.db()).await?;
    let mut tx = st.db().begin().await?;
    // Dry run must never wait long for locks (real runs take precedence)
    sqlx::query("SET LOCAL lock_timeout = '3s'").execute(&mut *tx).await?;
    sqlx::query("SET LOCAL statement_timeout = '30s'").execute(&mut *tx).await?;
    let id = match draft {
        Some(mut d) => {
            // Name is irrelevant for the dry run; a unique placeholder avoids
            // name conflicts with the original (only within this transaction).
            d.insert("name".into(), Value::String(format!("__paperrust_dry_run_{}__", uuid::Uuid::new_v4().simple())));
            let existing = d.get("id").and_then(Value::as_i64).filter(|_| who.user().has_perm(CHANGE_WORKFLOW));
            let existing = match existing {
                Some(e) if pl_workflows::load::get_row(&mut tx, pl_workflows::schema::T_WORKFLOW, e).await?.is_some() => Some(e),
                _ => None,
            };
            d.remove("id");
            crate::workflows::save(&mut tx, d, existing, false).await?
        }
        None => wf_id.unwrap_or(0),
    };
    let wf = pl_consumer::workflows::workflow_by_id(&mut tx, id).await?.ok_or(AErr::NotFound)?;
    let trigger = pick_trigger(&wf, requested_trigger)?;
    let docs = match explicit {
        Some(ids) => {
            let ok = common::permitted_docs(&mut tx, user, "view_document", &ids).await?;
            ids.into_iter().filter(|i| ok.contains(i)).collect::<Vec<_>>()
        }
        None => common::latest_visible_docs(&mut tx, user, limit as u64).await?,
    };
    let (ai_available, ai_enabled) = crate::summary::ai_status().await;
    let ai_checked = with_ai && wf.ai_condition.is_some() && ai_available && ai_enabled;
    if ai_checked && docs.len() > MAX_WITH_AI {
        return Err(AErr::field("with_ai", &format!("With the AI condition at most {MAX_WITH_AI} documents can be checked.")));
    }
    let opts = DryRunOptions {
        trigger_type: trigger,
        ai_mode: if ai_checked { AiMode::Inline } else { AiMode::NotChecked },
        time_zone: *st.tz(),
        source,
        mailrule_id: mail_rule,
    };
    let results = pl_consumer::workflow_dryrun::dry_run(&mut tx, &wf, &docs, &opts).await?;
    // Title/date for the list (before the rollback, same state)
    let meta = doc_meta(&mut tx, &docs).await?;
    tx.rollback().await?;

    let mut conn = st.db().acquire().await?;
    let body = render_results(&mut conn, user, &wf, trigger, &results, &meta, ai_checked, (ai_available, ai_enabled), draft_is(&b)).await?;
    Ok(json_resp(StatusCode::OK, &body))
}

fn draft_is(b: &Map<String, Value>) -> bool {
    b.get("workflow").is_some_and(Value::is_object)
}

async fn doc_meta(conn: &mut PgConnection, ids: &[i64]) -> AResult<std::collections::HashMap<i64, (String, String)>> {
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let rows = sqlx::query("SELECT id, title, added FROM documents_document WHERE id = ANY($1)").bind(&ids32).fetch_all(&mut *conn).await?;
    let mut out = std::collections::HashMap::new();
    for r in rows {
        let added: chrono::DateTime<chrono::Utc> = r.try_get(2)?;
        out.insert(
            i64::from(r.try_get::<i32, _>(0)?),
            (r.try_get::<String, _>(1)?, added.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
    }
    Ok(out)
}

fn want_state(names: &mut Names, s: &DocState) {
    for t in &s.tags {
        names.want(NameKind::Tag, *t);
    }
    if let Some(c) = s.correspondent {
        names.want(NameKind::Correspondent, c);
    }
    if let Some(c) = s.document_type {
        names.want(NameKind::DocumentType, c);
    }
    if let Some(c) = s.storage_path {
        names.want(NameKind::StoragePath, c);
    }
    if let Some(c) = s.owner {
        names.want(NameKind::User, c);
    }
    for f in s.custom_fields.keys() {
        names.want(NameKind::CustomField, *f);
    }
    for u in s.view_users.iter().chain(&s.change_users) {
        names.want(NameKind::User, *u);
    }
    for g in s.view_groups.iter().chain(&s.change_groups) {
        names.want(NameKind::Group, *g);
    }
}

fn obj(names: &Names, k: NameKind, id: Option<i64>) -> Value {
    match id {
        Some(id) => json!({"id": id, "name": names.get(k, id)}),
        None => Value::Null,
    }
}

fn objs(names: &Names, k: NameKind, ids: &[i64]) -> Value {
    Value::Array(ids.iter().map(|id| json!({"id": id, "name": names.get(k, *id)})).collect())
}

fn state_json(names: &Names, s: &DocState) -> Value {
    json!({
        "title": s.title,
        "correspondent": obj(names, NameKind::Correspondent, s.correspondent),
        "document_type": obj(names, NameKind::DocumentType, s.document_type),
        "storage_path": obj(names, NameKind::StoragePath, s.storage_path),
        "owner": obj(names, NameKind::User, s.owner),
        "tags": objs(names, NameKind::Tag, &s.tags),
        "custom_fields": s.custom_fields.iter().map(|(f, v)| json!({"field": {"id": f, "name": names.get(NameKind::CustomField, *f)}, "value": v})).collect::<Vec<_>>(),
        "view_users": objs(names, NameKind::User, &s.view_users),
        "view_groups": objs(names, NameKind::Group, &s.view_groups),
        "change_users": objs(names, NameKind::User, &s.change_users),
        "change_groups": objs(names, NameKind::Group, &s.change_groups),
        "in_trash": s.in_trash,
    })
}

/// Differences before/after (field name, before, after; lists as added/removed).
pub fn changes(names: &Names, b: &DocState, a: &DocState) -> Vec<Value> {
    let mut out = Vec::new();
    if a.title != b.title {
        out.push(json!({"field": "title", "before": b.title, "after": a.title}));
    }
    for (field, k, x, y) in [
        ("correspondent", NameKind::Correspondent, b.correspondent, a.correspondent),
        ("document_type", NameKind::DocumentType, b.document_type, a.document_type),
        ("storage_path", NameKind::StoragePath, b.storage_path, a.storage_path),
        ("owner", NameKind::User, b.owner, a.owner),
    ] {
        if x != y {
            out.push(json!({"field": field, "before": obj(names, k, x), "after": obj(names, k, y)}));
        }
    }
    for (field, k, x, y) in [
        ("tags", NameKind::Tag, &b.tags, &a.tags),
        ("view_users", NameKind::User, &b.view_users, &a.view_users),
        ("view_groups", NameKind::Group, &b.view_groups, &a.view_groups),
        ("change_users", NameKind::User, &b.change_users, &a.change_users),
        ("change_groups", NameKind::Group, &b.change_groups, &a.change_groups),
    ] {
        let bs: BTreeSet<i64> = x.iter().copied().collect();
        let as_: BTreeSet<i64> = y.iter().copied().collect();
        let added: Vec<i64> = as_.difference(&bs).copied().collect();
        let removed: Vec<i64> = bs.difference(&as_).copied().collect();
        if !added.is_empty() || !removed.is_empty() {
            out.push(json!({"field": field, "added": objs(names, k, &added), "removed": objs(names, k, &removed)}));
        }
    }
    let keys: BTreeSet<i64> = b.custom_fields.keys().chain(a.custom_fields.keys()).copied().collect();
    for f in keys {
        let (x, y) = (b.custom_fields.get(&f), a.custom_fields.get(&f));
        if x != y {
            out.push(json!({
                "field": "custom_field",
                "custom_field": {"id": f, "name": names.get(NameKind::CustomField, f)},
                "before": x.cloned().unwrap_or(Value::Null),
                "after": y.cloned().unwrap_or(Value::Null),
                "removed": y.is_none(),
            }));
        }
    }
    if a.in_trash != b.in_trash {
        out.push(json!({"field": "in_trash", "before": b.in_trash, "after": a.in_trash}));
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn render_results(
    conn: &mut PgConnection,
    user: &User,
    wf: &WorkflowDef,
    trigger: i64,
    results: &[DryRunDoc],
    meta: &std::collections::HashMap<i64, (String, String)>,
    ai_checked: bool,
    ai_state: (bool, bool),
    draft: bool,
) -> AResult<Value> {
    let mut names = Names::default();
    for r in results {
        for s in r.before.iter().chain(r.after.iter()) {
            want_state(&mut names, s);
        }
    }
    names.resolve(conn, user).await?;
    let mut counts = Map::new();
    for k in ["main", "else", "skipped", "error"] {
        counts.insert(k.into(), json!(results.iter().filter(|r| r.result == k).count()));
    }
    counts.insert("checked".into(), json!(results.len()));
    let docs: Vec<Value> = results
        .iter()
        .map(|r| {
            let (title, added) = meta.get(&r.document_id).cloned().unwrap_or_default();
            let ch = match (&r.before, &r.after) {
                (Some(b), Some(a)) => changes(&names, b, a),
                _ => Vec::new(),
            };
            json!({
                "id": r.document_id,
                "title": title,
                "added": added,
                "result": r.result,
                "code": r.code,
                "reason": r.reason,
                "changes": ch,
                "effects": r.effects,
                "before": r.before.as_ref().map(|s| state_json(&names, s)),
                "after": r.after.as_ref().map(|s| state_json(&names, s)),
            })
        })
        .collect();
    let mut notes: Vec<&str> = Vec::new();
    if trigger == 1 {
        notes.push("consumption_approximate");
    }
    if trigger == 4 {
        notes.push("schedule_window_not_checked");
    }
    if wf.ai_condition.is_some() && !ai_checked {
        notes.push(if !ai_state.0 || !ai_state.1 { "ai_condition_ignored" } else { "ai_condition_not_checked" });
    }
    Ok(json!({
        "workflow_id": if draft { Value::Null } else { json!(wf.id) },
        "trigger_type": trigger,
        "trigger_key": crate::summary::trigger_key(trigger),
        "approximate": trigger == 1,
        "ai": {"available": ai_state.0, "enabled": ai_state.1, "checked": ai_checked},
        "notes": notes,
        "counts": counts,
        "documents": docs,
    }))
}

/// POST /workflows/{id}/apply – retroactively on selected documents (real execution in the worker
/// via the task `run_workflows_for_documents`, PaperRust extension `match_trigger`).
pub async fn apply(
    State(st): State<AppState>,
    who: Who,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AErr> {
    let user = who.require(&[VIEW_WORKFLOW, CHANGE_DOCUMENT])?;
    let id = common::path_id(&id)?;
    let b = common::body_object(&headers, &body)?;
    let ids = common::int_list(&b, "document_ids")?.ok_or_else(|| AErr::field("document_ids", "This field is required."))?;
    if ids.is_empty() {
        return Err(AErr::field("document_ids", "This list may not be empty."));
    }
    if ids.len() > MAX_LIMIT as usize {
        return Err(AErr::field("document_ids", &format!("At most {MAX_LIMIT} documents.")));
    }
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let wf = pl_consumer::workflows::workflow_by_id(&mut conn, id).await?.ok_or(AErr::NotFound)?;
    let trigger = pick_trigger(&wf, common::opt_int(&b, "trigger_type")?)?;
    // like bulk_edit: all documents must be changeable
    let ok = common::permitted_docs(&mut conn, user, "change_document", &ids).await?;
    if ids.iter().any(|i| !ok.contains(i)) {
        return Err(AErr::Forbidden("Insufficient permissions.".into()));
    }
    // Security review 2026-11-19 (F1): like bulk_edit `run_workflow` – actions set owner/permissions
    // and can move to the trash → only owner of all documents (or superuser), only
    // active workflows; triggers other than "Manual" additionally only with `change_workflow`.
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM documents_workflow WHERE id = $1")
        .bind(i32::try_from(id).unwrap_or(i32::MAX))
        .fetch_optional(&mut *conn)
        .await?;
    let owns = user.is_superuser || common::owns_all_docs(&mut conn, user, &ids).await?;
    apply_gate(owns, enabled.unwrap_or(false), trigger, user.has_perm(CHANGE_WORKFLOW))?;
    let (filter_documents, match_trigger) = apply_mode(trigger);
    let mut kw = Map::new();
    kw.insert("document_ids".into(), json!(ids));
    kw.insert("workflow_id".into(), json!(id));
    kw.insert("filter_documents".into(), json!(filter_documents));
    if let Some(t) = match_trigger {
        kw.insert("match_trigger".into(), json!(t));
    }
    let mut req = pl_core::tasks::TaskRequest::new(pl_consumer::manual::RUN_WORKFLOWS_FOR_DOCUMENTS)
        .trigger_source(pl_core::tasks::trigger_source::MANUAL);
    req.kwargs = kw;
    let prepared = pl_core::tasks::prepare_task(&st, req).map_err(|_| AErr::Unavailable("No task queue available.".into()))?;
    let task_id = prepared.task_id;
    pl_core::tasks::record_task(st.db(), &prepared.req, task_id, chrono::Utc::now())
        .await
        .map_err(|e| AErr::Internal(format!("record task: {e:?}")))?;
    prepared.publish(&st).await.map_err(|e| AErr::Internal(format!("publish task: {e:?}")))?;
    Ok(json_resp(
        StatusCode::ACCEPTED,
        &json!({"task_id": task_id, "documents": ids.len(), "trigger_type": trigger, "filter_documents": filter_documents}),
    ))
}

/// Additional conditions for `apply` (F1): ownership, active workflow, `change_workflow` except for "Manual".
pub fn apply_gate(owns_all: bool, enabled: bool, trigger: i64, change_workflow: bool) -> AResult<()> {
    if !owns_all {
        return Err(AErr::Forbidden("Insufficient permissions.".into()));
    }
    if !enabled {
        return Err(AErr::field("workflow", "Workflow is not enabled"));
    }
    if trigger != 5 && !change_workflow {
        return Err(AErr::Forbidden("Insufficient permissions to run this workflow with this trigger.".into()));
    }
    Ok(())
}

/// (`filter_documents`, `match_trigger`): ingestion triggers do not exist for existing documents →
/// actions without filter (the selection comes from the dry run); otherwise filter of the trigger type.
pub fn apply_mode(trigger: i64) -> (bool, Option<i64>) {
    if trigger == 1 { (false, None) } else { (true, Some(trigger)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review F1: non-owners, disabled workflows and foreign triggers without
    /// `change_workflow` are rejected.
    #[test]
    fn apply_gate_rules() {
        assert!(matches!(apply_gate(false, true, 5, true), Err(AErr::Forbidden(_))));
        assert!(matches!(apply_gate(true, false, 5, true), Err(AErr::Invalid { .. })));
        assert!(matches!(apply_gate(true, true, 1, false), Err(AErr::Forbidden(_))));
        assert!(matches!(apply_gate(true, true, 3, false), Err(AErr::Forbidden(_))));
        assert!(apply_gate(true, true, 5, false).is_ok());
        assert!(apply_gate(true, true, 1, true).is_ok());
    }
    use pl_matching::workflow::WorkflowTrigger;

    fn wf(types: &[i64]) -> WorkflowDef {
        WorkflowDef {
            id: 1,
            triggers: types.iter().map(|t| WorkflowTrigger { trigger_type: *t, ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn trigger_choice() {
        assert_eq!(pick_trigger(&wf(&[1, 3]), None).unwrap(), 3);
        assert_eq!(pick_trigger(&wf(&[1]), None).unwrap(), 1);
        assert_eq!(pick_trigger(&wf(&[2, 4]), Some(4)).unwrap(), 4);
        assert!(pick_trigger(&wf(&[2]), Some(3)).is_err());
        assert!(pick_trigger(&wf(&[]), None).is_err());
        assert_eq!(apply_mode(1), (false, None));
        assert_eq!(apply_mode(3), (true, Some(3)));
    }

    #[test]
    fn diff_lists_and_fields() {
        let names = Names::default();
        let b = DocState { title: "a".into(), tags: vec![1, 2], correspondent: Some(1), ..Default::default() };
        let mut a = b.clone();
        a.title = "b".into();
        a.tags = vec![2, 3];
        a.correspondent = None;
        a.custom_fields.insert(5, json!({"value_text": "x"}));
        let c = changes(&names, &b, &a);
        let fields: Vec<&str> = c.iter().map(|x| x["field"].as_str().unwrap()).collect();
        assert_eq!(fields, vec!["title", "correspondent", "tags", "custom_field"]);
        assert_eq!(c[2]["added"][0]["id"], 3);
        assert_eq!(c[2]["removed"][0]["id"], 1);
        assert!(changes(&names, &b, &b).is_empty());
    }
}
