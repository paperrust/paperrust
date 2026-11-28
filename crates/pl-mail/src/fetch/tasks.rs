//! Tasks `process_mail_accounts`, `apply_mail_action`, `error_callback`.
//!
//! upstream: src/paperless_mail/tasks.py@v3.4.2 (process_mail_accounts, MAIL_FETCH_LOCK_KEY/TTL),
//! src/paperless_mail/mail.py@v3.4.2 (apply_mail_action, error_callback).
//!
//! Lock against parallel fetches: upstream `cache.add(paperless_mail_fetch_lock, TTL 1800)`; here
//! a PostgreSQL advisory lock on a dedicated connection (applies across processes for all
//! PaperRust workers of the same DB, releases itself when the connection drops – no TTL needed).

use std::sync::Arc;

use serde_json::{Map, Value};
use sqlx::Connection;

use super::client::{ImapError, MailBox};
use super::db::{self, NewProcessed};
use super::handler::{AccountRun, parse_py_isoformat, post_consume};
use super::{MailWorker, criteria};

/// Key of the advisory lock (`paperless_mail_fetch_lock`).
pub const MAIL_FETCH_LOCK_ID: i64 = 0x7061_7065_726d_6c66; // "papermlf"

pub const SKIPPED: &str = "Skipped: mail account processing already in progress.";

/// Failure of `process_mail_accounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Database or lock problem.
    Database(String),
    /// Every account that was tried failed (PaperRust; upstream always returns the text).
    AllAccountsFailed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Database(m) | FetchError::AllAccountsFailed(m) => f.write_str(m),
        }
    }
}

/// `process_mail_accounts(account_ids=None)` → result text.
pub async fn process_mail_accounts(worker: &MailWorker, account_ids: Option<Vec<i32>>) -> Result<String, String> {
    process_mail_accounts_checked(worker, account_ids).await.map_err(|e| e.to_string())
}

/// Like [`process_mail_accounts`], with the kind of failure.
pub async fn process_mail_accounts_checked(worker: &MailWorker, account_ids: Option<Vec<i32>>) -> Result<String, FetchError> {
    let db = |e: sqlx::Error| FetchError::Database(e.to_string());
    let mut lock_conn = worker.pool.acquire().await.map_err(db)?.detach();
    let (got,): (bool,) =
        sqlx::query_as("SELECT pg_try_advisory_lock($1)").bind(MAIL_FETCH_LOCK_ID).fetch_one(&mut lock_conn).await.map_err(db)?;
    if !got {
        tracing::info!(target: "paperless.mail.tasks", "Mail account processing is already running; skipping this run.");
        let _ = lock_conn.close().await;
        return Ok(SKIPPED.to_string());
    }
    let result = run_accounts(worker, account_ids).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)").bind(MAIL_FETCH_LOCK_ID).execute(&mut lock_conn).await;
    let _ = lock_conn.close().await;
    result
}

async fn run_accounts(worker: &MailWorker, account_ids: Option<Vec<i32>>) -> Result<String, FetchError> {
    let db = |e: sqlx::Error| FetchError::Database(e.to_string());
    let ids = account_ids.filter(|v| !v.is_empty());
    let accounts = db::accounts(&worker.pool, ids.as_deref()).await.map_err(db)?;
    let mut total: i64 = 0;
    let mut outcomes: Vec<AccountOutcome> = Vec::new();
    for account in accounts {
        if !db::has_enabled_rules(&worker.pool, account.id).await.map_err(db)? {
            tracing::info!(target: "paperless.mail.tasks", "No rules enabled for account {}. Skipping.", account.name);
            continue;
        }
        let name = account.name.clone();
        let mut run = AccountRun::new(worker);
        let error = match run.handle_mail_account(account).await {
            Ok(n) => {
                total += n;
                run.failure().map(str::to_string)
            }
            Err(e) => {
                tracing::error!(target: "paperless.mail.tasks", "Error while processing mail account {name}: {e}");
                Some(e.to_string())
            }
        };
        outcomes.push(AccountOutcome { name, error });
    }
    summarize(total, &outcomes)
}

/// Result of one account in a fetch run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountOutcome {
    pub name: String,
    /// `None` = processed without an account-level error.
    pub error: Option<String>,
}

/// Result text of `process_mail_accounts`.
///
/// upstream logs account errors and always returns "Added N document(s)." / "No new documents
/// were added.", so a run where every account fails looks like a quiet success. PaperRust:
/// all tried accounts failed → task failure with the errors; some failed → upstream text plus
/// the failed accounts (Known differences).
pub fn summarize(total: i64, outcomes: &[AccountOutcome]) -> Result<String, FetchError> {
    let failed: Vec<String> = outcomes.iter().filter_map(|o| o.error.as_ref().map(|e| format!("{}: {}", o.name, e))).collect();
    let base = if total > 0 { format!("Added {total} document(s).") } else { "No new documents were added.".to_string() };
    if failed.is_empty() {
        return Ok(base);
    }
    if failed.len() == outcomes.len() {
        return Err(FetchError::AllAccountsFailed(format!("All {} mail account(s) failed: {}", failed.len(), failed.join("; "))));
    }
    Ok(format!("{base} {} of {} mail account(s) failed: {}", failed.len(), outcomes.len(), failed.join("; ")))
}

fn kw_i32(kw: &Map<String, Value>, key: &str) -> Result<i32, pl_queue::TaskError> {
    kw.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| pl_queue::TaskError::fail("TypeError", format!("missing required argument: '{key}'")))
}

fn kw_str(kw: &Map<String, Value>, key: &str) -> Option<String> {
    kw.get(key).and_then(Value::as_str).map(str::to_string)
}

fn received(kw: &Map<String, Value>, worker: &MailWorker) -> chrono::DateTime<chrono::FixedOffset> {
    kw_str(kw, "message_date")
        .and_then(|s| parse_py_isoformat(&s))
        .unwrap_or_else(|| chrono::Utc::now().with_timezone(&worker.config.time_zone).fixed_offset())
}

/// Python-like traceback text for `ProcessedMail.error` (content not byte-identical).
fn traceback_text(exc_type: &str, message: &str) -> String {
    format!("Traceback (most recent call last):\n  File \"paperrust\", in apply_mail_action\n{exc_type}: {message}\n")
}

/// Result of `apply_mail_action`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Success,
    Failed {
        error_type: String,
        message: String,
    },
    /// PaperRust: the mail action was not run because the message may no longer be the one that
    /// was imported (UIDVALIDITY, folder or account changed). Recorded as FAILED with the reason.
    Skipped {
        reason: String,
    },
}

/// The message the queued action refers to, as recorded at import time.
///
/// `folder`/`account_id` are PaperRust additions to the task kwargs (`None` for tasks queued by
/// upstream or older PaperRust versions → no check, upstream behaviour).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTarget {
    pub uid: String,
    pub folder: Option<String>,
    pub account_id: Option<i32>,
    pub uid_validity: Option<String>,
}

impl ActionTarget {
    pub fn from_kwargs(kw: &Map<String, Value>) -> Self {
        Self {
            uid: kw_str(kw, "message_uid").unwrap_or_else(|| "None".into()),
            folder: kw_str(kw, "folder"),
            account_id: kw.get("account_id").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()),
            uid_validity: kw_str(kw, "uid_validity"),
        }
    }

    /// Folder for the `ProcessedMail` row: the import folder when known.
    fn record_folder<'a>(&'a self, rule: &'a db::Rule) -> &'a str {
        self.folder.as_deref().unwrap_or(&rule.folder)
    }
}

/// Reason to skip before connecting: the rule now points at another account or folder.
fn moved_target(target: &ActionTarget, rule: &db::Rule) -> Option<String> {
    if let Some(a) = target.account_id
        && a != rule.account_id
    {
        return Some(format!("Rule {}: mail account changed since mail {} was imported; mail action skipped", rule.display(), target.uid));
    }
    if let Some(f) = &target.folder
        && f != &rule.folder
    {
        return Some(format!(
            "Rule {}: folder changed from {f} to {} since mail {} was imported; mail action skipped",
            rule.display(),
            rule.folder,
            target.uid
        ));
    }
    None
}

/// Reason to skip after SELECT: the folder's UIDVALIDITY differs from the one at import, so the
/// UID may now name another message (RFC 3501 2.3.1.1). Unknown on either side → no check.
fn uidvalidity_mismatch(target: &ActionTarget, current: Option<i64>, rule: &db::Rule) -> Option<String> {
    let stored = target.uid_validity.as_deref()?;
    let current = current?;
    if stored.trim() == current.to_string() {
        return None;
    }
    Some(format!(
        "Rule {}: UIDVALIDITY of folder {} changed from {stored} to {current} since mail {} was imported; mail action skipped",
        rule.display(),
        rule.folder,
        target.uid
    ))
}

/// IMAP part of `apply_mail_action` (connect, login, SELECT, checks, `post_consume`, logout).
pub async fn run_mail_action(
    connect: &super::client::ConnectOptions,
    account: &db::Account,
    rule: &db::Rule,
    target: &ActionTarget,
) -> Result<ActionOutcome, (String, String)> {
    if let Some(reason) = moved_target(target, rule) {
        tracing::warn!(target: "paperless_mail", "{reason}");
        return Ok(ActionOutcome::Skipped { reason });
    }
    let port = u16::try_from(account.imap_port.unwrap_or(0)).unwrap_or(0);
    let mut mb = MailBox::connect(&account.imap_server, port, account.imap_security, connect).await.map_err(err_pair)?;
    let inner: Result<ActionOutcome, (String, String)> = async {
        let gmail = mb.capabilities.iter().any(|c| c == "X-GM-EXT-1");
        mb.login(&account.username, &account.password, account.is_token).await.map_err(err_pair)?;
        mb.folder_set(&rule.folder).await.map_err(err_pair)?;
        let mut current = mb.selected_uidvalidity;
        if current.is_none() && target.uid_validity.is_some() {
            // SELECT without the response code: same source as at import time.
            current = mb.folder_status_uidvalidity(&rule.folder).await.ok().flatten();
        }
        if let Some(reason) = uidvalidity_mismatch(target, current, rule) {
            tracing::warn!(target: "paperless_mail", "{reason}");
            return Ok(ActionOutcome::Skipped { reason });
        }
        let action = criteria::get_rule_action(rule.action, rule.action_parameter.as_deref(), gmail).map_err(|e| match e {
            criteria::ActionError::Mail(m) => ("MailError".to_string(), m),
            criteria::ActionError::Value(m) => ("ValueError".to_string(), m),
        })?;
        if let Err(e) = post_consume(&mut mb, &action, &target.uid, rule.action_parameter.as_deref()).await {
            if matches!(e, ImapError::Status { .. }) {
                tracing::error!(target: "paperless_mail", "Error while processing mail action during post_consume: {e}");
            }
            return Err(err_pair(e));
        }
        Ok(ActionOutcome::Success)
    }
    .await;
    let bye = mb.logout().await;
    let outcome = inner?;
    if !bye {
        return Err(("MailboxLogoutError".into(), "Response status \"BYE\" expected".into()));
    }
    Ok(outcome)
}

/// `apply_mail_action(result, rule_id, message_uid, message_subject, message_date, uid_validity)`
/// (+ PaperRust kwargs `folder`, `account_id`, see [`ActionTarget`]).
pub async fn apply_mail_action(worker: &MailWorker, kw: &Map<String, Value>) -> Result<ActionOutcome, pl_queue::TaskError> {
    let rule_id = kw_i32(kw, "rule_id")?;
    let rule = db::rule(&worker.pool, rule_id)
        .await
        .map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))?
        .ok_or_else(|| pl_queue::TaskError::fail("DoesNotExist", "MailRule matching query does not exist."))?;
    let account = db::account(&worker.pool, rule.account_id)
        .await
        .map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))?
        .ok_or_else(|| pl_queue::TaskError::fail("DoesNotExist", "MailAccount matching query does not exist."))?;
    let target = ActionTarget::from_kwargs(kw);
    let subject = kw_str(kw, "message_subject").unwrap_or_default();
    let date = received(kw, worker);

    let outcome = match run_mail_action(&worker.config.connect, &account, &rule, &target).await {
        Ok(o) => o,
        Err((t, m)) => ActionOutcome::Failed { error_type: t, message: m },
    };
    let (status, error) = match &outcome {
        ActionOutcome::Success => ("SUCCESS", None),
        ActionOutcome::Failed { error_type, message } => ("FAILED", Some(traceback_text(error_type, message))),
        ActionOutcome::Skipped { reason } => ("FAILED", Some(traceback_text("MailError", reason))),
    };
    let p = NewProcessed {
        owner_id: rule.owner_id,
        rule_id: rule.id,
        folder: target.record_folder(&rule),
        uid: &target.uid,
        uid_validity: target.uid_validity.as_deref(),
        subject: &subject,
        received: date,
        status,
        error: error.as_deref(),
    };
    db::create_processed(&worker.pool, &p).await.map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))?;
    Ok(outcome)
}

fn err_pair(e: ImapError) -> (String, String) {
    match e {
        ImapError::Mail(m) => ("MailError".into(), m),
        ImapError::Status { kind, message } => (kind.to_string(), message),
        ImapError::Other(m) => ("IMAP4.error".into(), m),
    }
}

/// `error_callback(request, exc, tb, rule_id, …)`: ProcessedMail FAILED (idempotent).
pub async fn error_callback(worker: &MailWorker, args: &[Value], kw: &Map<String, Value>) -> Result<(), pl_queue::TaskError> {
    let rule_id = kw_i32(kw, "rule_id")?;
    let rule = db::rule(&worker.pool, rule_id)
        .await
        .map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))?
        .ok_or_else(|| pl_queue::TaskError::fail("DoesNotExist", "MailRule matching query does not exist."))?;
    let target = ActionTarget::from_kwargs(kw);
    let subject = kw_str(kw, "message_subject").unwrap_or_default();
    let exc = args.get(1);
    let et = exc.and_then(|e| e.get("exc_type")).and_then(Value::as_str).unwrap_or("Exception");
    let em = exc.and_then(|e| e.get("exc_message")).and_then(Value::as_str).unwrap_or("");
    let error = match args.get(2).and_then(Value::as_str) {
        Some(tb) if !tb.is_empty() => tb.to_string(),
        _ => traceback_text(et, em),
    };
    let p = NewProcessed {
        owner_id: rule.owner_id,
        rule_id: rule.id,
        folder: target.record_folder(&rule),
        uid: &target.uid,
        uid_validity: target.uid_validity.as_deref(),
        subject: &subject,
        received: received(kw, worker),
        status: "FAILED",
        error: Some(&error),
    };
    db::get_or_create_processed(&worker.pool, &p).await.map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Handlers for pl-queue

pub(crate) async fn process_mail_accounts_task(w: Arc<MailWorker>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    let ids = match ctx.kwargs.get("account_ids").or_else(|| ctx.args.first()) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_i64).filter_map(|v| i32::try_from(v).ok()).collect()),
        Some(other) => return Err(pl_queue::TaskError::fail("TypeError", format!("invalid account_ids: {other}"))),
    };
    process_mail_accounts_checked(&w, ids).await.map(Value::String).map_err(|e| match e {
        FetchError::Database(m) => pl_queue::TaskError::fail("DatabaseError", m),
        FetchError::AllAccountsFailed(m) => pl_queue::TaskError::fail("MailError", m),
    })
}

pub(crate) async fn apply_mail_action_task(w: Arc<MailWorker>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    match apply_mail_action(&w, &ctx.kwargs).await? {
        ActionOutcome::Success => Ok(Value::Null),
        // upstream: `raise` after the FAILED entry
        ActionOutcome::Failed { error_type, message } => Err(pl_queue::TaskError::fail(error_type, message)),
        // the document was consumed; only the IMAP action was withheld (warning logged)
        ActionOutcome::Skipped { reason } => Ok(Value::String(reason)),
    }
}

pub(crate) async fn error_callback_task(w: Arc<MailWorker>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    error_callback(&w, &ctx.args, &ctx.kwargs).await.map(|()| Value::Null)
}
