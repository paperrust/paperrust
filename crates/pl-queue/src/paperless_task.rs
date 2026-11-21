//! `documents_paperlesstask` lifecycle exactly like the Celery signal handlers upstream.
//!
//! upstream: src/documents/signals/handlers.py (`before_task_publish_handler`,
//! `task_prerun_handler`, `task_postrun_handler`, `task_failure_handler`, `task_revoked_handler`,
//! `_extract_input_data`, `_determine_trigger_source`, `_extract_owner_id`),
//! src/documents/models.py (`PaperlessTask`).
//!
//! All functions do nothing for untracked tasks (`TRACKED_TASKS`). Errors are
//! returned; the caller only logs them (upstream: `logger.exception(...)`, no abort).

use chrono::{DateTime, Utc};
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Map, Value};
use sqlx::Row;

use crate::queue::QueueError;
use crate::registry::tracked_task_type;

fn t() -> Alias {
    Alias::new("documents_paperlesstask")
}
fn c(name: &str) -> Alias {
    Alias::new(name)
}

/// `_extract_input_data(task_type, kwargs)`.
///
/// `consume_file`: in PaperRust `input_doc`/`overrides` are JSON (fields like the Python
/// dataclasses `ConsumableDocument`/`DocumentMetadataOverrides`, see contract);
/// `original_file` → `filename` = last path component (`Path.name`).
pub fn extract_input_data(task_type: &str, kwargs: &Map<String, Value>) -> Value {
    let mut m = Map::new();
    match task_type {
        "consume_file" => {
            let Some(input_doc) = kwargs.get("input_doc").filter(|v| !v.is_null()) else {
                return Value::Object(m);
            };
            let original = input_doc.get("original_file").and_then(Value::as_str).unwrap_or("");
            m.insert("filename".into(), Value::String(path_name(original).to_string()));
            m.insert("mime_type".into(), input_doc.get("mime_type").cloned().unwrap_or(Value::Null));
            if let Some(p) = input_doc.get("original_path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                m.insert("source_path".into(), Value::String(p.to_string()));
            }
            if let Some(r) = input_doc.get("mailrule_id").filter(|v| truthy(v)) {
                m.insert("mailrule_id".into(), r.clone());
            }
            if let Some(Value::Object(ov)) = kwargs.get("overrides") {
                let od: Map<String, Value> =
                    ov.iter().filter(|(k, v)| !v.is_null() && !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
                if !od.is_empty() {
                    m.insert("overrides".into(), Value::Object(od));
                }
            }
        }
        "mail_fetch" => {
            if let Some(v) = kwargs.get("account_ids").filter(|v| !v.is_null()) {
                m.insert("account_ids".into(), v.clone());
            }
        }
        "apply_ai_suggestions" => {
            if let Some(v) = kwargs.get("document_id").filter(|v| !v.is_null()) {
                m.insert("document_id".into(), v.clone());
            }
        }
        _ => {}
    }
    Value::Object(m)
}

/// Python truth value for JSON.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `PurePath.name` (POSIX and Windows separators).
fn path_name(p: &str) -> &str {
    let p = p.trim_end_matches(['/', '\\']);
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// `_extract_owner_id`: only `consume_file`, from `overrides.owner_id`.
pub fn extract_owner_id(task_type: &str, kwargs: &Map<String, Value>) -> Option<i64> {
    if task_type != "consume_file" {
        return None;
    }
    kwargs.get("overrides")?.get("owner_id")?.as_i64()
}

/// Background bulk change (upstream dev 60a35edee, `before_task_publish_handler`):
/// `input_data = {operation, document_count}`, owner = triggering user.
pub const BULK_OPERATION_TASK: &str = "documents.bulk_operations.execute_bulk_operation";

/// `input_data` per task (special case [`BULK_OPERATION_TASK`], otherwise [`extract_input_data`]).
pub fn input_data_for(task_name: &str, task_type: &str, kwargs: &Map<String, Value>) -> Value {
    if task_name == BULK_OPERATION_TASK {
        let count = kwargs.get("document_ids").and_then(Value::as_array).map_or(0, Vec::len);
        return serde_json::json!({
            "operation": kwargs.get("operation").cloned().unwrap_or(Value::Null),
            "document_count": count,
        });
    }
    extract_input_data(task_type, kwargs)
}

/// `owner_id` per task (special case [`BULK_OPERATION_TASK`]: `user_id`).
pub fn owner_id_for(task_name: &str, task_type: &str, kwargs: &Map<String, Value>) -> Option<i64> {
    if task_name == BULK_OPERATION_TASK {
        return kwargs.get("user_id").and_then(Value::as_i64);
    }
    extract_owner_id(task_type, kwargs)
}

/// `before_task_publish_handler`: `get_or_create(task_id, defaults=…)` with `status=pending`.
/// Returns `true` if the task is tracked.
pub async fn record_pending<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    task_name: &str,
    task_id: &str,
    kwargs: &Map<String, Value>,
    headers: &Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<bool, QueueError> {
    let Some(task_type) = tracked_task_type(task_name) else {
        return Ok(false);
    };
    let mut q = Query::insert();
    q.into_table(t())
        .columns([
            c("task_id"),
            c("task_type"),
            c("trigger_source"),
            c("status"),
            c("date_created"),
            c("input_data"),
            c("acknowledged"),
            c("owner_id"),
        ])
        .values([
            task_id.into(),
            task_type.into(),
            pl_core::tasks::determine_trigger_source(headers).into(),
            "pending".into(),
            now.into(),
            input_data_for(task_name, task_type, kwargs).into(),
            false.into(),
            owner_id_for(task_name, task_type, kwargs).into(),
        ])?
        .on_conflict(OnConflict::column(c("task_id")).do_nothing().to_owned());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(executor).await?;
    Ok(true)
}

/// `task_prerun_handler`: `status=started, date_started=now`.
pub async fn mark_started<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    task_name: &str,
    task_id: &str,
    now: DateTime<Utc>,
) -> Result<(), QueueError> {
    if tracked_task_type(task_name).is_none() {
        return Ok(());
    }
    let (sql, values) = Query::update()
        .table(t())
        .values([(c("status"), "started".into()), (c("date_started"), now.into())])
        .and_where(Expr::col(c("task_id")).eq(task_id))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(executor).await?;
    Ok(())
}

fn secs(d: chrono::Duration) -> f64 {
    d.num_microseconds().map_or(d.num_seconds() as f64, |us| us as f64 / 1_000_000.0)
}

async fn load_dates(conn: &mut sqlx::PgConnection, task_id: &str) -> Result<Option<(Option<DateTime<Utc>>, DateTime<Utc>)>, QueueError> {
    let (sql, values) = Query::select()
        .columns([c("date_started"), c("date_created")])
        .from(t())
        .and_where(Expr::col(c("task_id")).eq(task_id))
        .build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(&mut *conn).await?;
    Ok(match row {
        Some(r) => Some((r.try_get("date_started")?, r.try_get("date_created")?)),
        None => None,
    })
}

/// `task_postrun_handler` with `state=SUCCESS`: `status=success` (or `failure` if the
/// result is a dict with `duplicate_of`), `date_done`, `duration_seconds` (if
/// started), `wait_time_seconds`, `result_data` (only for a dict result).
pub async fn mark_success(
    conn: &mut sqlx::PgConnection,
    task_name: &str,
    task_id: &str,
    retval: &Value,
    now: DateTime<Utc>,
) -> Result<(), QueueError> {
    if tracked_task_type(task_name).is_none() {
        return Ok(());
    }
    let Some((started, created)) = load_dates(conn, task_id).await? else {
        return Ok(());
    };
    let mut status = "success";
    let mut q = Query::update();
    q.table(t()).value(c("date_done"), now);
    if let Some(s) = started {
        q.value(c("duration_seconds"), secs(now - s));
        q.value(c("wait_time_seconds"), secs(s - created));
    }
    if let Value::Object(m) = retval {
        q.value(c("result_data"), retval.clone());
        if m.contains_key("duplicate_of") {
            status = "failure";
        }
    }
    q.value(c("status"), status).and_where(Expr::col(c("task_id")).eq(task_id));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    Ok(())
}

/// Python `str[:5000]` (characters, not bytes).
fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `result_data` of the `task_failure_handler`.
pub fn failure_result_data(error_type: Option<&str>, error_message: Option<&str>, traceback: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("error_type".into(), Value::String(error_type.unwrap_or("Unknown").to_string()));
    m.insert("error_message".into(), Value::String(error_message.unwrap_or("Unknown error").to_string()));
    if let Some(tb) = traceback.filter(|s| !s.is_empty()) {
        m.insert("traceback".into(), Value::String(truncate_chars(tb, 5000)));
    }
    Value::Object(m)
}

/// `task_failure_handler`: `status=failure`, `result_data`, `date_done`, duration/wait time.
pub async fn mark_failure(
    conn: &mut sqlx::PgConnection,
    task_name: &str,
    task_id: &str,
    result_data: &Value,
    now: DateTime<Utc>,
) -> Result<(), QueueError> {
    if tracked_task_type(task_name).is_none() {
        return Ok(());
    }
    let Some((started, created)) = load_dates(conn, task_id).await? else {
        return Ok(());
    };
    let mut q = Query::update();
    q.table(t()).value(c("status"), "failure").value(c("result_data"), result_data.clone()).value(c("date_done"), now);
    if let Some(s) = started {
        q.value(c("duration_seconds"), secs(now - s));
        q.value(c("wait_time_seconds"), secs(s - created));
    }
    q.and_where(Expr::col(c("task_id")).eq(task_id));
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    Ok(())
}

/// Reason for `task_revoked_handler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// revoked before start
    Revoked,
    /// `expires` exceeded
    Expired,
    /// terminated during execution (`terminate=True`)
    Terminated,
}

impl RevokeReason {
    /// `result_data = {"reason": "<…> Check worker logs for details."}`.
    pub fn text(self) -> &'static str {
        match self {
            Self::Revoked => "Task revoked before completion. Check worker logs for details.",
            Self::Expired => "Task expired before execution. Check worker logs for details.",
            Self::Terminated => "Task terminated during execution. Check worker logs for details.",
        }
    }
}

/// `task_revoked_handler`: `status=revoked, date_done=now, result_data={"reason": …}`.
pub async fn mark_revoked<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    task_name: &str,
    task_id: &str,
    reason: RevokeReason,
    now: DateTime<Utc>,
) -> Result<(), QueueError> {
    if tracked_task_type(task_name).is_none() {
        return Ok(());
    }
    let (sql, values) = Query::update()
        .table(t())
        .values([
            (c("status"), "revoked".into()),
            (c("date_done"), now.into()),
            (c("result_data"), serde_json::json!({"reason": reason.text()}).into()),
        ])
        .and_where(Expr::col(c("task_id")).eq(task_id))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(executor).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn input_data_like_upstream() {
        assert_eq!(extract_input_data("mail_fetch", &obj(json!({"account_ids": [1, 2]}))), json!({"account_ids": [1, 2]}));
        assert_eq!(extract_input_data("mail_fetch", &obj(json!({"account_ids": null}))), json!({}));
        assert_eq!(extract_input_data("apply_ai_suggestions", &obj(json!({"action_id": 1, "document_id": 7}))), json!({"document_id": 7}));
        assert_eq!(extract_input_data("bulk_update", &obj(json!({"document_ids": [1]}))), json!({}));
        let kw = obj(json!({
            "input_doc": {"source": 1, "original_file": "/tmp/scratch/a/Scan 01.pdf", "mime_type": "application/pdf", "original_path": null, "mailrule_id": null},
            "overrides": {"title": "T", "owner_id": 3, "tag_ids": null, "_private": 1, "created": "2024-01-02"}
        }));
        assert_eq!(
            extract_input_data("consume_file", &kw),
            json!({"filename": "Scan 01.pdf", "mime_type": "application/pdf", "overrides": {"title": "T", "owner_id": 3, "created": "2024-01-02"}})
        );
        assert_eq!(extract_owner_id("consume_file", &kw), Some(3));
        assert_eq!(extract_owner_id("bulk_update", &kw), None);
        let bk = obj(json!({"operation": "add_tag", "document_ids": [1, 2, 3], "parameters": {"tag": 4}, "user_id": 9}));
        assert_eq!(input_data_for(BULK_OPERATION_TASK, "bulk_update", &bk), json!({"operation": "add_tag", "document_count": 3}));
        assert_eq!(owner_id_for(BULK_OPERATION_TASK, "bulk_update", &bk), Some(9));
        assert_eq!(owner_id_for("documents.tasks.bulk_update_documents", "bulk_update", &bk), None);
        assert_eq!(extract_input_data("consume_file", &Map::new()), json!({}));
    }

    #[test]
    fn failure_data_like_upstream() {
        let tb = "x".repeat(6000);
        let v = failure_result_data(Some("ValueError"), Some("boom"), Some(&tb));
        assert_eq!(v["error_type"], "ValueError");
        assert_eq!(v["traceback"].as_str().unwrap().len(), 5000);
        assert_eq!(failure_result_data(None, None, None), json!({"error_type": "Unknown", "error_message": "Unknown error"}));
        assert_eq!(RevokeReason::Expired.text(), "Task expired before execution. Check worker logs for details.");
    }
}
