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

/// `process_mail_accounts(account_ids=None)` → result text.
pub async fn process_mail_accounts(worker: &MailWorker, account_ids: Option<Vec<i32>>) -> Result<String, String> {
    let mut lock_conn = worker.pool.acquire().await.map_err(|e| e.to_string())?.detach();
    let (got,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(MAIL_FETCH_LOCK_ID)
        .fetch_one(&mut lock_conn)
        .await
        .map_err(|e| e.to_string())?;
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

async fn run_accounts(worker: &MailWorker, account_ids: Option<Vec<i32>>) -> Result<String, String> {
    let ids = account_ids.filter(|v| !v.is_empty());
    let accounts = db::accounts(&worker.pool, ids.as_deref()).await.map_err(|e| e.to_string())?;
    let mut total: i64 = 0;
    for account in accounts {
        if !db::has_enabled_rules(&worker.pool, account.id).await.map_err(|e| e.to_string())? {
            tracing::info!(target: "paperless.mail.tasks", "No rules enabled for account {}. Skipping.", account.name);
            continue;
        }
        let name = account.name.clone();
        let mut run = AccountRun::new(worker);
        match run.handle_mail_account(account).await {
            Ok(n) => total += n,
            Err(e) => tracing::error!(target: "paperless.mail.tasks", "Error while processing mail account {name}: {e}"),
        }
    }
    if total > 0 { Ok(format!("Added {total} document(s).")) } else { Ok("No new documents were added.".to_string()) }
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
    Failed { error_type: String, message: String },
}

/// `apply_mail_action(result, rule_id, message_uid, message_subject, message_date, uid_validity)`.
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
    let uid = kw_str(kw, "message_uid").unwrap_or_else(|| "None".into());
    let subject = kw_str(kw, "message_subject").unwrap_or_default();
    let uid_validity = kw_str(kw, "uid_validity");
    let date = received(kw, worker);

    let r: Result<(), (String, String)> = async {
        let port = u16::try_from(account.imap_port.unwrap_or(0)).unwrap_or(0);
        let mut mb = MailBox::connect(&account.imap_server, port, account.imap_security, &worker.config.connect).await.map_err(err_pair)?;
        let inner: Result<(), (String, String)> = async {
            let gmail = mb.capabilities.iter().any(|c| c == "X-GM-EXT-1");
            mb.login(&account.username, &account.password, account.is_token).await.map_err(err_pair)?;
            mb.folder_set(&rule.folder).await.map_err(err_pair)?;
            let action = criteria::get_rule_action(rule.action, rule.action_parameter.as_deref(), gmail).map_err(|e| match e {
                criteria::ActionError::Mail(m) => ("MailError".to_string(), m),
                criteria::ActionError::Value(m) => ("ValueError".to_string(), m),
            })?;
            if let Err(e) = post_consume(&mut mb, &action, &uid, rule.action_parameter.as_deref()).await {
                if matches!(e, ImapError::Status { .. }) {
                    tracing::error!(target: "paperless_mail", "Error while processing mail action during post_consume: {e}");
                }
                return Err(err_pair(e));
            }
            Ok(())
        }
        .await;
        let bye = mb.logout().await;
        inner?;
        if !bye {
            return Err(("MailboxLogoutError".into(), "Response status \"BYE\" expected".into()));
        }
        Ok(())
    }
    .await;

    let (status, error) = match &r {
        Ok(()) => ("SUCCESS", None),
        Err((t, m)) => ("FAILED", Some(traceback_text(t, m))),
    };
    let p = NewProcessed {
        owner_id: rule.owner_id,
        rule_id: rule.id,
        folder: &rule.folder,
        uid: &uid,
        uid_validity: uid_validity.as_deref(),
        subject: &subject,
        received: date,
        status,
        error: error.as_deref(),
    };
    db::create_processed(&worker.pool, &p).await.map_err(|e| pl_queue::TaskError::fail("DatabaseError", e.to_string()))?;
    match r {
        Ok(()) => Ok(ActionOutcome::Success),
        Err((t, m)) => Ok(ActionOutcome::Failed { error_type: t, message: m }),
    }
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
    let uid = kw_str(kw, "message_uid").unwrap_or_else(|| "None".into());
    let subject = kw_str(kw, "message_subject").unwrap_or_default();
    let uid_validity = kw_str(kw, "uid_validity");
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
        folder: &rule.folder,
        uid: &uid,
        uid_validity: uid_validity.as_deref(),
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
    process_mail_accounts(&w, ids).await.map(Value::String).map_err(|e| pl_queue::TaskError::fail("DatabaseError", e))
}

pub(crate) async fn apply_mail_action_task(w: Arc<MailWorker>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    match apply_mail_action(&w, &ctx.kwargs).await? {
        ActionOutcome::Success => Ok(Value::Null),
        // upstream: `raise` after the FAILED entry
        ActionOutcome::Failed { error_type, message } => Err(pl_queue::TaskError::fail(error_type, message)),
    }
}

pub(crate) async fn error_callback_task(w: Arc<MailWorker>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    error_callback(&w, &ctx.args, &ctx.kwargs).await.map(|()| Value::Null)
}
