//! PaperRust extensions of workflow execution (docs/WORKFLOWS-NEU.md, contract
//! docs/contract/automation.md). No upstream counterpart.
//!
//! * **History** (`paperrust_workflow_run`): every real execution (ingestion, added, updated,
//!   schedule, manual, mail = ingestion with source "mail") writes, besides the ngx `WorkflowRun`, an
//!   entry with result, branch, message and changed fields ([`insert_run`]).
//! * **"Else" branch** (`paperrust_workflow_else_action`): actions that run when the trigger
//!   has fired but the conditions do not match. Decision in [`decide`] – one function for
//!   all execution paths and the test run.
//! * **AI condition** (`paperrust_workflow_ext.ai_condition`): "only if the AI thinks: field = value
//!   (confidence ≥ x)" via an attached [`AiJudge`] (pl-server attaches pl-ai). Without AI it is
//!   ignored and marked in the history.
//!
//! The tables are created by [`ensure_conn`] / [`ensure_pool`] as needed (or `paperrust manage migrate`).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool, Row};

use crate::db::DocRow;
use crate::workflows::{
    ACTION_APPLY_AI_SUGGESTIONS, ACTION_ASSIGNMENT, ACTION_EMAIL, ACTION_MOVE_TO_TRASH, ACTION_PASSWORD_REMOVAL,
    ACTION_REMOTE_OCR, ACTION_REMOVAL, ACTION_WEBHOOK, ActionDef, WorkflowDef,
};

/// Migration (name, SQL) for `paperrust manage migrate`.
pub const MIGRATION: (&str, &str) = ("pl_consumer.0001_paperrust_workflow", include_str!("../migrations/0001_paperrust_workflow.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

static READY: AtomicBool = AtomicBool::new(false);

const EXISTS_SQL: &str = "SELECT to_regclass('public.paperrust_workflow_run') IS NOT NULL \
    AND to_regclass('public.paperrust_workflow_else_action') IS NOT NULL \
    AND to_regclass('public.paperrust_workflow_ext') IS NOT NULL";

fn create_script() -> String {
    // A single command (DO block): without an outer transaction it runs as one transaction, the
    // lock applies until its end; inside an outer transaction until the end of that.
    format!(
        "DO $paperrust$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('paperrust_workflow_ext'));
{}
END $paperrust$",
        MIGRATION.1
    )
}

/// Only for tests with several throwaway databases in one process: forget "ready".
#[doc(hidden)]
pub fn forget_ready_for_tests() {
    READY.store(false, Ordering::Relaxed);
}

/// Create the tables via a pool connection (without an outer transaction) and remember them as ready.
pub async fn ensure_pool(pool: &PgPool) -> Result<(), sqlx::Error> {
    if READY.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut c = pool.acquire().await?;
    let exists: bool = sqlx::query_scalar(EXISTS_SQL).fetch_one(&mut *c).await?;
    if !exists {
        sqlx::query(&create_script()).execute(&mut *c).await?;
    }
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// Ensure the tables on the caller's connection (also inside a transaction).
/// Does not remember "ready" itself: an outer transaction could be rolled back; that is
/// taken over by [`ensure_pool`]. Until then every call costs one `to_regclass` query.
pub async fn ensure_conn(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    if READY.load(Ordering::Relaxed) {
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar(EXISTS_SQL).fetch_one(&mut *conn).await?;
    if !exists {
        sqlx::query(&create_script()).execute(&mut *conn).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------ AI condition

/// Fields about which an AI condition can judge (like `pl_ai::workflow_assist::AiCondition`).
pub const AI_FIELDS: [&str; 4] = ["document_type", "correspondent", "storage_path", "tags"];

/// Default threshold "sure" (same as `pl_ai::workflow_assist::SURE`).
pub const AI_SURE: f64 = 0.9;

fn default_min() -> f64 {
    AI_SURE
}

/// AI condition of a workflow (`paperrust_workflow_ext.ai_condition`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiConditionDef {
    /// `document_type` | `correspondent` | `storage_path` | `tags`.
    pub field: String,
    pub object_id: i64,
    #[serde(default = "default_min")]
    pub min_confidence: f64,
}

impl AiConditionDef {
    /// Formal check (field, threshold 0…1). Whether the object exists is checked by the API.
    pub fn validate(&self) -> Result<(), String> {
        if !AI_FIELDS.contains(&self.field.as_str()) {
            return Err(format!("\"{}\" is not a valid choice.", self.field));
        }
        if !(0.0..=1.0).contains(&self.min_confidence) || self.min_confidence.is_nan() {
            return Err("Ensure this value is between 0 and 1.".into());
        }
        Ok(())
    }

    /// Table of the target object.
    pub fn object_table(&self) -> &'static str {
        match self.field.as_str() {
            "document_type" => "documents_documenttype",
            "correspondent" => "documents_correspondent",
            "storage_path" => "documents_storagepath",
            _ => "documents_tag",
        }
    }
}

/// Verdict of the AI on a condition.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AiJudgement {
    pub matched: bool,
    /// What the AI thinks for the field (name/value), if known.
    pub value: Option<String>,
    pub confidence: Option<f64>,
}

/// AI connection for the condition (implementation in pl-automation via pl-ai `ai_verdict`).
pub trait AiJudge: Send + Sync {
    /// Is the AI switched on (effective configuration)?
    fn enabled(&self) -> BoxFuture<'_, bool>;
    /// Verdict on a **committed** document (reads via its own pool).
    fn judge<'a>(&'a self, document_id: i64, condition: &'a AiConditionDef) -> BoxFuture<'a, Result<AiJudgement, String>>;
}

static JUDGE: RwLock<Option<Arc<dyn AiJudge>>> = RwLock::new(None);

/// Attach the AI connection (`None` = none). Called at server and worker startup.
pub fn install_ai_judge(judge: Option<Arc<dyn AiJudge>>) {
    if let Ok(mut g) = JUDGE.write() {
        *g = judge;
    }
}

/// The attached AI connection.
pub fn ai_judge() -> Option<Arc<dyn AiJudge>> {
    JUDGE.read().ok().and_then(|g| g.clone())
}

/// How the AI condition is handled in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiMode {
    /// Ask immediately (worker: schedule, manual, bulk change, caught-up runs).
    #[default]
    Inline,
    /// Catch up the run as a task after the commit (ingestion: document not yet committed;
    /// API "document updated": no LLM call in the request).
    Defer,
    /// Do not ask (test run without AI): treat the condition as met and mark it.
    NotChecked,
}

/// Additional options of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExtOptions {
    pub ai_mode: AiMode,
    /// Also note non-matching conditions in the history (manual/caught-up runs, test run).
    /// Automatic triggers do not note them (otherwise one entry per change and workflow).
    pub log_skips: bool,
}

// ------------------------------------------------------------------------------ Decision

/// Which action list runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Branch {
    Main,
    Else,
}

impl Branch {
    pub fn as_str(self) -> &'static str {
        match self {
            Branch::Main => "main",
            Branch::Else => "else",
        }
    }
}

/// Result of [`decide`].
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run { branch: Branch, code: &'static str, note: Option<String> },
    /// Execute nothing. `log`: note in the history; `error`: as an error.
    Skip { code: &'static str, note: Option<String>, log: bool, error: bool },
    /// Catch up as a task after the commit (AI condition, [`AiMode::Defer`]).
    Defer,
}

/// Result of filter matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchState {
    Matched,
    /// Trigger of the type present, filter not met (`reason` only if determined).
    NotMatched { reason: Option<String> },
    /// No trigger of this type on the workflow – the workflow is not responsible for this trigger.
    NotFired,
    /// Matching failed (e.g. unknown algorithm) – like upstream: do not execute.
    Error(String),
    /// Filter skipped (manual "apply to all"): main actions without conditions.
    Bypassed,
}

/// Main or else actions of the workflow.
pub fn branch_actions(wf: &WorkflowDef, branch: Branch) -> &[ActionDef] {
    match branch {
        Branch::Main => &wf.actions,
        Branch::Else => &wf.else_actions,
    }
}

/// The one decision "what runs" for all execution paths and the test run.
///
/// 1. No trigger of the type → nothing (not even the else branch).
/// 2. Filter not met → else branch if present; otherwise nothing.
/// 3. Filter met, AI condition set → for the ingestion trigger / without a document / without AI /
///    AI off: ignored (main branch, marked); [`AiMode::Defer`] → catch up;
///    [`AiMode::NotChecked`] → main branch, marked; otherwise ask the AI: yes → main, no → else or
///    nothing, error → nothing (noted as an error, else branch does not run).
pub fn decide<'a>(wf: &'a WorkflowDef, trigger_type: i64, state: MatchState, document_id: Option<i64>, opts: ExtOptions) -> BoxFuture<'a, Decision> {
    Box::pin(decide_inner(wf, trigger_type, state, document_id, opts))
}

async fn decide_inner(wf: &WorkflowDef, trigger_type: i64, state: MatchState, document_id: Option<i64>, opts: ExtOptions) -> Decision {
    let matched = match state {
        MatchState::NotFired => return Decision::Skip { code: "not_fired", note: None, log: false, error: false },
        MatchState::Error(e) => return Decision::Skip { code: "match_error", note: Some(e), log: true, error: true },
        MatchState::Bypassed => return Decision::Run { branch: Branch::Main, code: "filters_bypassed", note: None },
        MatchState::NotMatched { reason } => {
            if !wf.else_actions.is_empty() {
                return Decision::Run { branch: Branch::Else, code: "else", note: reason };
            }
            return Decision::Skip { code: "conditions_not_met", note: reason, log: opts.log_skips, error: false };
        }
        MatchState::Matched => true,
    };
    debug_assert!(matched);
    let Some(cond) = wf.ai_condition.as_ref() else {
        return Decision::Run { branch: Branch::Main, code: "ok", note: None };
    };
    if trigger_type == pl_matching::workflow::TRIGGER_CONSUMPTION || document_id.is_none() {
        return Decision::Run {
            branch: Branch::Main,
            code: "ai_ignored_consumption",
            note: Some("AI condition ignored: there is no document yet when consumption starts".into()),
        };
    }
    let Some(judge) = ai_judge() else {
        return Decision::Run { branch: Branch::Main, code: "ai_ignored_unavailable", note: Some("AI condition ignored: AI is not available".into()) };
    };
    if !judge.enabled().await {
        return Decision::Run { branch: Branch::Main, code: "ai_ignored_disabled", note: Some("AI condition ignored: AI is switched off".into()) };
    }
    match opts.ai_mode {
        AiMode::Defer => Decision::Defer,
        AiMode::NotChecked => {
            Decision::Run { branch: Branch::Main, code: "ai_not_checked", note: Some("AI condition not checked (dry run without AI)".into()) }
        }
        AiMode::Inline => {
            let Some(doc) = document_id else {
                return Decision::Run { branch: Branch::Main, code: "ai_ignored_consumption", note: None };
            };
            match judge.judge(doc, cond).await {
                Ok(j) => {
                    let what = describe_judgement(&j);
                    if j.matched {
                        Decision::Run { branch: Branch::Main, code: "ai_met", note: Some(format!("AI condition met: {what}")) }
                    } else if !wf.else_actions.is_empty() {
                        Decision::Run { branch: Branch::Else, code: "ai_not_met_else", note: Some(format!("AI condition not met: {what}")) }
                    } else {
                        Decision::Skip { code: "ai_not_met", note: Some(format!("AI condition not met: {what}")), log: true, error: false }
                    }
                }
                Err(e) => Decision::Skip {
                    code: "ai_error",
                    note: Some(format!("AI condition could not be checked: {}", short(&e))),
                    log: true,
                    error: true,
                },
            }
        }
    }
}

fn describe_judgement(j: &AiJudgement) -> String {
    match (&j.value, j.confidence) {
        (Some(v), Some(c)) => format!("{v} ({:.0} %)", c * 100.0),
        (Some(v), None) => v.clone(),
        (None, _) => "no opinion".into(),
    }
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

// ------------------------------------------------------------------------------ History

/// Result of a run in the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunResult {
    Ok,
    Skipped,
    Error,
}

impl RunResult {
    pub fn as_str(self) -> &'static str {
        match self {
            RunResult::Ok => "ok",
            RunResult::Skipped => "skipped",
            RunResult::Error => "error",
        }
    }
}

/// A run (history and result of the test run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRecord {
    pub workflow_id: i64,
    pub trigger_type: i64,
    pub document_id: Option<i64>,
    pub result: RunResult,
    /// `None` = no actions executed.
    pub branch: Option<Branch>,
    pub code: String,
    pub message: String,
    pub changed_fields: Vec<String>,
    /// `DocumentSource` on ingestion (3 = mail).
    pub source: Option<i64>,
}

/// A run to be caught up (AI condition with [`AiMode::Defer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredRun {
    pub workflow_id: i64,
    pub document_id: i64,
    pub trigger_type: i64,
}

/// Retention of the history in days (`PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS`, default 90,
/// `0` = unlimited).
pub fn retention_days() -> i64 {
    pl_core::config::var("PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(90)
}

static LAST_CLEANUP: AtomicI64 = AtomicI64::new(0);

/// Write an entry (connection of the run). At most once per hour and process, entries
/// older than the retention period are deleted in the process.
pub fn insert_run<'a>(conn: &'a mut PgConnection, r: &'a RunRecord) -> BoxFuture<'a, Result<(), sqlx::Error>> {
    Box::pin(insert_run_inner(conn, r))
}

async fn insert_run_inner(conn: &mut PgConnection, r: &RunRecord) -> Result<(), sqlx::Error> {
    ensure_conn(conn).await?;
    let (Ok(wf), Ok(tt)) = (i32::try_from(r.workflow_id), i16::try_from(r.trigger_type)) else { return Ok(()) };
    sqlx::query(
        "INSERT INTO paperrust_workflow_run (workflow_id, trigger_type, document_id, run_at, result, branch, code, message, changed_fields, source) \
         SELECT $1, $2, $3, now(), $4, $5, $6, $7, $8, $9 WHERE EXISTS (SELECT 1 FROM documents_workflow WHERE id = $1)",
    )
    .bind(wf)
    .bind(tt)
    .bind(r.document_id.and_then(|d| i32::try_from(d).ok()))
    .bind(r.result.as_str())
    .bind(r.branch.map_or("none", Branch::as_str))
    .bind(&r.code)
    .bind(short_message(&r.message))
    .bind(serde_json::to_value(&r.changed_fields).unwrap_or(serde_json::Value::Array(Vec::new())))
    .bind(r.source.and_then(|s| i16::try_from(s).ok()))
    .execute(&mut *conn)
    .await?;
    let now = chrono::Utc::now().timestamp();
    let last = LAST_CLEANUP.load(Ordering::Relaxed);
    if now - last >= 3600 && LAST_CLEANUP.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        cleanup(conn, retention_days()).await?;
    }
    Ok(())
}

fn short_message(m: &str) -> String {
    m.chars().take(1000).collect()
}

/// Error case: write an "error" entry on a best-effort basis. If the caller's transaction is already
/// aborted, this fails silently (the error is then only in the log; Known difference).
pub async fn insert_error(conn: &mut PgConnection, workflow_id: i64, trigger_type: i64, document_id: Option<i64>, err: &sqlx::Error) {
    let r = RunRecord {
        workflow_id,
        trigger_type,
        document_id,
        result: RunResult::Error,
        branch: None,
        code: "error".into(),
        message: error_text(err),
        changed_fields: Vec::new(),
        source: None,
    };
    if let Err(e) = insert_run(conn, &r).await {
        tracing::debug!(target: "paperless.workflows", "could not record failed workflow run: {e}");
    }
}

/// Error text without internals (no connection data, truncated).
pub fn error_text(e: &sqlx::Error) -> String {
    let s = match e {
        sqlx::Error::Database(d) => d.message().to_string(),
        sqlx::Error::Protocol(p) => p.clone(),
        sqlx::Error::RowNotFound => "row not found".into(),
        other => other.to_string(),
    };
    short(&s)
}

/// Delete entries older than `days` days (`0` = delete nothing). Returns: deleted.
pub async fn cleanup(conn: &mut PgConnection, days: i64) -> Result<u64, sqlx::Error> {
    if days <= 0 {
        return Ok(0);
    }
    ensure_conn(conn).await?;
    let r = sqlx::query("DELETE FROM paperrust_workflow_run WHERE run_at < now() - make_interval(days => $1)")
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .execute(&mut *conn)
        .await?;
    Ok(r.rows_affected())
}

/// Changed fields of a run: row fields (compared before/after) and the areas that
/// the executed actions touch (tags, user fields, permissions, external effects).
pub fn changed_fields(before: &DocRow, after: Option<&DocRow>, actions: &[ActionDef]) -> Vec<String> {
    let mut out: BTreeSet<&'static str> = BTreeSet::new();
    if let Some(a) = after {
        if a.title != before.title {
            out.insert("title");
        }
        if a.correspondent_id != before.correspondent_id {
            out.insert("correspondent");
        }
        if a.document_type_id != before.document_type_id {
            out.insert("document_type");
        }
        if a.storage_path_id != before.storage_path_id {
            out.insert("storage_path");
        }
        if a.owner_id != before.owner_id {
            out.insert("owner");
        }
    }
    out.extend(touched(actions));
    out.into_iter().map(String::from).collect()
}

/// Areas that actions touch (without row comparison; for the ingestion trigger).
pub fn touched(actions: &[ActionDef]) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    for a in actions {
        match a.action_type {
            ACTION_ASSIGNMENT => {
                if !a.assign_tags.is_empty() {
                    out.insert("tags");
                }
                if !a.assign_custom_fields.is_empty() {
                    out.insert("custom_fields");
                }
                if !(a.assign_view_users.is_empty()
                    && a.assign_view_groups.is_empty()
                    && a.assign_change_users.is_empty()
                    && a.assign_change_groups.is_empty())
                {
                    out.insert("permissions");
                }
            }
            ACTION_REMOVAL => {
                if a.remove_all_tags || !a.remove_tags.is_empty() {
                    out.insert("tags");
                }
                if a.remove_all_custom_fields || !a.remove_custom_fields.is_empty() {
                    out.insert("custom_fields");
                }
                if a.remove_all_permissions
                    || !(a.remove_view_users.is_empty()
                        && a.remove_view_groups.is_empty()
                        && a.remove_change_users.is_empty()
                        && a.remove_change_groups.is_empty())
                {
                    out.insert("permissions");
                }
            }
            ACTION_EMAIL => {
                out.insert("email");
            }
            ACTION_WEBHOOK => {
                out.insert("webhook");
            }
            ACTION_PASSWORD_REMOVAL => {
                out.insert("password_removal");
            }
            ACTION_MOVE_TO_TRASH => {
                out.insert("trash");
            }
            ACTION_REMOTE_OCR => {
                out.insert("remote_ocr");
            }
            ACTION_APPLY_AI_SUGGESTIONS => {
                out.insert("ai_suggestions");
            }
            _ => {}
        }
    }
    out
}

/// [`RunRecord`] for a [`Decision`] (without the fields of the execution).
pub fn record_for(wf: &WorkflowDef, trigger_type: i64, document_id: Option<i64>, decision: &Decision) -> Option<RunRecord> {
    let base = |result, branch, code: &str, note: &Option<String>| RunRecord {
        workflow_id: wf.id,
        trigger_type,
        document_id,
        result,
        branch,
        code: code.to_string(),
        message: note.clone().unwrap_or_default(),
        changed_fields: Vec::new(),
        source: None,
    };
    match decision {
        Decision::Run { branch, code, note } => Some(base(RunResult::Ok, Some(*branch), code, note)),
        Decision::Skip { code, note, log: true, error } => {
            Some(base(if *error { RunResult::Error } else { RunResult::Skipped }, None, code, note))
        }
        Decision::Skip { .. } | Decision::Defer => None,
    }
}

// ------------------------------------------------------------------------------ Loading

/// AI conditions of the workflows (invalid entries are skipped with a warning).
pub async fn load_ai_conditions(conn: &mut PgConnection, wf_ids: &[i64]) -> Result<HashMap<i64, AiConditionDef>, sqlx::Error> {
    let mut out = HashMap::new();
    if wf_ids.is_empty() {
        return Ok(out);
    }
    ensure_conn(conn).await?;
    let ids: Vec<i32> = wf_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let rows = sqlx::query("SELECT workflow_id, ai_condition FROM paperrust_workflow_ext WHERE workflow_id = ANY($1) AND ai_condition IS NOT NULL")
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await?;
    for r in rows {
        let id = i64::from(r.try_get::<i32, _>(0)?);
        let v: serde_json::Value = r.try_get(1)?;
        match serde_json::from_value::<AiConditionDef>(v) {
            Ok(c) if c.validate().is_ok() => {
                out.insert(id, c);
            }
            _ => tracing::warn!(target: "paperless.workflows", "ignoring invalid AI condition of workflow {id}"),
        }
    }
    Ok(out)
}

/// Else actions of a workflow (ids by `order`, id).
pub async fn else_action_ids(conn: &mut PgConnection, workflow_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    ensure_conn(conn).await?;
    let Ok(id) = i32::try_from(workflow_id) else { return Ok(Vec::new()) };
    let ids: Vec<i32> = sqlx::query_scalar(
        "SELECT a.id FROM documents_workflowaction a JOIN paperrust_workflow_else_action e ON e.workflowaction_id = a.id \
         WHERE e.workflow_id = $1 ORDER BY a.\"order\", a.id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.into_iter().map(i64::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(else_n: usize, ai: Option<AiConditionDef>) -> WorkflowDef {
        WorkflowDef {
            id: 1,
            name: "w".into(),
            triggers: vec![],
            actions: vec![ActionDef { id: 10, action_type: ACTION_ASSIGNMENT, ..Default::default() }],
            else_actions: (0..else_n).map(|i| ActionDef { id: 20 + i as i64, action_type: ACTION_REMOVAL, ..Default::default() }).collect(),
            ai_condition: ai,
        }
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    #[test]
    fn decide_basic_branches() {
        let o = ExtOptions::default();
        assert_eq!(
            block(decide(&wf(0, None), 3, MatchState::NotFired, Some(1), o)),
            Decision::Skip { code: "not_fired", note: None, log: false, error: false }
        );
        assert!(matches!(block(decide(&wf(0, None), 3, MatchState::Matched, Some(1), o)), Decision::Run { branch: Branch::Main, .. }));
        assert!(matches!(
            block(decide(&wf(1, None), 3, MatchState::NotMatched { reason: None }, Some(1), o)),
            Decision::Run { branch: Branch::Else, .. }
        ));
        assert!(matches!(
            block(decide(&wf(0, None), 3, MatchState::NotMatched { reason: None }, Some(1), o)),
            Decision::Skip { log: false, .. }
        ));
        let logged = ExtOptions { log_skips: true, ..o };
        assert!(matches!(
            block(decide(&wf(0, None), 3, MatchState::NotMatched { reason: None }, Some(1), logged)),
            Decision::Skip { log: true, error: false, .. }
        ));
        // Error in matching: never the else branch
        assert!(matches!(block(decide(&wf(1, None), 3, MatchState::Error("x".into()), Some(1), o)), Decision::Skip { error: true, .. }));
        assert!(matches!(block(decide(&wf(1, None), 3, MatchState::Bypassed, Some(1), o)), Decision::Run { branch: Branch::Main, .. }));
    }

    #[test]
    fn ai_condition_without_judge_is_ignored_and_marked() {
        install_ai_judge(None);
        let c = AiConditionDef { field: "document_type".into(), object_id: 3, min_confidence: 0.9 };
        match block(decide(&wf(0, Some(c.clone())), 3, MatchState::Matched, Some(1), ExtOptions::default())) {
            Decision::Run { branch: Branch::Main, code, note } => {
                assert_eq!(code, "ai_ignored_unavailable");
                assert!(note.is_some());
            }
            other => panic!("{other:?}"),
        }
        match block(decide(&wf(0, Some(c)), 1, MatchState::Matched, None, ExtOptions::default())) {
            Decision::Run { code, .. } => assert_eq!(code, "ai_ignored_consumption"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn condition_validation() {
        let mut c = AiConditionDef { field: "tags".into(), object_id: 1, min_confidence: 0.5 };
        assert!(c.validate().is_ok());
        c.min_confidence = 1.5;
        assert!(c.validate().is_err());
        c.min_confidence = 0.5;
        c.field = "title".into();
        assert!(c.validate().is_err());
        let d: AiConditionDef = serde_json::from_value(serde_json::json!({"field": "correspondent", "object_id": 4})).unwrap();
        assert_eq!(d.min_confidence, AI_SURE);
    }

    #[test]
    fn touched_and_changed() {
        let before = DocRow { id: 1, title: "a".into(), ..test_row() };
        let mut after = before.clone();
        after.title = "b".into();
        after.correspondent_id = Some(3);
        let acts = vec![
            ActionDef { action_type: ACTION_ASSIGNMENT, assign_tags: vec![1], ..Default::default() },
            ActionDef { action_type: ACTION_EMAIL, ..Default::default() },
        ];
        assert_eq!(changed_fields(&before, Some(&after), &acts), vec!["correspondent", "email", "tags", "title"]);
        assert_eq!(changed_fields(&before, None, &[]), Vec::<String>::new());
    }

    fn test_row() -> DocRow {
        DocRow {
            id: 0,
            title: String::new(),
            content: String::new(),
            mime_type: String::new(),
            checksum: String::new(),
            archive_checksum: None,
            page_count: None,
            created: chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap_or_default(),
            modified: chrono::Utc::now(),
            added: chrono::Utc::now(),
            filename: None,
            archive_filename: None,
            original_filename: None,
            archive_serial_number: None,
            correspondent_id: None,
            document_type_id: None,
            storage_path_id: None,
            owner_id: None,
            root_document_id: None,
            version_label: None,
            version_index: None,
            deleted_at: None,
            restored_at: None,
            transaction_id: None,
        }
    }

    #[test]
    fn records_for_decisions() {
        let w = wf(0, None);
        assert!(record_for(&w, 3, Some(1), &Decision::Defer).is_none());
        assert!(record_for(&w, 3, Some(1), &Decision::Skip { code: "x", note: None, log: false, error: false }).is_none());
        let r = record_for(&w, 3, Some(1), &Decision::Skip { code: "ai_error", note: Some("n".into()), log: true, error: true }).unwrap();
        assert_eq!(r.result, RunResult::Error);
        assert_eq!(r.message, "n");
        let r = record_for(&w, 3, Some(1), &Decision::Run { branch: Branch::Else, code: "else", note: None }).unwrap();
        assert_eq!(r.branch, Some(Branch::Else));
    }
}
