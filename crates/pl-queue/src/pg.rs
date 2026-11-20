//! [`PgQueue`]: queue in PostgreSQL (`paperrust_task_queue`, `FOR UPDATE SKIP LOCKED`).
//!
//! Replaces the Redis broker + Celery worker protocol. Task behaviour (status,
//! `PaperlessTask`, retries, expiry, revocation, callbacks, groups/chords) like Celery 5.6.3 with
//! the paperless settings (`CELERY_TASK_TRACK_STARTED`, `CELERY_TASK_TIME_LIMIT`,
//! `CELERY_TASK_ALLOW_ERROR_CB_ON_CHORD_HEADER`), see docs/contract/pl-queue.md.
//!
//! **SQL:** The queue queries are raw SQL (commented), because `FOR UPDATE SKIP LOCKED`
//! in a CTE with `UPDATE … FROM … RETURNING` has no sea-query form. They only use
//! PostgreSQL ≥ 9.5. For a later SQLite backend (single process) `SKIP LOCKED` is dropped;
//! there `UPDATE … WHERE id = (SELECT … LIMIT 1) RETURNING` in an `IMMEDIATE`
//! transaction is enough. Writes to `documents_paperlesstask` go through sea-query
//! ([`crate::paperless_task`]).
//!
//! **Fencing:** Every state change of a running job checks `(id, attempts)` and
//! `state='running' AND locked_by=<me>` **or** `state='queued'` (lease expired and put back by the
//! cleaner, but not yet claimed anew by anyone – `attempts` is then
//! unchanged; the original worker may then still finish or take it back via heartbeat).
//! As soon as another worker has claimed the job (`attempts + 1`), the old one
//! can no longer overwrite anything → no double effect on queue/PaperlessTask state; the old
//! worker aborts execution at the next heartbeat or at the latest after 80 % of the lease without a
//! successful heartbeat (self-fencing in the worker).

use chrono::{DateTime, Duration, Utc};
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use crate::paperless_task::{self as pt, RevokeReason};
use crate::queue::{EnqueueOptions, QueueError, Signature, TaskInfo, TaskQueue, TaskStatus};

/// Channel for `LISTEN/NOTIFY` on enqueue (payload = queue name).
pub const QUEUE_NOTIFY_CHANNEL: &str = "paperrust_task_queue";

/// A job claimed by the worker.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub task_id: String,
    pub task_name: String,
    pub queue: String,
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
    pub headers: Map<String, Value>,
    /// Deliveries since the last enqueue (fencing token).
    pub attempts: i32,
    /// `request.retries`.
    pub retries: i32,
    pub expires: Option<DateTime<Utc>>,
    pub time_limit: Option<f64>,
    pub revoke_requested: bool,
    pub link: Vec<Signature>,
    pub link_error: Vec<Signature>,
    pub group_id: Option<String>,
    pub group_index: Option<i32>,
    pub locked_by: String,
    /// Time of the claim (start of the lease, local clock).
    pub claimed_at: std::time::Instant,
}

impl Job {
    pub fn uuid(&self) -> Option<Uuid> {
        Uuid::parse_str(&self.task_id).ok()
    }
}

/// Failure details of a job (`task_failure`: exception type, text, traceback).
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub error_type: String,
    pub message: String,
    pub traceback: Option<String>,
}

impl Failure {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self { error_type: error_type.into(), message: message.into(), traceback: None }
    }
    fn result_data(&self) -> Value {
        pt::failure_result_data(Some(&self.error_type), Some(&self.message), self.traceback.as_deref())
    }
}

/// Result of a heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heartbeat {
    /// Lease extended.
    Ok,
    /// Lease extended, `revoke(terminate=True)` requested.
    Terminate,
    /// Lease lost (expired and reassigned/finished) → abort execution.
    Lost,
}

/// Report of the cleaner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Newly enqueued jobs (worker lost, deliveries left).
    pub requeued: Vec<String>,
    /// Permanently failed jobs (`WorkerLostError`).
    pub failed: Vec<String>,
}

/// PostgreSQL queue.
#[derive(Debug, Clone)]
pub struct PgQueue {
    pool: PgPool,
    max_deliveries: i32,
}

const JOB_COLUMNS: &str = "q.id, q.task_id, q.task_name, q.queue, q.args, q.kwargs, q.headers, q.attempts, q.retries, \
     q.expires, q.time_limit, q.revoke_requested, q.link, q.link_error, q.group_id, q.group_index, q.locked_by";

fn sigs(v: Value) -> Vec<Signature> {
    serde_json::from_value(v).unwrap_or_default()
}

fn job_from_row(r: &PgRow) -> Result<Job, sqlx::Error> {
    let obj = |v: Value| match v {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    Ok(Job {
        id: r.try_get("id")?,
        task_id: r.try_get("task_id")?,
        task_name: r.try_get("task_name")?,
        queue: r.try_get("queue")?,
        args: match r.try_get::<Value, _>("args")? {
            Value::Array(a) => a,
            _ => Vec::new(),
        },
        kwargs: obj(r.try_get("kwargs")?),
        headers: obj(r.try_get("headers")?),
        attempts: r.try_get("attempts")?,
        retries: r.try_get("retries")?,
        expires: r.try_get("expires")?,
        time_limit: r.try_get("time_limit")?,
        revoke_requested: r.try_get("revoke_requested")?,
        link: sigs(r.try_get("link")?),
        link_error: sigs(r.try_get("link_error")?),
        group_id: r.try_get("group_id")?,
        group_index: r.try_get("group_index")?,
        locked_by: r.try_get::<Option<String>, _>("locked_by")?.unwrap_or_default(),
        claimed_at: std::time::Instant::now(),
    })
}

/// Additional columns on enqueue (group membership).
#[derive(Debug, Clone, Default)]
struct Membership {
    group_id: Option<String>,
    group_index: Option<i32>,
}

/// Errback arguments `(request, exc, traceback)` as JSON (see contract).
fn errback_args(job: &Job, f: &Failure) -> Vec<Value> {
    vec![
        json!({"id": job.task_id, "task": job.task_name, "args": job.args, "kwargs": job.kwargs, "group": job.group_id, "retries": job.retries}),
        json!({"exc_type": f.error_type, "exc_message": f.message}),
        f.traceback.clone().map_or(Value::Null, Value::String),
    ]
}

/// `Signature._merge`: new arguments **before** the own ones; `immutable` (`.si()`) ignores them.
fn with_args(sig: &Signature, prefix: Vec<Value>) -> (Map<String, Value>, EnqueueOptions) {
    let mut opts = sig.options.clone();
    let mut args = if sig.immutable { Vec::new() } else { prefix };
    args.extend(sig.args.iter().cloned());
    opts.args = args;
    (sig.kwargs.clone(), opts)
}

impl PgQueue {
    /// New queue on `pool`. `max_deliveries` see `QueueConfig`.
    pub fn new(pool: PgPool, max_deliveries: i32) -> Self {
        Self { pool, max_deliveries: max_deliveries.max(1) }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Enqueue in the caller's transaction/connection (atomic with its changes –
    /// replaces Celery's `transaction.on_commit` pattern). Tracked tasks get their
    /// `PaperlessTask` (`get_or_create`, `pending`). Enqueueing the same `task_id` again is a
    /// no-op (idempotent).
    pub async fn enqueue_in(&self, conn: &mut PgConnection, task_name: &str, kwargs: Map<String, Value>, opts: EnqueueOptions) -> Result<Uuid, QueueError> {
        self.enqueue_member(conn, task_name, kwargs, opts, Membership::default(), Utc::now()).await
    }

    async fn enqueue_member(
        &self,
        conn: &mut PgConnection,
        task_name: &str,
        kwargs: Map<String, Value>,
        opts: EnqueueOptions,
        member: Membership,
        now: DateTime<Utc>,
    ) -> Result<Uuid, QueueError> {
        if task_name.is_empty() || task_name.len() > 255 {
            return Err(QueueError::Invalid("task name".into()));
        }
        let priority = opts.priority.unwrap_or(0);
        if !(0..=9).contains(&priority) {
            return Err(QueueError::Invalid("priority must be 0..=9".into()));
        }
        let task_id = opts.task_id.unwrap_or_else(Uuid::new_v4);
        let id_s = task_id.to_string();
        let headers = opts.all_headers();
        let eta = match (opts.eta, opts.countdown) {
            (Some(eta), _) => eta,
            (None, Some(c)) if c.is_finite() && c > 0.0 => now + Duration::microseconds((c * 1e6) as i64),
            _ => now,
        };
        let expires = opts.expires.filter(|e| e.is_finite()).map(|e| now + Duration::microseconds((e * 1e6) as i64));
        let queue = opts.queue.clone().unwrap_or_else(|| crate::DEFAULT_QUEUE.to_string());
        // before_task_publish → PaperlessTask (upstream: error only logged; here in the same
        // transaction, an error aborts the enqueue so that both stay consistent).
        pt::record_pending(&mut *conn, task_name, &id_s, &kwargs, &headers, now).await?;
        // Raw SQL: the table belongs to pl-queue (see module comment).
        sqlx::query(
            "INSERT INTO paperrust_task_queue \
             (task_id, task_name, queue, priority, args, kwargs, headers, eta, expires, time_limit, link, link_error, group_id, group_index, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) ON CONFLICT (task_id) DO NOTHING",
        )
        .bind(&id_s)
        .bind(task_name)
        .bind(&queue)
        .bind(priority)
        .bind(Value::Array(opts.args.clone()))
        .bind(Value::Object(kwargs))
        .bind(Value::Object(headers))
        .bind(eta)
        .bind(expires)
        .bind(opts.time_limit)
        .bind(serde_json::to_value(&opts.link)?)
        .bind(serde_json::to_value(&opts.link_error)?)
        .bind(member.group_id)
        .bind(member.group_index)
        .bind(now)
        .execute(&mut *conn)
        .await?;
        sqlx::query("SELECT pg_notify($1, $2)").bind(QUEUE_NOTIFY_CHANNEL).bind(&queue).execute(&mut *conn).await?;
        Ok(task_id)
    }

    /// Claim jobs (at most `limit`), oldest/most important first. `task_names`: only these
    /// tasks (those the worker can execute); `None` = all.
    pub async fn claim(&self, worker: &str, queues: &[String], task_names: Option<&[String]>, limit: i64, visibility_secs: f64) -> Result<Vec<Job>, QueueError> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        let now = Utc::now();
        // Raw SQL: SKIP LOCKED (see module comment). Ordering priority (0 first), eta, id.
        let sql = format!(
            "WITH next AS ( \
               SELECT id FROM paperrust_task_queue \
               WHERE state = 'queued' AND eta <= $1 AND queue = ANY($2) AND ($3::text[] IS NULL OR task_name = ANY($3)) \
               ORDER BY priority, eta, id LIMIT $4 FOR UPDATE SKIP LOCKED) \
             UPDATE paperrust_task_queue q SET state = 'running', locked_by = $5, \
               locked_until = $1 + ($6 * interval '1 second'), heartbeat_at = $1, attempts = q.attempts + 1, \
               started_at = $1 \
             FROM next WHERE q.id = next.id RETURNING {JOB_COLUMNS}"
        );
        let rows = sqlx::query(&sql)
            .bind(now)
            .bind(queues)
            .bind(task_names)
            .bind(limit)
            .bind(worker)
            .bind(visibility_secs)
            .fetch_all(&self.pool)
            .await?;
        let mut jobs = rows.iter().map(job_from_row).collect::<Result<Vec<_>, _>>()?;
        jobs.sort_by_key(|j| j.id);
        Ok(jobs)
    }

    /// `task_prerun`: `PaperlessTask` → `started` (only log errors).
    pub async fn mark_started(&self, job: &Job) {
        if let Err(e) = pt::mark_started(&self.pool, &job.task_name, &job.task_id, Utc::now()).await {
            tracing::error!(error = %e, task_id = %job.task_id, "Setting PaperlessTask started failed");
        }
    }

    /// Extend the lease.
    pub async fn heartbeat(&self, job: &Job, visibility_secs: f64) -> Result<Heartbeat, QueueError> {
        let now = Utc::now();
        let row = sqlx::query(
            "UPDATE paperrust_task_queue SET state = 'running', locked_by = $4, locked_until = $1 + ($2 * interval '1 second'), heartbeat_at = $1 \
             WHERE id = $3 AND attempts = $5 AND ((state = 'running' AND locked_by = $4) OR state = 'queued') RETURNING terminate_requested",
        )
        .bind(now)
        .bind(visibility_secs)
        .bind(job.id)
        .bind(&job.locked_by)
        .bind(job.attempts)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            None => Heartbeat::Lost,
            Some(r) if r.try_get::<bool, _>("terminate_requested")? => Heartbeat::Terminate,
            Some(_) => Heartbeat::Ok,
        })
    }

    /// Fencing update: set the state of a running job. `false` = lease lost.
    async fn finish_row(conn: &mut PgConnection, job: &Job, state: &str, result: Option<&Value>, error: Option<&Value>, now: DateTime<Utc>) -> Result<bool, QueueError> {
        let n = sqlx::query(
            "UPDATE paperrust_task_queue SET state = $1, result = $2, error = $3, finished_at = $4, locked_by = NULL, locked_until = NULL \
             WHERE id = $5 AND attempts = $7 AND ((state = 'running' AND locked_by = $6) OR state = 'queued')",
        )
        .bind(state)
        .bind(result)
        .bind(error)
        .bind(now)
        .bind(job.id)
        .bind(&job.locked_by)
        .bind(job.attempts)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        Ok(n == 1)
    }

    /// Success: `task_postrun` (SUCCESS), callbacks (`link`), group/chord accounting.
    pub async fn complete(&self, job: &Job, retval: Value) -> Result<bool, QueueError> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        if !Self::finish_row(&mut tx, job, "done", Some(&retval), None, now).await? {
            tx.rollback().await?;
            return Ok(false);
        }
        pt::mark_success(&mut tx, &job.task_name, &job.task_id, &retval, now).await?;
        for sig in &job.link {
            let (kw, opts) = with_args(sig, vec![retval.clone()]);
            self.enqueue_member(&mut tx, &sig.task, kw, opts, Membership::default(), now).await?;
        }
        if let Some(g) = &job.group_id {
            self.group_part_done(&mut tx, g, job, None, now).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Final failure: `task_failure`, `link_error`, group/chord accounting.
    pub async fn fail(&self, job: &Job, failure: &Failure) -> Result<bool, QueueError> {
        let mut tx = self.pool.begin().await?;
        let ok = self.fail_in(&mut tx, job, failure, Utc::now()).await?;
        if ok {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(ok)
    }

    async fn fail_in(&self, conn: &mut PgConnection, job: &Job, failure: &Failure, now: DateTime<Utc>) -> Result<bool, QueueError> {
        let data = failure.result_data();
        if !Self::finish_row(conn, job, "failed", None, Some(&data), now).await? {
            return Ok(false);
        }
        pt::mark_failure(conn, &job.task_name, &job.task_id, &data, now).await?;
        for sig in &job.link_error {
            let (kw, opts) = with_args(sig, errback_args(job, failure));
            self.enqueue_member(conn, &sig.task, kw, opts, Membership::default(), now).await?;
        }
        if let Some(g) = &job.group_id {
            self.group_part_done(conn, g, job, Some(failure), now).await?;
        }
        Ok(true)
    }

    /// `task.retry(countdown=…)`: back into the queue, `retries + 1`. The `PaperlessTask`
    /// stays unchanged (upstream: `task_postrun` with `RETRY` is skipped → `started`).
    pub async fn retry(&self, job: &Job, countdown_secs: f64, failure: &Failure) -> Result<bool, QueueError> {
        let now = Utc::now();
        let eta = now + Duration::microseconds((countdown_secs.max(0.0) * 1e6) as i64);
        let n = sqlx::query(
            "UPDATE paperrust_task_queue SET state = 'queued', eta = $1, retries = retries + 1, attempts = 0, \
             locked_by = NULL, locked_until = NULL, error = $2 \
             WHERE id = $3 AND attempts = $5 AND ((state = 'running' AND locked_by = $4) OR state = 'queued')",
        )
        .bind(eta)
        .bind(failure.result_data())
        .bind(job.id)
        .bind(&job.locked_by)
        .bind(job.attempts)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n == 1)
    }

    /// Revocation of a claimed job (revoked/expired before start or terminated during
    /// execution): `task_revoked`.
    pub async fn revoke_job(&self, job: &Job, reason: RevokeReason) -> Result<bool, QueueError> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let err = json!({"reason": reason.text()});
        if !Self::finish_row(&mut tx, job, "revoked", None, Some(&err), now).await? {
            tx.rollback().await?;
            return Ok(false);
        }
        pt::mark_revoked(&mut *tx, &job.task_name, &job.task_id, reason, now).await?;
        if let Some(g) = &job.group_id {
            let f = Failure::new("TaskRevokedError", reason.text());
            self.group_part_done(&mut tx, g, job, Some(&f), now).await.map(|_| ())?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// On shutdown: release the running job (immediately available again, does not count as a
    /// delivery).
    pub async fn release(&self, job: &Job) -> Result<bool, QueueError> {
        let n = sqlx::query(
            "UPDATE paperrust_task_queue SET state = 'queued', eta = $1, attempts = GREATEST(attempts - 1, 0), \
             locked_by = NULL, locked_until = NULL WHERE id = $2 AND attempts = $4 AND ((state = 'running' AND locked_by = $3) OR state = 'queued')",
        )
        .bind(Utc::now())
        .bind(job.id)
        .bind(&job.locked_by)
        .bind(job.attempts)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n == 1)
    }

    /// Expired leases (crashed workers): re-enqueue or – after `max_deliveries`
    /// deliveries – fail as `WorkerLostError`.
    pub async fn reap_expired_leases(&self) -> Result<ReapReport, QueueError> {
        let now = Utc::now();
        let mut report = ReapReport::default();
        let mut tx = self.pool.begin().await?;
        let requeued = sqlx::query(
            "WITH lost AS (SELECT id FROM paperrust_task_queue WHERE state = 'running' AND locked_until < $1 AND attempts < $2 \
               FOR UPDATE SKIP LOCKED) \
             UPDATE paperrust_task_queue q SET state = 'queued', eta = $1, locked_by = NULL, locked_until = NULL \
             FROM lost WHERE q.id = lost.id RETURNING q.task_id",
        )
        .bind(now)
        .bind(self.max_deliveries)
        .fetch_all(&mut *tx)
        .await?;
        for r in requeued {
            report.requeued.push(r.try_get("task_id")?);
        }
        let sql = format!(
            "SELECT {JOB_COLUMNS} FROM paperrust_task_queue q WHERE q.state = 'running' AND q.locked_until < $1 AND q.attempts >= $2 \
             FOR UPDATE SKIP LOCKED"
        );
        let dead = sqlx::query(&sql).bind(now).bind(self.max_deliveries).fetch_all(&mut *tx).await?;
        for r in &dead {
            let job = job_from_row(r)?;
            let f = Failure::new(
                "WorkerLostError",
                format!("Worker exited prematurely: lease expired after {} deliveries. Job: {}.", job.attempts, job.task_id),
            );
            if self.fail_in(&mut tx, &job, &f, now).await? {
                report.failed.push(job.task_id.clone());
            }
        }
        tx.commit().await?;
        for id in &report.requeued {
            tracing::warn!(task_id = %id, "worker lost, task requeued");
        }
        for id in &report.failed {
            tracing::error!(task_id = %id, "worker lost too often, task failed");
        }
        Ok(report)
    }

    /// Delete finished rows older than `older_than_secs` (`CELERY_RESULT_EXPIRES`).
    pub async fn purge_finished(&self, older_than_secs: f64) -> Result<u64, QueueError> {
        let cutoff = Utc::now() - Duration::microseconds((older_than_secs * 1e6) as i64);
        let n = sqlx::query("DELETE FROM paperrust_task_queue WHERE state IN ('done', 'failed', 'revoked') AND finished_at < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected();
        sqlx::query("DELETE FROM paperrust_task_group WHERE finished_at < $1").bind(cutoff).execute(&self.pool).await?;
        Ok(n)
    }

    /// Group/chord part finished (`failure = None` → success). Under the group's row lock.
    async fn group_part_done(&self, conn: &mut PgConnection, group_id: &str, job: &Job, failure: Option<&Failure>, now: DateTime<Utc>) -> Result<(), QueueError> {
        let failed_inc = i32::from(failure.is_some());
        let Some(row) = sqlx::query(
            "UPDATE paperrust_task_group SET completed = completed + $1, failed = failed + $2 \
             WHERE group_id = $3 AND finished_at IS NULL RETURNING total, completed, failed, body, on_error",
        )
        .bind(1 - failed_inc)
        .bind(failed_inc)
        .bind(group_id)
        .fetch_optional(&mut *conn)
        .await?
        else {
            return Ok(());
        };
        let total: i32 = row.try_get("total")?;
        let completed: i32 = row.try_get("completed")?;
        let failed: i32 = row.try_get("failed")?;
        let body: Option<Signature> = row.try_get::<Option<Value>, _>("body")?.and_then(|v| serde_json::from_value(v).ok());
        let on_error: Option<Signature> = row.try_get::<Option<Value>, _>("on_error")?.and_then(|v| serde_json::from_value(v).ok());
        // CELERY_TASK_ALLOW_ERROR_CB_ON_CHORD_HEADER: errback per failed header task
        // (not on revocation – Celery calls errbacks only on on_failure).
        if let (Some(f), Some(eb)) = (failure, &on_error)
            && f.error_type != "TaskRevokedError"
        {
            let (kw, opts) = with_args(eb, errback_args(job, f));
            self.enqueue_member(conn, &eb.task, kw, opts, Membership::default(), now).await?;
        }
        if completed + failed < total {
            return Ok(());
        }
        sqlx::query("UPDATE paperrust_task_group SET finished_at = $1 WHERE group_id = $2").bind(now).bind(group_id).execute(&mut *conn).await?;
        let Some(body) = body else { return Ok(()) };
        if failed == 0 {
            let results: Vec<Value> = sqlx::query("SELECT result FROM paperrust_task_queue WHERE group_id = $1 ORDER BY group_index")
                .bind(group_id)
                .fetch_all(&mut *conn)
                .await?
                .iter()
                .map(|r| r.try_get::<Option<Value>, _>("result").map(|v| v.unwrap_or(Value::Null)))
                .collect::<Result<_, _>>()?;
            let (kw, opts) = with_args(&body, vec![Value::Array(results)]);
            self.enqueue_member(conn, &body.task, kw, opts, Membership::default(), now).await?;
        } else if let Some(eb) = &on_error {
            // ChordError → errbacks of the body once per chord.
            let f = Failure::new("ChordError", format!("{failed} of {total} header tasks failed"));
            let pseudo = Job { task_id: group_id.to_string(), task_name: body.task.clone(), args: body.args.clone(), kwargs: body.kwargs.clone(), group_id: None, ..job.clone() };
            let (kw, opts) = with_args(eb, errback_args(&pseudo, &f));
            self.enqueue_member(conn, &eb.task, kw, opts, Membership::default(), now).await?;
        }
        Ok(())
    }

    async fn create_group(&self, header: Vec<Signature>, body: Option<Signature>, on_error: Option<Signature>) -> Result<Uuid, QueueError> {
        let now = Utc::now();
        let group_id = Uuid::new_v4();
        let gid = group_id.to_string();
        let mut tx = self.pool.begin().await?;
        if header.is_empty() {
            // celery: an empty chord calls the body immediately with []
            if let Some(body) = body {
                let (kw, opts) = with_args(&body, vec![Value::Array(Vec::new())]);
                self.enqueue_member(&mut tx, &body.task, kw, opts, Membership::default(), now).await?;
            }
            tx.commit().await?;
            return Ok(group_id);
        }
        sqlx::query("INSERT INTO paperrust_task_group (group_id, total, body, on_error, created_at) VALUES ($1, $2, $3, $4, $5)")
            .bind(&gid)
            .bind(i32::try_from(header.len()).map_err(|_| QueueError::Invalid("group too large".into()))?)
            .bind(body.as_ref().map(serde_json::to_value).transpose()?)
            .bind(on_error.as_ref().map(serde_json::to_value).transpose()?)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        for (i, sig) in header.iter().enumerate() {
            let (kw, opts) = with_args(sig, Vec::new());
            let member = Membership { group_id: Some(gid.clone()), group_index: Some(i as i32) };
            self.enqueue_member(&mut tx, &sig.task, kw, opts, member, now).await?;
        }
        tx.commit().await?;
        Ok(group_id)
    }

    async fn status_impl(&self, task_id: Uuid) -> Result<Option<TaskInfo>, QueueError> {
        let row = sqlx::query("SELECT task_name, state, retries, result, error, finished_at FROM paperrust_task_queue WHERE task_id = $1")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(r) = row else {
            // after cleanup: state from documents_paperlesstask (tracked tasks only)
            let row = sqlx::query("SELECT status, result_data, date_done FROM documents_paperlesstask WHERE task_id = $1")
                .bind(task_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
            let Some(r) = row else { return Ok(None) };
            let status: String = r.try_get("status")?;
            let Some(status) = TaskStatus::from_paperless(&status) else { return Ok(None) };
            let data: Option<Value> = r.try_get("result_data")?;
            let failed = matches!(status, TaskStatus::Failure | TaskStatus::Revoked);
            return Ok(Some(TaskInfo {
                task_id,
                task_name: None,
                status,
                result: if failed { None } else { data.clone() },
                error: if failed { data } else { None },
                retries: 0,
                date_done: r.try_get("date_done")?,
            }));
        };
        let state: String = r.try_get("state")?;
        let retries: i32 = r.try_get("retries")?;
        let status = match state.as_str() {
            "queued" if retries > 0 => TaskStatus::Retry,
            "queued" => TaskStatus::Pending,
            "running" => TaskStatus::Started,
            "done" => TaskStatus::Success,
            "failed" => TaskStatus::Failure,
            _ => TaskStatus::Revoked,
        };
        Ok(Some(TaskInfo {
            task_id,
            task_name: Some(r.try_get("task_name")?),
            status,
            result: r.try_get("result")?,
            error: if status == TaskStatus::Success { None } else { r.try_get("error")? },
            retries,
            date_done: r.try_get("finished_at")?,
        }))
    }

    async fn revoke_impl(&self, task_id: Uuid, terminate: bool) -> Result<bool, QueueError> {
        let now = Utc::now();
        let id = task_id.to_string();
        let mut tx = self.pool.begin().await?;
        let sql = format!("SELECT {JOB_COLUMNS}, q.state FROM paperrust_task_queue q WHERE q.task_id = $1 FOR UPDATE");
        let Some(row) = sqlx::query(&sql).bind(&id).fetch_optional(&mut *tx).await? else {
            tx.rollback().await?;
            return Ok(false);
        };
        let state: String = row.try_get("state")?;
        let job = job_from_row(&row)?;
        let affected = match state.as_str() {
            "queued" => {
                // before start: REVOKED immediately (Celery: discarded on receipt)
                let err = json!({"reason": RevokeReason::Revoked.text()});
                sqlx::query("UPDATE paperrust_task_queue SET state = 'revoked', error = $1, finished_at = $2, revoke_requested = true WHERE id = $3")
                    .bind(&err)
                    .bind(now)
                    .bind(job.id)
                    .execute(&mut *tx)
                    .await?;
                pt::mark_revoked(&mut *tx, &job.task_name, &job.task_id, RevokeReason::Revoked, now).await?;
                if let Some(g) = &job.group_id {
                    let f = Failure::new("TaskRevokedError", RevokeReason::Revoked.text());
                    self.group_part_done(&mut tx, g, &job, Some(&f), now).await?;
                }
                true
            }
            "running" => {
                sqlx::query("UPDATE paperrust_task_queue SET revoke_requested = true, terminate_requested = terminate_requested OR $1 WHERE id = $2")
                    .bind(terminate)
                    .bind(job.id)
                    .execute(&mut *tx)
                    .await?;
                true
            }
            _ => false,
        };
        tx.commit().await?;
        Ok(affected)
    }
}

impl TaskQueue for PgQueue {
    fn enqueue<'a>(&'a self, task_name: &'a str, kwargs: Map<String, Value>, opts: EnqueueOptions) -> BoxFuture<'a, Result<Uuid, QueueError>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let id = self.enqueue_in(&mut tx, task_name, kwargs, opts).await?;
            tx.commit().await?;
            Ok(id)
        })
    }

    fn status(&self, task_id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>> {
        Box::pin(self.status_impl(task_id))
    }

    fn revoke(&self, task_id: Uuid, terminate: bool) -> BoxFuture<'_, Result<bool, QueueError>> {
        Box::pin(self.revoke_impl(task_id, terminate))
    }

    fn enqueue_group(&self, header: Vec<Signature>) -> BoxFuture<'_, Result<Uuid, QueueError>> {
        Box::pin(self.create_group(header, None, None))
    }

    fn enqueue_chord(&self, header: Vec<Signature>, body: Signature, on_error: Option<Signature>) -> BoxFuture<'_, Result<Uuid, QueueError>> {
        Box::pin(self.create_group(header, Some(body), on_error))
    }
}
