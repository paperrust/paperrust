//! "Post-process existing archive" (PaperRust extension, cf. paperless-ai "scan"): a
//! job processes matching documents one after another with the automatic processing
//! ([`super::review::process_document`], trigger `manual`), with rate limiting
//! (`AiOptions::backfill_per_minute`), progress, cancellation and resuming.
//!
//! | Route | Rechte |
//! |---|---|
//! | `GET /api/x-ai/jobs/` | active superuser |
//! | `POST /api/x-ai/jobs/` `{"mode", "filter"}` | active superuser; mode like `/api/x-ai/run/` (M1) |
//! | `POST /api/x-ai/jobs/preview/` `{"filter"}` → `{"count"}` | active superuser |
//! | `GET /api/x-ai/jobs/{id}/` | active superuser |
//! | `POST /api/x-ai/jobs/{id}/cancel/`, `…/resume/` | active superuser |
//!
//! Execution: in the worker as task `paperrust.ai.backfill` (kwargs `job_id`, [`run_job_reporting`]),
//! state in `paperrust_ai_job`. Only without a hooked-in queue (crate tests, tools) does the
//! job run in its own process (`tokio::spawn`).
//!
//! Lease (review finding 2): the worker claims the job atomically (`queued` → `running`, random
//! `lease_owner`) and renews `heartbeat` every [`HEARTBEAT_EVERY`] – also while one slow document is
//! being processed. A running job counts as `interrupted` only once its lease expired
//! ([`LEASE_TTL`] without heartbeat, i.e. the worker died); every write of the worker is fenced on its
//! `lease_owner`, so a worker that lost its lease (job interrupted and resumed elsewhere) stops after
//! the current document. "Resume" continues from `last_document_id` with the persisted new-object
//! budget (`new_objects_used`, finding 3). Enqueued jobs that have not started yet count as
//! interrupted only after 6 hours (the worker may be busy with other tasks). At most one queued or
//! running job at a time, guarded by the unique partial index `paperrust_ai_job_one_active`.
//!
//! Per document (finding 1) the worker reloads the job owner (still an active superuser?), the
//! global AI switch and the effective mode (`apply` falls back to `suggest` as soon as the server
//! only allows suggestions). An unreadable stored filter fails the job (finding 4).
//! Only documents that the requester may change are processed.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthUser, User};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use sea_query::{Alias as A, Expr, Query, SelectStatement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

use super::store::{body_json, require_superuser, storage_err};
use crate::config::AutoMode;

/// Selection of documents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobFilter {
    /// Only documents with at least one of these tags.
    pub tags: Vec<i64>,
    /// Only documents without a correspondent.
    pub without_correspondent: bool,
    /// Only documents without a document type.
    pub without_document_type: bool,
    /// Only documents without tags.
    pub without_tags: bool,
    /// Issue date from/to (inclusive).
    pub created_from: Option<chrono::NaiveDate>,
    pub created_to: Option<chrono::NaiveDate>,
    /// Only documents that the AI has never looked at (no review-list entry).
    pub only_unprocessed: bool,
    /// At most this many documents (0 = limit from the options).
    pub max_documents: u32,
}

/// `SELECT d.id FROM documents_document d WHERE …` (ascending by id, from `after`).
pub fn filter_query(user: &User, f: &JobFilter, after: Option<i64>) -> SelectStatement {
    let mut q = Query::select();
    q.column((A::new("d"), A::new("id")))
        .from_as(A::new("documents_document"), A::new("d"))
        .and_where(Expr::col((A::new("d"), A::new("deleted_at"))).is_null())
        .and_where(Expr::col((A::new("d"), A::new("root_document_id"))).is_null())
        .and_where(Expr::col((A::new("d"), A::new("id"))).in_subquery(pl_authz::objects::permitted_document_ids(
            Some(user),
            "change_document",
            false,
        )));
    if !f.tags.is_empty() {
        let ids: Vec<i32> = f.tags.iter().filter_map(|t| i32::try_from(*t).ok()).collect();
        let mut sub = Query::select();
        sub.column(A::new("document_id")).from(A::new("documents_document_tags")).and_where(Expr::col(A::new("tag_id")).is_in(ids));
        q.and_where(Expr::col((A::new("d"), A::new("id"))).in_subquery(sub));
    }
    if f.without_tags {
        let mut sub = Query::select();
        sub.column(A::new("document_id")).from(A::new("documents_document_tags"));
        q.and_where(Expr::col((A::new("d"), A::new("id"))).not_in_subquery(sub));
    }
    if f.without_correspondent {
        q.and_where(Expr::col((A::new("d"), A::new("correspondent_id"))).is_null());
    }
    if f.without_document_type {
        q.and_where(Expr::col((A::new("d"), A::new("document_type_id"))).is_null());
    }
    if let Some(d) = f.created_from {
        q.and_where(Expr::col((A::new("d"), A::new("created"))).gte(d));
    }
    if let Some(d) = f.created_to {
        q.and_where(Expr::col((A::new("d"), A::new("created"))).lte(d));
    }
    if f.only_unprocessed {
        let mut sub = Query::select();
        sub.column(A::new("document_id")).from(A::new("paperrust_ai_review"));
        q.and_where(Expr::col((A::new("d"), A::new("id"))).not_in_subquery(sub));
    }
    if let Some(a) = after.and_then(|a| i32::try_from(a).ok()) {
        q.and_where(Expr::col((A::new("d"), A::new("id"))).gt(a));
    }
    q.order_by((A::new("d"), A::new("id")), sea_query::Order::Asc);
    q
}

const JOB_COLUMNS: &str = "id, created, created_by_id, run_id, mode, filter, status, total, processed, applied, suggested, errors, last_document_id, cancel_requested, heartbeat, finished, message, new_objects_used";

/// A running job without heartbeat for this long counts as interrupted (lease expired).
pub const LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(120);
/// Heartbeat interval of the worker (well below [`LEASE_TTL`]).
pub const HEARTBEAT_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Marks stuck jobs as `interrupted`: running jobs whose lease expired (no heartbeat for
/// [`LEASE_TTL`]) and queued jobs older than 6 hours. Clears the lease, which fences out the old
/// worker should it still be alive.
const MARK_STALE_SQL: &str = "UPDATE paperrust_ai_job SET status = 'interrupted', lease_owner = NULL, message = 'server restarted' \
     WHERE (status = 'running' AND COALESCE(heartbeat, created) < now() - make_interval(secs => $1)) \
        OR (status = 'queued' AND COALESCE(heartbeat, created) < now() - interval '6 hours')";

/// Claim: `queued` → `running` with this worker's lease (no row = someone else has it / not queued).
const CLAIM_SQL: &str = "UPDATE paperrust_ai_job SET status = 'running', lease_owner = $2, heartbeat = now() \
     WHERE id = $1 AND status = 'queued' RETURNING id";

/// Heartbeat, fenced on the lease (0 rows = lease lost).
const HEARTBEAT_SQL: &str = "UPDATE paperrust_ai_job SET heartbeat = now() WHERE id = $1 AND lease_owner = $2 AND status = 'running'";

fn job_json(r: &sqlx::postgres::PgRow) -> Value {
    let dt =
        |k: &str| r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(k).ok().flatten().map(|d| pl_core::datetime::python_isoformat(&d));
    let n = |k: &str| r.try_get::<i32, _>(k).unwrap_or_default();
    json!({
        "id": r.try_get::<i64, _>("id").unwrap_or_default(),
        "created": dt("created"),
        "created_by": r.try_get::<Option<i32>, _>("created_by_id").ok().flatten(),
        "run_id": r.try_get::<uuid::Uuid, _>("run_id").map(|u| u.to_string()).unwrap_or_default(),
        "mode": r.try_get::<String, _>("mode").unwrap_or_default(),
        "filter": r.try_get::<Value, _>("filter").unwrap_or(Value::Null),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
        "total": n("total"),
        "processed": n("processed"),
        "applied": n("applied"),
        "suggested": n("suggested"),
        "errors": n("errors"),
        "cancel_requested": r.try_get::<bool, _>("cancel_requested").unwrap_or(false),
        "heartbeat": dt("heartbeat"),
        "finished": dt("finished"),
        "message": r.try_get::<Option<String>, _>("message").ok().flatten(),
        "new_objects_used": n("new_objects_used"),
    })
}

/// Mark stuck jobs (lease expired) as `interrupted` ([`MARK_STALE_SQL`]).
async fn mark_stale(state: &AppState) {
    if let Err(e) = sqlx::query(MARK_STALE_SQL).bind(LEASE_TTL.as_secs_f64()).execute(state.db()).await {
        tracing::warn!(target: "pl_ai", error = %e, "marking stale AI jobs failed");
    }
}

/// `23505` on the unique partial index `paperrust_ai_job_one_active` → 409, other errors as usual.
fn insert_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505")) {
        return busy_conflict();
    }
    storage_err(e)
}

fn busy_conflict() -> ApiError {
    ApiError::json(StatusCode::CONFLICT, json!({"detail": "Another AI job is running."}))
}

async fn count(state: &AppState, user: &User, f: &JobFilter) -> Result<i64, ApiError> {
    let mut q = filter_query(user, f, None);
    q.clear_order_by();
    let mut c = Query::select();
    c.expr(Expr::cust("count(*)")).from_subquery(q, A::new("x"));
    let (sql, values) = c.build_sqlx(sea_query::PostgresQueryBuilder);
    sqlx::query_scalar_with(&sql, values).fetch_one(state.db()).await.map_err(ApiError::internal)
}

fn parse_filter(b: &Value) -> Result<JobFilter, ApiError> {
    serde_json::from_value(b.get("filter").cloned().unwrap_or(json!({})))
        .map_err(|_| ApiError::validation(json!({"filter": ["Invalid filter."]})))
}

/// POST /api/x-ai/jobs/preview/ → `{"count"}` (how many documents match).
pub async fn preview(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?;
    let f = parse_filter(&body_json(&body)?)?;
    let opts = super::store::load_options(state.db()).await;
    let n = count(&state, user, &f).await?;
    let max = if f.max_documents == 0 { opts.backfill_max_documents } else { f.max_documents.min(opts.backfill_max_documents) };
    Ok(Drf::ok(json!({"count": n, "limit": max, "per_minute": opts.backfill_per_minute})).indent(ctx.negotiation.indent).into_response())
}

/// GET /api/x-ai/jobs/
pub async fn list(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    mark_stale(&state).await;
    let rows = sqlx::query(&format!("SELECT {JOB_COLUMNS} FROM paperrust_ai_job ORDER BY created DESC, id DESC LIMIT 20"))
        .fetch_all(state.db())
        .await
        .map_err(storage_err)?;
    let results: Vec<Value> = rows.iter().map(job_json).collect();
    Ok(Drf::ok(json!({"count": results.len(), "results": results})).indent(ctx.negotiation.indent).into_response())
}

async fn load(state: &AppState, id: &str) -> Result<sqlx::postgres::PgRow, ApiError> {
    let id = super::py_int(id).ok_or_else(ApiError::not_found)?;
    sqlx::query(&format!("SELECT {JOB_COLUMNS} FROM paperrust_ai_job WHERE id = $1"))
        .bind(id)
        .fetch_optional(state.db())
        .await
        .map_err(storage_err)?
        .ok_or_else(ApiError::not_found)
}

/// GET /api/x-ai/jobs/{id}/
pub async fn retrieve(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    mark_stale(&state).await;
    let r = load(&state, &id).await?;
    Ok(Drf::ok(job_json(&r)).indent(ctx.negotiation.indent).into_response())
}

async fn busy(state: &AppState) -> Result<bool, ApiError> {
    mark_stale(state).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_ai_job WHERE status IN ('queued', 'running')")
        .fetch_one(state.db())
        .await
        .map_err(storage_err)?;
    Ok(n > 0)
}

/// POST /api/x-ai/jobs/ `{"mode": "suggest"|"apply", "filter": {…}}` → 201 with the job.
pub async fn create(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, body: Bytes) -> Result<Response, ApiError> {
    let user = require_superuser(&auth)?;
    let b = body_json(&body)?;
    let requested = match b.get("mode").and_then(Value::as_str).unwrap_or("suggest") {
        "suggest" => AutoMode::Suggest,
        "apply" => AutoMode::Apply,
        _ => return Err(ApiError::validation(json!({"mode": ["\"mode\" must be \"suggest\" or \"apply\"."]}))),
    };
    let cfg = super::effective_config(&state).await?;
    if !cfg.ai_enabled {
        return Err(ApiError::validation(json!({"non_field_errors": ["AI is disabled."]})));
    }
    let mode = super::review::manual_run_mode(requested, cfg.rs.auto_mode).map_err(ApiError::permission_denied_msg)?;
    let f = parse_filter(&b)?;
    // friendly pre-check; the atomic guard is the unique partial index (insert_err)
    if busy(&state).await? {
        return Err(busy_conflict());
    }
    let opts = super::store::load_options(state.db()).await;
    let max = if f.max_documents == 0 { opts.backfill_max_documents } else { f.max_documents.min(opts.backfill_max_documents) };
    let total = count(&state, user, &f).await?.min(i64::from(max));
    let run = uuid::Uuid::new_v4();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO paperrust_ai_job (created_by_id, run_id, mode, filter, status, total, heartbeat) VALUES ($1, $2, $3, $4, 'queued', $5, now()) RETURNING id",
    )
    .bind(user.id as i32)
    .bind(run)
    .bind(if mode == AutoMode::Apply { "apply" } else { "suggest" })
    .bind(serde_json::to_value(&f).unwrap_or(json!({})))
    .bind(i32::try_from(total).unwrap_or(i32::MAX))
    .fetch_one(state.db())
    .await
    .map_err(insert_err)?;
    enqueue_job(&state, id).await?;
    let r = load(&state, &id.to_string()).await?;
    Ok(Drf::created(job_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/jobs/{id}/cancel/
pub async fn cancel(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    let r = load(&state, &id).await?;
    let jid: i64 = r.try_get("id").map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE paperrust_ai_job SET cancel_requested = true, \
         status = CASE WHEN status = 'interrupted' THEN 'cancelled' ELSE status END, \
         finished = CASE WHEN status = 'interrupted' THEN now() ELSE finished END WHERE id = $1",
    )
    .bind(jid)
    .execute(state.db())
    .await
    .map_err(storage_err)?;
    let r = load(&state, &id).await?;
    Ok(Drf::ok(job_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// POST /api/x-ai/jobs/{id}/resume/ – resume an interrupted job.
pub async fn resume(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_superuser(&auth)?;
    mark_stale(&state).await;
    let r = load(&state, &id).await?;
    let jid: i64 = r.try_get("id").map_err(ApiError::internal)?;
    let status: String = r.try_get("status").unwrap_or_default();
    if status != "interrupted" {
        return Err(ApiError::validation(json!({"status": [format!("Job is {status}.")]})));
    }
    if busy(&state).await? {
        return Err(busy_conflict());
    }
    // atomic: only an interrupted job is requeued (a concurrent resume loses), the unique partial
    // index rejects a second active job (insert_err → 409)
    let n = sqlx::query(
        "UPDATE paperrust_ai_job SET status = 'queued', cancel_requested = false, lease_owner = NULL, heartbeat = now(), message = NULL \
         WHERE id = $1 AND status = 'interrupted'",
    )
    .bind(jid)
    .execute(state.db())
    .await
    .map_err(insert_err)?
    .rows_affected();
    if n == 0 {
        return Err(busy_conflict());
    }
    enqueue_job(&state, jid).await?;
    let r = load(&state, &id).await?;
    Ok(Drf::ok(job_json(&r)).indent(ctx.negotiation.indent).into_response())
}

/// Task name in the worker.
pub const BACKFILL_TASK: &str = "paperrust.ai.backfill";

/// Maximum runtime of the task (2000 documents at 6/min ≈ 5.5 h; the queue default would be 30 min).
pub const BACKFILL_TIME_LIMIT_SECS: f64 = 7.0 * 24.0 * 3600.0;

/// Enqueue a job (`paperrust.ai.backfill`, kwargs `job_id`). Without a hooked-in
/// queue, run in its own process. If enqueueing fails, the job is marked as `failed`
/// and 500 is reported.
async fn enqueue_job(state: &AppState, id: i64) -> Result<(), ApiError> {
    if pl_core::tasks::dispatcher(state).is_none() {
        let st = state.clone();
        tokio::spawn(async move { run_job_reporting(&st, id).await });
        return Ok(());
    }
    let mut req =
        pl_core::tasks::TaskRequest::new(BACKFILL_TASK).kwarg("job_id", id).trigger_source(pl_core::tasks::trigger_source::MANUAL);
    req.time_limit = (Some(BACKFILL_TIME_LIMIT_SECS), None);
    if let Err(e) = pl_core::tasks::send_task(state, req).await {
        mark_failed(state, id, "could not queue the job").await;
        return Err(e);
    }
    Ok(())
}

async fn mark_failed(state: &AppState, id: i64, msg: &str) {
    if let Err(e) = sqlx::query("UPDATE paperrust_ai_job SET status = 'failed', finished = now(), message = $2 WHERE id = $1")
        .bind(id)
        .bind(msg.chars().take(200).collect::<String>())
        .execute(state.db())
        .await
    {
        tracing::warn!(target: "pl_ai", job = id, error = %e, "marking the AI job as failed did not work");
    }
}

/// Marks the job as failed – only while it is still queued (not claimed) or still held by `lease`
/// (a worker that lost its lease must not overwrite the state of the new run).
async fn mark_failed_owned(state: &AppState, id: i64, lease: uuid::Uuid, msg: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE paperrust_ai_job SET status = 'failed', finished = now(), message = $3 \
         WHERE id = $1 AND ((status = 'queued' AND lease_owner IS NULL) OR (status = 'running' AND lease_owner = $2))",
    )
    .bind(id)
    .bind(lease)
    .bind(msg.chars().take(200).collect::<String>())
    .execute(state.db())
    .await
    {
        tracing::warn!(target: "pl_ai", job = id, error = %e, "marking the AI job as failed did not work");
    }
}

/// [`run_job`] and on error mark the job as `failed` (worker task `paperrust.ai.backfill`).
pub async fn run_job_reporting(state: &AppState, id: i64) -> Result<(), String> {
    let lease = uuid::Uuid::new_v4();
    let res = run_job_leased(state, id, lease).await;
    if let Err(e) = &res {
        tracing::warn!(target: "pl_ai", job = id, error = %e, "AI job failed");
        mark_failed_owned(state, id, lease, e).await;
    }
    res
}

/// Wait time between two documents.
pub fn pause_for(per_minute: u32) -> std::time::Duration {
    std::time::Duration::from_millis(60_000 / u64::from(per_minute.clamp(1, 120)))
}

/// Stored job filter; unreadable → error (review finding 4: never fall back to "all documents").
pub fn parse_stored_filter(v: Value) -> Result<JobFilter, String> {
    serde_json::from_value(v).map_err(|_| "stored job filter is unreadable; the job was not run".to_string())
}

/// Is the job owner still allowed to run a job (active superuser, like `POST /api/x-ai/jobs/`)?
pub fn owner_allowed(u: Option<&User>) -> bool {
    u.is_some_and(|u| u.is_active && u.is_superuser)
}

/// Mode for the next document: the stored mode limited by the current server setting
/// (`apply` only while `PAPERRUST_AI_AUTO_MODE=apply`, otherwise suggestions only).
pub fn effective_job_mode(stored: AutoMode, configured: AutoMode) -> AutoMode {
    super::review::manual_run_mode(stored, configured).unwrap_or(AutoMode::Suggest)
}

/// New objects left for the job (`max` from the current setting minus what the job already created).
pub fn budget_left(max: u32, used: i64) -> u32 {
    let used = u32::try_from(used.max(0)).unwrap_or(u32::MAX);
    max.saturating_sub(used)
}

/// Runs `fut` and calls `beat` every `every` meanwhile (lease renewal while a slow document is
/// processed). Returns the output and whether every heartbeat still held the lease.
pub async fn with_heartbeat<F, B, BF>(fut: F, every: std::time::Duration, mut beat: B) -> (F::Output, bool)
where
    F: std::future::Future,
    B: FnMut() -> BF,
    BF: std::future::Future<Output = bool>,
{
    tokio::pin!(fut);
    let mut held = true;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            out = &mut fut => return (out, held),
            _ = tick.tick() => {
                if held && !beat().await {
                    held = false;
                }
            }
        }
    }
}

/// One heartbeat; `false` = the lease is gone (DB errors count as "still held", the next fenced
/// write decides).
async fn heartbeat(state: &AppState, id: i64, lease: uuid::Uuid) -> bool {
    match sqlx::query(HEARTBEAT_SQL).bind(id).bind(lease).execute(state.db()).await {
        Ok(r) => r.rows_affected() > 0,
        Err(e) => {
            tracing::warn!(target: "pl_ai", job = id, error = %e, "AI job heartbeat failed");
            true
        }
    }
}

/// Final state change fenced on the lease; `false` = lease lost (nothing changed).
async fn finish(state: &AppState, id: i64, lease: uuid::Uuid, status: &str, message: Option<&str>) -> Result<bool, String> {
    let r = sqlx::query(
        "UPDATE paperrust_ai_job SET status = $3, finished = now(), message = COALESCE($4, message) \
         WHERE id = $1 AND lease_owner = $2 AND status = 'running'",
    )
    .bind(id)
    .bind(lease)
    .bind(status)
    .bind(message)
    .execute(state.db())
    .await
    .map_err(|e| e.to_string())?;
    Ok(r.rows_affected() > 0)
}

fn lease_lost(id: i64) -> Result<(), String> {
    tracing::warn!(target: "pl_ai", job = id, "AI job lost its lease (interrupted and taken over); this worker stops");
    Ok(())
}

/// Runs a job (also for the worker, task `paperrust.ai.backfill`, kwargs `job_id`) with a fresh lease.
/// Aborts if `cancel_requested` is set or AI was switched off.
pub async fn run_job(state: &AppState, id: i64) -> Result<(), String> {
    run_job_leased(state, id, uuid::Uuid::new_v4()).await
}

async fn run_job_leased(state: &AppState, id: i64, lease: uuid::Uuid) -> Result<(), String> {
    let r = sqlx::query(
        "SELECT created_by_id, run_id, mode, filter, total, processed, last_document_id, status, new_objects_used FROM paperrust_ai_job WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(state.db())
    .await
    .map_err(|e| e.to_string())?
    .ok_or("job not found")?;
    let status: String = r.try_get("status").map_err(|e| e.to_string())?;
    if status != "queued" {
        return Ok(());
    }
    let uid: Option<i32> = r.try_get("created_by_id").map_err(|e| e.to_string())?;
    let run: uuid::Uuid = r.try_get("run_id").map_err(|e| e.to_string())?;
    let stored_mode =
        if r.try_get::<String, _>("mode").map_err(|e| e.to_string())? == "apply" { AutoMode::Apply } else { AutoMode::Suggest };
    let f = parse_stored_filter(r.try_get::<Value, _>("filter").map_err(|e| e.to_string())?)?;
    let total = i64::from(r.try_get::<i32, _>("total").unwrap_or(0));
    let mut processed = i64::from(r.try_get::<i32, _>("processed").unwrap_or(0));
    let mut last: Option<i64> = r.try_get::<Option<i32>, _>("last_document_id").ok().flatten().map(i64::from);
    let mut used = i64::from(r.try_get::<i32, _>("new_objects_used").unwrap_or(0));
    let claimed: Option<i64> =
        sqlx::query_scalar(CLAIM_SQL).bind(id).bind(lease).fetch_optional(state.db()).await.map_err(|e| e.to_string())?;
    if claimed.is_none() {
        // another worker claimed it in the meantime, or it was cancelled
        return Ok(());
    }
    let mut mode = stored_mode;
    loop {
        let flags = sqlx::query("SELECT cancel_requested, lease_owner FROM paperrust_ai_job WHERE id = $1")
            .bind(id)
            .fetch_one(state.db())
            .await
            .map_err(|e| e.to_string())?;
        if flags.try_get::<Option<uuid::Uuid>, _>(1).ok().flatten() != Some(lease) {
            return lease_lost(id);
        }
        if flags.try_get::<bool, _>(0).unwrap_or(false) {
            if !finish(state, id, lease, "cancelled", None).await? {
                return lease_lost(id);
            }
            return Ok(());
        }
        // finding 1: current rights and settings for every document
        let user = match uid {
            Some(u) => pl_authz::user::load_user_by_id(state.db(), i64::from(u)).await.map_err(|e| e.to_string())?,
            None => None,
        };
        let Some(user) = user.filter(|u| owner_allowed(Some(u))) else {
            return Err("job owner is no longer an active superuser".into());
        };
        let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
        if !cfg.ai_enabled {
            if !finish(state, id, lease, "cancelled", Some("AI disabled")).await? {
                return lease_lost(id);
            }
            return Ok(());
        }
        let now_mode = effective_job_mode(stored_mode, cfg.rs.auto_mode);
        if now_mode != mode {
            mode = now_mode;
            tracing::info!(target: "pl_ai", job = id, "AI job continues in suggest mode (server only allows suggestions)");
            let _ = sqlx::query(
                "UPDATE paperrust_ai_job SET mode = 'suggest', message = 'switched to suggestions only (server setting)' \
                 WHERE id = $1 AND lease_owner = $2",
            )
            .bind(id)
            .bind(lease)
            .execute(state.db())
            .await;
        }
        if processed >= total {
            break;
        }
        let mut q = filter_query(&user, &f, last);
        q.limit(1);
        let (sql, values) = q.build_sqlx(sea_query::PostgresQueryBuilder);
        let next: Option<i32> = sqlx::query_scalar_with(&sql, values).fetch_optional(state.db()).await.map_err(|e| e.to_string())?;
        let Some(doc) = next.map(i64::from) else { break };
        let left_before = budget_left(cfg.rs.auto_max_new_objects, used);
        let mut left = left_before;
        let (res, held) = with_heartbeat(
            super::review::process_document(state, doc, mode, "manual", Some(&user), run, &mut left),
            HEARTBEAT_EVERY,
            || heartbeat(state, id, lease),
        )
        .await;
        let created = left_before.saturating_sub(left);
        used += i64::from(created);
        processed += 1;
        last = Some(doc);
        let (a, s, e) = match res.status.as_str() {
            "error" => (0, 0, 1),
            _ => (i32::from(res.applied > 0), i32::from(res.pending > 0), 0),
        };
        let n = sqlx::query(
            "UPDATE paperrust_ai_job SET processed = processed + 1, applied = applied + $2, suggested = suggested + $3, errors = errors + $4, \
             last_document_id = $5, new_objects_used = new_objects_used + $6, heartbeat = now() \
             WHERE id = $1 AND lease_owner = $7 AND status = 'running'",
        )
        .bind(id)
        .bind(a)
        .bind(s)
        .bind(e)
        .bind(doc as i32)
        .bind(i32::try_from(created).unwrap_or(i32::MAX))
        .bind(lease)
        .execute(state.db())
        .await
        .map_err(|e| e.to_string())?
        .rows_affected();
        if n == 0 || !held {
            return lease_lost(id);
        }
        // rate limiting (heartbeat while waiting, cancellation is detected on the next iteration)
        let opts = super::store::load_options(state.db()).await;
        let mut wait = pause_for(opts.backfill_per_minute);
        while !wait.is_zero() {
            let step = wait.min(HEARTBEAT_EVERY);
            tokio::time::sleep(step).await;
            wait -= step;
            if !heartbeat(state, id, lease).await {
                return lease_lost(id);
            }
        }
    }
    if !finish(state, id, lease, "done", None).await? {
        return lease_lost(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> User {
        User {
            id: 1,
            username: "admin".into(),
            password: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: true,
            is_staff: true,
            is_superuser: true,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: Vec::new(),
            permissions: Default::default(),
        }
    }

    #[test]
    fn filter_sql() {
        let f = JobFilter {
            tags: vec![3, 4],
            without_correspondent: true,
            without_document_type: true,
            created_from: chrono::NaiveDate::from_ymd_opt(2020, 1, 1),
            only_unprocessed: true,
            ..Default::default()
        };
        let (sql, values) = filter_query(&user(), &f, Some(17)).build_sqlx(sea_query::PostgresQueryBuilder);
        assert!(sql.contains(r#""d"."deleted_at" IS NULL"#), "{sql}");
        assert!(sql.contains(r#""d"."root_document_id" IS NULL"#), "{sql}");
        assert!(sql.contains(r#""tag_id" IN ("#), "{sql}");
        assert!(sql.contains(r#""d"."correspondent_id" IS NULL"#) && sql.contains(r#""d"."document_type_id" IS NULL"#), "{sql}");
        assert!(sql.contains(r#"NOT IN (SELECT "document_id" FROM "paperrust_ai_review")"#), "{sql}");
        assert!(sql.contains(r#""d"."id" > "#) && sql.split(" /*t:").next().unwrap_or(&sql).ends_with(r#"ORDER BY "d"."id" ASC"#), "{sql}");
        assert!(values.0.0.len() < 20);
        let f: JobFilter = serde_json::from_value(json!({"without_tags": true})).unwrap();
        let (sql, _) = filter_query(&user(), &f, None).build_sqlx(sea_query::PostgresQueryBuilder);
        assert!(sql.contains(r#"NOT IN (SELECT "document_id" FROM "documents_document_tags")"#), "{sql}");
        assert!(serde_json::from_value::<JobFilter>(json!({"tags": "x"})).is_err());
    }

    /// Finding 4: an unreadable stored filter is an error, never the empty default filter
    /// (which would select every document the owner may change).
    #[test]
    fn unreadable_filter_fails() {
        assert!(parse_stored_filter(json!({"tags": "x"})).is_err());
        assert!(parse_stored_filter(json!("garbage")).is_err());
        assert!(parse_stored_filter(Value::Null).is_err());
        assert_eq!(parse_stored_filter(json!({})).ok(), Some(JobFilter::default()));
        let f = parse_stored_filter(json!({"tags": [3], "without_tags": false})).ok();
        assert_eq!(f.map(|f| f.tags), Some(vec![3]));
    }

    /// Finding 1: the mode is re-evaluated against the current server setting; revoked superuser
    /// status or deactivation stops the job.
    #[test]
    fn mode_and_owner_rechecked() {
        use AutoMode::*;
        assert_eq!(effective_job_mode(Apply, Apply), Apply);
        assert_eq!(effective_job_mode(Apply, Suggest), Suggest, "switch to suggest-only stops changes");
        assert_eq!(effective_job_mode(Apply, Off), Suggest);
        assert_eq!(effective_job_mode(Suggest, Apply), Suggest);
        let mut u = user();
        assert!(owner_allowed(Some(&u)));
        u.is_superuser = false;
        assert!(!owner_allowed(Some(&u)));
        u.is_superuser = true;
        u.is_active = false;
        assert!(!owner_allowed(Some(&u)));
        assert!(!owner_allowed(None));
    }

    /// Finding 3: the budget for new objects is computed from the persisted counter.
    #[test]
    fn budget_survives_resume() {
        assert_eq!(budget_left(5, 0), 5);
        assert_eq!(budget_left(5, 3), 2, "resume keeps what was already used");
        assert_eq!(budget_left(5, 9), 0);
        assert_eq!(budget_left(5, -1), 5);
        assert!(JOB_COLUMNS.contains("new_objects_used"));
    }

    /// Finding 2: a slow document keeps the lease fresh (heartbeats during the work); a lost lease
    /// is reported; stale marking only hits expired leases and clears the owner; every worker
    /// write is fenced on the lease; at most one active job is enforced by a unique index.
    #[tokio::test]
    async fn heartbeat_during_slow_document() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let beats = Arc::new(AtomicUsize::new(0));
        let b = beats.clone();
        let slow = async {
            tokio::time::sleep(std::time::Duration::from_millis(220)).await;
            7
        };
        let (out, held) = with_heartbeat(slow, std::time::Duration::from_millis(20), move || {
            let b = b.clone();
            async move {
                b.fetch_add(1, Ordering::SeqCst);
                true
            }
        })
        .await;
        assert_eq!(out, 7);
        assert!(held);
        assert!(beats.load(Ordering::SeqCst) >= 5, "heartbeats while the document is processed");
        let slow = tokio::time::sleep(std::time::Duration::from_millis(80));
        let (_, held) = with_heartbeat(slow, std::time::Duration::from_millis(10), || async { false }).await;
        assert!(!held, "lost lease is reported");

        assert!(LEASE_TTL > HEARTBEAT_EVERY * 3);
        assert!(MARK_STALE_SQL.contains("lease_owner = NULL"), "{MARK_STALE_SQL}");
        assert!(CLAIM_SQL.contains("status = 'queued'") && CLAIM_SQL.contains("lease_owner = $2"), "{CLAIM_SQL}");
        assert!(HEARTBEAT_SQL.contains("lease_owner = $2"), "{HEARTBEAT_SQL}");
        let m = super::super::migrations::M0005_PAPERRUST_AI_JOB_LEASE;
        assert!(m.contains("CREATE UNIQUE INDEX IF NOT EXISTS paperrust_ai_job_one_active"), "{m}");
        assert!(m.contains("WHERE status IN ('queued', 'running')"), "{m}");
        assert!(m.contains("new_objects_used") && m.contains("lease_owner uuid"), "{m}");
    }

    #[test]
    fn pauses() {
        assert_eq!(pause_for(6), std::time::Duration::from_secs(10));
        assert_eq!(pause_for(0), std::time::Duration::from_secs(60));
        assert_eq!(pause_for(1000), std::time::Duration::from_millis(500));
    }
}
