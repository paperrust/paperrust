//! Test run (dry run) of a workflow on existing documents (PaperRust, docs/WORKFLOWS-NEU.md
//! topic 7, contract docs/contract/automation.md "test run").
//!
//! **No second implementation:** every document runs through the same core as the real execution
//! ([`crate::workflows::run_workflow_list_inner`]: filter matching, [`crate::workflow_ext::decide`],
//! actions, saving, history) – inside the caller's transaction in a savepoint
//! that is rolled back afterwards. External effects are only recorded:
//! e-mail/webhook/remove password ([`Recorder`] as `WorkflowEffects`), trash
//! ([`Recorder`] as `DocumentDeleter`), AI suggestions (result `ai_suggestions`, not enqueued),
//! files (no `settings` → no moving). The state before/after is read from the database
//! ([`DocState`]).
//!
//! Ingestion triggers (`CONSUMPTION`) do not really exist for existing documents: matching runs
//! on a reconstructed ingestion document (file name = `original_filename`, source/mail rule from the
//! options), the actions act on the document instead of on the defaults → result `approximate`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use pl_matching::workflow::{ConsumableDocument, DocumentSource, TRIGGER_CONSUMPTION, WorkflowDocument, document_matches_workflow};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, Row};

use crate::audit::AuditLog;
use crate::db;
use crate::docops::DocInstance;
use crate::stages::{ActionContext, ActionInput, BoxFuture, WorkflowEffects};
use crate::workflow_ext::{AiMode, Branch, ExtOptions, MatchState, RunResult};
use crate::workflows::{self, DocumentDeleter, MatchMode, RunEnv, RunOutcome, WorkflowDef};

/// Options of a test run.
#[derive(Debug, Clone)]
pub struct DryRunOptions {
    pub trigger_type: i64,
    pub ai_mode: AiMode,
    pub time_zone: chrono_tz::Tz,
    /// Ingestion trigger only: assumed source (`DocumentSource`); `None` = first source of the trigger.
    pub source: Option<i64>,
    /// Ingestion trigger only: assumed mail rule; `None` = that of the trigger.
    pub mailrule_id: Option<i64>,
}

/// A recorded external effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DryRunEffect {
    pub action_id: Option<i64>,
    /// `email` | `webhook` | `password_removal` | `trash` | `ai_suggestions`
    pub kind: &'static str,
    /// Recipient (e-mail), address (webhook).
    pub target: Option<String>,
}

/// State of a document for before/after.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct DocState {
    pub title: String,
    pub correspondent: Option<i64>,
    pub document_type: Option<i64>,
    pub storage_path: Option<i64>,
    pub owner: Option<i64>,
    pub tags: Vec<i64>,
    /// Field id → value (`{"value_text": …}` without empty columns).
    pub custom_fields: BTreeMap<i64, Value>,
    pub view_users: Vec<i64>,
    pub view_groups: Vec<i64>,
    pub change_users: Vec<i64>,
    pub change_groups: Vec<i64>,
    pub in_trash: bool,
}

/// Result per document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DryRunDoc {
    pub document_id: i64,
    /// `main` | `else` | `skipped` | `error`
    pub result: &'static str,
    pub code: String,
    pub reason: Option<String>,
    pub before: Option<DocState>,
    pub after: Option<DocState>,
    pub effects: Vec<DryRunEffect>,
    pub approximate: bool,
}

/// Records external effects instead of executing them.
#[derive(Default)]
pub struct Recorder {
    effects: Mutex<Vec<DryRunEffect>>,
}

impl Recorder {
    fn push(&self, e: DryRunEffect) {
        if let Ok(mut v) = self.effects.lock() {
            v.push(e);
        }
    }
    fn take(&self) -> Vec<DryRunEffect> {
        self.effects.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

async fn email_target(conn: &mut PgConnection, action_id: i64) -> Option<String> {
    let id = i32::try_from(action_id).ok()?;
    sqlx::query_scalar::<_, String>(
        "SELECT e.\"to\" FROM documents_workflowaction a JOIN documents_workflowactionemail e ON e.id = a.email_id WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .ok()
    .flatten()
}

async fn webhook_target(conn: &mut PgConnection, action_id: i64) -> Option<String> {
    let id = i32::try_from(action_id).ok()?;
    sqlx::query_scalar::<_, String>(
        "SELECT w.url FROM documents_workflowaction a JOIN documents_workflowactionwebhook w ON w.id = a.webhook_id WHERE a.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .ok()
    .flatten()
}

impl WorkflowEffects for Recorder {
    fn email(&self, ctx: &ActionContext) {
        self.push(DryRunEffect { action_id: Some(ctx.workflow_action_id), kind: "email", target: None });
    }
    fn webhook(&self, ctx: &ActionContext) {
        self.push(DryRunEffect { action_id: Some(ctx.workflow_action_id), kind: "webhook", target: None });
    }
    fn password_removal(&self, ctx: &ActionContext) {
        self.push(DryRunEffect { action_id: Some(ctx.workflow_action_id), kind: "password_removal", target: None });
    }
    fn email_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let target = email_target(conn, input.ctx.workflow_action_id).await;
            self.push(DryRunEffect { action_id: Some(input.ctx.workflow_action_id), kind: "email", target });
        })
    }
    fn webhook_in<'a>(&'a self, conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let target = webhook_target(conn, input.ctx.workflow_action_id).await;
            self.push(DryRunEffect { action_id: Some(input.ctx.workflow_action_id), kind: "webhook", target });
        })
    }
    fn password_removal_in<'a>(&'a self, _conn: &'a mut PgConnection, input: &'a ActionInput) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.push(DryRunEffect { action_id: Some(input.ctx.workflow_action_id), kind: "password_removal", target: None });
        })
    }
}

impl DocumentDeleter for Recorder {
    fn soft_delete<'a>(&'a self, _conn: &'a mut PgConnection, _audit: &'a AuditLog, _doc_id: i64) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            self.push(DryRunEffect { action_id: None, kind: "trash", target: None });
            Ok(())
        })
    }
}

/// State of a document (sees the open transaction).
pub async fn doc_state(conn: &mut PgConnection, id: i64) -> Result<Option<DocState>, sqlx::Error> {
    let Ok(id32) = i32::try_from(id) else { return Ok(None) };
    let Some(r) = db::load_document(conn, id).await? else { return Ok(None) };
    let tags: Vec<i32> = sqlx::query_scalar("SELECT tag_id FROM documents_document_tags WHERE document_id = $1::int4 ORDER BY tag_id")
        .bind(id32)
        .fetch_all(&mut *conn)
        .await?;
    let cf = sqlx::query(
        "SELECT field_id, jsonb_strip_nulls(jsonb_build_object('value_text', value_text, 'value_bool', value_bool, \
         'value_url', value_url, 'value_date', value_date, 'value_int', value_int, 'value_float', value_float, \
         'value_monetary', value_monetary, 'value_document_ids', value_document_ids, 'value_select', value_select, \
         'value_long_text', value_long_text)) FROM documents_customfieldinstance \
         WHERE document_id = $1 AND deleted_at IS NULL ORDER BY field_id, id",
    )
    .bind(id32)
    .fetch_all(&mut *conn)
    .await?;
    let mut custom_fields = BTreeMap::new();
    for row in cf {
        custom_fields.insert(i64::from(row.try_get::<i32, _>(0)?), row.try_get::<Value, _>(1)?);
    }
    let pk = id.to_string();
    let mut st = DocState {
        title: r.title.clone(),
        correspondent: r.correspondent_id,
        document_type: r.document_type_id,
        storage_path: r.storage_path_id,
        owner: r.owner_id,
        tags: tags.into_iter().map(i64::from).collect(),
        custom_fields,
        in_trash: r.deleted_at.is_some(),
        ..Default::default()
    };
    for (table, col, view, change) in [
        ("guardian_userobjectpermission", "user_id", 0, 1),
        ("guardian_groupobjectpermission", "group_id", 2, 3),
    ] {
        let sql = format!(
            "SELECT p.codename, x.{col} FROM {table} x JOIN auth_permission p ON p.id = x.permission_id \
             JOIN django_content_type ct ON ct.id = x.content_type_id \
             WHERE ct.app_label = 'documents' AND ct.model = 'document' AND x.object_pk = $1 \
             AND p.codename IN ('view_document', 'change_document') ORDER BY x.{col}"
        );
        for row in sqlx::query(&sql).bind(&pk).fetch_all(&mut *conn).await? {
            let code: String = row.try_get(0)?;
            let who = i64::from(row.try_get::<i32, _>(1)?);
            let slot = if code == "view_document" { view } else { change };
            match slot {
                0 => st.view_users.push(who),
                1 => st.change_users.push(who),
                2 => st.view_groups.push(who),
                _ => st.change_groups.push(who),
            }
        }
    }
    Ok(Some(st))
}

/// Reconstructed ingestion document for matching an ingestion trigger.
pub fn consumable_for(wf: &WorkflowDef, original_filename: Option<&str>, opts: &DryRunOptions) -> ConsumableDocument {
    let first = wf.triggers.iter().find(|t| t.trigger_type == TRIGGER_CONSUMPTION);
    let source = opts
        .source
        .or_else(|| first.and_then(|t| t.sources.first().copied()))
        .and_then(DocumentSource::from_i64)
        .unwrap_or(DocumentSource::ConsumeFolder);
    let name = original_filename.filter(|n| !n.is_empty()).unwrap_or("document");
    ConsumableDocument {
        source,
        original_file: format!("/consume/{name}"),
        original_path: None,
        mailrule_id: opts.mailrule_id.or_else(|| first.and_then(|t| t.filter_mailrule_id)),
    }
}

/// Test run of `wf` on `doc_ids` (order is kept). Runs in the transaction of `conn`; each
/// document in its own savepoint that is rolled back. Error on one document →
/// result `error` for that document, the run continues.
pub async fn dry_run(conn: &mut PgConnection, wf: &WorkflowDef, doc_ids: &[i64], opts: &DryRunOptions) -> Result<Vec<DryRunDoc>, sqlx::Error> {
    let mut out = Vec::with_capacity(doc_ids.len());
    for &id in doc_ids {
        sqlx::query("SAVEPOINT paperrust_dry_run").execute(&mut *conn).await?;
        let r = dry_run_one(conn, wf, id, opts).await;
        sqlx::query("ROLLBACK TO SAVEPOINT paperrust_dry_run").execute(&mut *conn).await?;
        sqlx::query("RELEASE SAVEPOINT paperrust_dry_run").execute(&mut *conn).await?;
        out.push(match r {
            Ok(d) => d,
            Err(e) => DryRunDoc {
                document_id: id,
                result: "error",
                code: "error".into(),
                reason: Some(crate::workflow_ext::error_text(&e)),
                before: None,
                after: None,
                effects: Vec::new(),
                approximate: opts.trigger_type == TRIGGER_CONSUMPTION,
            },
        });
    }
    Ok(out)
}

async fn dry_run_one(conn: &mut PgConnection, wf: &WorkflowDef, id: i64, opts: &DryRunOptions) -> Result<DryRunDoc, sqlx::Error> {
    let approximate = opts.trigger_type == TRIGGER_CONSUMPTION;
    let missing = |code: &str| DryRunDoc {
        document_id: id,
        result: "skipped",
        code: code.into(),
        reason: None,
        before: None,
        after: None,
        effects: Vec::new(),
        approximate,
    };
    let Some(row) = db::load_document(conn, id).await? else { return Ok(missing("document_not_found")) };
    if row.deleted_at.is_some() || row.root_document_id.is_some() {
        return Ok(missing("document_not_eligible"));
    }
    let before = doc_state(conn, id).await?;
    let mode = if approximate {
        let c = consumable_for(wf, row.original_filename.as_deref(), opts);
        MatchMode::Given(match document_matches_workflow(WorkflowDocument::Consumable(&c), &wf.name, &wf.triggers, TRIGGER_CONSUMPTION, None) {
            Ok(true) => MatchState::Matched,
            Ok(false) if !wf.fires_on(TRIGGER_CONSUMPTION) => MatchState::NotFired,
            Ok(false) => MatchState::NotMatched {
                reason: wf
                    .triggers
                    .iter()
                    .filter(|t| t.trigger_type == TRIGGER_CONSUMPTION)
                    .map(|t| pl_matching::workflow::consumable_document_matches_workflow(&c, t).1)
                    .rfind(|r| !r.is_empty()),
            },
            Err(e) => MatchState::Error(format!("{e:?}")),
        })
    } else {
        MatchMode::Evaluate
    };
    let rec = Recorder::default();
    let audit = AuditLog { enabled: true, actor_id: None };
    let env = RunEnv {
        audit: &audit,
        effects: &rec,
        time_zone: opts.time_zone,
        original_file: None,
        settings: None,
        deleter: &rec,
        ext: ExtOptions { ai_mode: opts.ai_mode, log_skips: true },
    };
    let mut doc = DocInstance::from_db(row);
    let outcome: RunOutcome = workflows::run_workflow_list_inner(
        conn,
        &env,
        opts.trigger_type,
        &mut doc,
        std::slice::from_ref(wf),
        PathBuf::new(),
        RunOutcome::default(),
        mode,
    )
    .await?;
    let mut effects = rec.take();
    for (action_id, _) in &outcome.ai_suggestions {
        effects.push(DryRunEffect { action_id: Some(*action_id), kind: "ai_suggestions", target: None });
    }
    let run = outcome.runs.iter().rev().find(|r| r.workflow_id == wf.id);
    let (result, code, reason) = match run {
        Some(r) => {
            let res = match (r.result, r.branch) {
                (RunResult::Ok, Some(Branch::Else)) => "else",
                (RunResult::Ok, _) => "main",
                (RunResult::Error, _) => "error",
                (RunResult::Skipped, _) => "skipped",
            };
            (res, r.code.clone(), Some(r.message.clone()).filter(|m| !m.is_empty()))
        }
        None if !outcome.deferred.is_empty() => ("skipped", "ai_deferred".to_string(), None),
        None => ("skipped", if wf.fires_on(opts.trigger_type) { "conditions_not_met" } else { "not_fired" }.to_string(), None),
    };
    let after = if matches!(result, "main" | "else") { doc_state(conn, id).await? } else { None };
    Ok(DryRunDoc { document_id: id, result, code, reason, before, after, effects, approximate })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard: the test run uses the same core as the real execution and no own
    /// implementation of matching or actions.
    #[test]
    fn dry_run_uses_production_core() {
        let me = include_str!("workflow_dryrun.rs");
        // first occurrence = the real test module (the string here comes after it)
        let code = &me[..me.find("mod tests {").unwrap_or(me.len())];
        assert!(code.contains("workflows::run_workflow_list_inner("), "dry run must call the production core");
        for forbidden in [
            "apply_assignment_to_document",
            "apply_removal_to_document",
            "existing_document_matches_workflow(",
            "docops::save",
            "workflow_ext::decide(",
            "record_run(",
        ] {
            assert!(!code.contains(forbidden), "dry run must not reimplement `{forbidden}`");
        }
        // and the real paths run through the same core
        let wf = include_str!("workflows.rs");
        let prod = &wf[..wf.find("mod tests {").unwrap_or(wf.len())];
        assert!(prod.matches("run_workflow_list_inner(conn, env,").count() >= 3, "run_workflows/run_workflow_list/run_workflow_as use the core");
        assert!(prod.contains("workflow_ext::decide(wf, trigger_type, state"), "core decides via workflow_ext::decide");
        assert_eq!(prod.matches("async fn apply_assignment_to_document(").count(), 1);
    }

    #[test]
    fn consumable_reconstruction() {
        let t = pl_matching::workflow::WorkflowTrigger {
            trigger_type: TRIGGER_CONSUMPTION,
            sources: vec![3],
            filter_mailrule_id: Some(7),
            ..Default::default()
        };
        let wf = WorkflowDef { id: 1, triggers: vec![t], ..Default::default() };
        let opts = DryRunOptions { trigger_type: 1, ai_mode: AiMode::NotChecked, time_zone: chrono_tz::UTC, source: None, mailrule_id: None };
        let c = consumable_for(&wf, Some("Rechnung 2024.pdf"), &opts);
        assert_eq!(c.source, DocumentSource::MailFetch);
        assert_eq!(c.mailrule_id, Some(7));
        assert_eq!(c.file_name(), "Rechnung 2024.pdf");
        let opts = DryRunOptions { source: Some(1), mailrule_id: Some(2), ..opts };
        let c = consumable_for(&wf, None, &opts);
        assert_eq!(c.source, DocumentSource::ConsumeFolder);
        assert_eq!(c.mailrule_id, Some(2));
        assert_eq!(c.file_name(), "document");
    }
}
