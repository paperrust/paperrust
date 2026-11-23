//! Worker loop for [`PgQueue`]: bounded parallelism, heartbeat/lease, time limit,
//! retries, revocation, clean shutdown.
//!
//! upstream: celery/worker/request.py@5.6.3 (`Request.revoked`, `on_timeout`, `on_failure`,
//! `on_retry`), celery/app/trace.py (signals `task_prerun`/`task_postrun`/`task_failure`),
//! celery/app/autoretry.py, src/paperless/settings/__init__.py (`CELERY_WORKER_CONCURRENCY`,
//! `CELERY_TASK_TIME_LIMIT`, `CELERY_TASK_TRACK_STARTED`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::Utc;
use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::backoff::Backoff;
use crate::config::QueueConfig;
use crate::events::EventBus;
use crate::paperless_task::RevokeReason;
use crate::pg::{Failure, Heartbeat, Job, PgQueue};
use crate::registry::{RetryPolicy, task_def};

/// Context of a running task (`self.request` + helpers).
#[derive(Clone)]
pub struct TaskContext {
    pub task_id: String,
    pub task_name: String,
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
    pub headers: Map<String, Value>,
    /// `request.retries`
    pub retries: i32,
    /// Status messages (`ProgressManager`).
    pub events: EventBus,
    /// `PAPERLESS_THREADS_PER_WORKER`.
    pub threads_per_worker: usize,
}

impl TaskContext {
    pub fn uuid(&self) -> Option<Uuid> {
        Uuid::parse_str(&self.task_id).ok()
    }
}

/// How the failure is handled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetryMode {
    /// Fail permanently (`raise`).
    No,
    /// Failure falls under `autoretry_for` → the task's rule ([`crate::registry::TaskDef::autoretry`]).
    Auto,
    /// `self.retry(countdown=…)` (Standard `max_retries=3`, `default_retry_delay=180`).
    Explicit { countdown: Option<f64> },
}

/// Failure of a task (corresponds to the exception).
#[derive(Debug, Clone, PartialEq)]
pub struct TaskError {
    /// `type(exc).__name__`
    pub error_type: String,
    /// `str(exc)`
    pub message: String,
    pub traceback: Option<String>,
    pub retry: RetryMode,
}

impl TaskError {
    pub fn fail(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self { error_type: error_type.into(), message: message.into(), traceback: None, retry: RetryMode::No }
    }
    /// Failure from `autoretry_for` (e.g. `SearchIndexLockError`, `LLMTimeoutError`).
    pub fn autoretry(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self { retry: RetryMode::Auto, ..Self::fail(error_type, message) }
    }
    /// `self.retry(exc=…, countdown=…)`.
    pub fn retry(error_type: impl Into<String>, message: impl Into<String>, countdown: Option<f64>) -> Self {
        Self { retry: RetryMode::Explicit { countdown }, ..Self::fail(error_type, message) }
    }
    fn failure(&self) -> Failure {
        Failure { error_type: self.error_type.clone(), message: self.message.clone(), traceback: self.traceback.clone() }
    }
}

/// Executable task. Return value = Celery `retval` (JSON; only objects end up in
/// `PaperlessTask.result_data`).
pub trait TaskHandler: Send + Sync {
    fn call(&self, ctx: TaskContext) -> BoxFuture<'static, Result<Value, TaskError>>;
}

impl<F, Fut> TaskHandler for F
where
    F: Fn(TaskContext) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, TaskError>> + Send + 'static,
{
    fn call(&self, ctx: TaskContext) -> BoxFuture<'static, Result<Value, TaskError>> {
        Box::pin(self(ctx))
    }
}

struct Registered {
    handler: Arc<dyn TaskHandler>,
    autoretry: Option<RetryPolicy>,
}

/// Registered tasks of this worker. A worker only claims tasks it knows.
#[derive(Default, Clone)]
pub struct Handlers {
    map: HashMap<String, Arc<Registered>>,
}

impl Handlers {
    pub fn new() -> Self {
        Self::default()
    }
    /// Register a task with the upstream retry rule.
    pub fn register(mut self, name: &str, handler: impl TaskHandler + 'static) -> Self {
        let autoretry = task_def(name).and_then(|d| d.autoretry);
        self.map.insert(name.to_string(), Arc::new(Registered { handler: Arc::new(handler), autoretry }));
        self
    }
    /// With a custom rule (tests/new tasks).
    pub fn register_with_policy(mut self, name: &str, handler: impl TaskHandler + 'static, autoretry: Option<RetryPolicy>) -> Self {
        self.map.insert(name.to_string(), Arc::new(Registered { handler: Arc::new(handler), autoretry }));
        self
    }
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.map.keys().cloned().collect();
        v.sort();
        v
    }
}

/// `Duration` from seconds without panicking on NaN/negative/huge values (falls back to `min`).
fn secs_or(v: f64, min: f64) -> StdDuration {
    StdDuration::try_from_secs_f64(v.max(min)).unwrap_or(StdDuration::from_secs_f64(min))
}

/// Lease timing of one running job (pure; `Instant`s are passed in so it is unit-testable).
///
/// The lease lasts `visibility` from the moment a claim/heartbeat/`mark_started` was **sent** (the
/// SQL writes `locked_until = <client now> + visibility`). The runner may only execute while it can
/// prove its lease with a margin: until `fence` = 80 % of the visibility after the send time of the
/// last successful lease write (`last_ok`). The remaining 20 % cover clock skew between worker hosts
/// (contract B3.8). TLA+ finding F1: this deadline is enforced by its own timer, independent of
/// heartbeat attempts (before, it was only checked after an attempt, i.e. every visibility/3, so a
/// heartbeat failing at once let the runner live until 100 %).
#[derive(Debug, Clone, Copy, PartialEq)]
struct LeaseTiming {
    /// Self-fence: 0.8 × visibility after `last_ok`.
    fence: StdDuration,
    /// Heartbeat period after a successful lease write: visibility/3.
    every: StdDuration,
    /// Next attempt after a failed/timed-out heartbeat: visibility/15 (several retries fit into the
    /// fence budget; the fence timer still stops the runner when none succeeds).
    retry: StdDuration,
}

impl LeaseTiming {
    fn new(visibility_secs: f64) -> Self {
        Self {
            fence: secs_or(visibility_secs * 0.8, 0.08),
            every: secs_or(visibility_secs / 3.0, 0.1),
            retry: secs_or(visibility_secs / 15.0, 0.05),
        }
    }
    /// When the runner must stop at the latest.
    fn fence_at(&self, last_ok: Instant) -> Instant {
        last_ok + self.fence
    }
    /// The lease is provably valid (with margin) at `now`.
    fn valid(&self, last_ok: Instant, now: Instant) -> bool {
        now < self.fence_at(last_ok)
    }
    /// Time left for a lease write started at `now` (its `timeout`).
    fn budget(&self, last_ok: Instant, now: Instant) -> StdDuration {
        self.fence_at(last_ok).saturating_duration_since(now)
    }
    /// Next heartbeat after a successful lease write sent at `sent`.
    fn next_after_ok(&self, sent: Instant) -> Instant {
        sent + self.every
    }
    /// Next heartbeat after a failed attempt that returned at `now`.
    fn next_after_fail(&self, now: Instant) -> Instant {
        now + self.retry
    }
}

/// Python `repr` of a number for `TimeLimitExceeded(<n>,)`.
fn py_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { pl_core::json::python_float_repr(v) }
}

/// Worker.
pub struct Worker {
    queue: PgQueue,
    handlers: Arc<Handlers>,
    cfg: QueueConfig,
    events: EventBus,
}

enum Outcome {
    Done(Result<Value, TaskError>),
    Panicked(String),
    TimeLimit(f64),
    Terminated,
    Lost,
    Shutdown,
}

impl Worker {
    pub fn new(queue: PgQueue, handlers: Handlers, cfg: QueueConfig, events: EventBus) -> Self {
        Self { queue, handlers: Arc::new(handlers), cfg, events }
    }

    /// Runs until `shutdown` is set to `true`. After that: no new jobs, running ones get
    /// `shutdown_grace_secs`; whatever still runs then is aborted and released (immediately
    /// available again).
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let sem = Arc::new(Semaphore::new(self.cfg.concurrency.max(1)));
        let (abort_tx, abort_rx) = watch::channel(false);
        let mut running: JoinSet<()> = JoinSet::new();
        let names = self.handlers.names();
        let mut listen_backoff = Backoff::default();
        let mut listen_retry_at: Option<tokio::time::Instant> = None;
        let mut listener = match connect_listener(self.queue.pool()).await {
            Ok(l) => Some(l),
            Err(e) => {
                let step = listen_backoff.fail();
                tracing::warn!(error = %e, retry_in_ms = step.delay.as_millis() as u64, "LISTEN failed, polling meanwhile");
                listen_retry_at = Some(tokio::time::Instant::now() + step.delay);
                None
            }
        };
        let poll = StdDuration::from_secs_f64(self.cfg.poll_interval_secs);
        let reap_every = StdDuration::from_secs_f64((self.cfg.visibility_timeout_secs / 2.0).max(0.5));
        let mut last_reap = tokio::time::Instant::now() - reap_every;
        let mut last_purge = tokio::time::Instant::now();
        tracing::info!(worker = %self.cfg.worker_name, concurrency = self.cfg.concurrency, "worker started");
        // E2E 2026-11-08: with a missing table the loop ran ~1000×/s without a pause.
        let mut db_backoff = Backoff::default();
        loop {
            if *shutdown.borrow() {
                break;
            }
            while running.try_join_next().is_some() {}
            let mut first_err: Option<(&'static str, crate::QueueError)> = None;
            if last_reap.elapsed() >= reap_every {
                last_reap = tokio::time::Instant::now();
                if let Err(e) = self.queue.reap_expired_leases().await {
                    first_err.get_or_insert(("reaping expired leases failed", e));
                }
            }
            if last_purge.elapsed() >= StdDuration::from_secs(60) {
                last_purge = tokio::time::Instant::now();
                if let Err(e) = self.queue.purge_finished(self.cfg.result_expires_secs).await {
                    first_err.get_or_insert(("purging finished tasks failed", e));
                }
            }
            let free = sem.available_permits();
            let jobs = if free == 0 || first_err.is_some() {
                Vec::new()
            } else {
                match self
                    .queue
                    .claim(&self.cfg.worker_name, &self.cfg.queues, Some(&names), free as i64, self.cfg.visibility_timeout_secs)
                    .await
                {
                    Ok(j) => j,
                    Err(e) => {
                        first_err = Some(("claiming tasks failed", e));
                        Vec::new()
                    }
                }
            };
            if let Some((what, e)) = first_err {
                let step = db_backoff.fail();
                tracing::error!(error = %e, attempt = step.attempt, retry_in_ms = step.delay.as_millis() as u64, "{what}");
                if crate::backoff::is_undefined_table(&e) {
                    // Table missing (DB recreated?): the migration is idempotent → apply it again.
                    match crate::migrations::apply(self.queue.pool()).await {
                        Ok(()) => tracing::warn!("task queue tables were missing; task queue migrations applied again"),
                        Err(me) => tracing::error!(error = %me, "re-applying the task queue migration failed"),
                    }
                }
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = tokio::time::sleep(step.delay) => {}
                }
                // Catch up aborted reap/purge runs on the next pass.
                last_reap = tokio::time::Instant::now() - reap_every;
                continue;
            }
            let recovered = db_backoff.succeed();
            if recovered > 0 {
                tracing::info!(failures = recovered, "task queue database access works again");
            }
            if jobs.is_empty() {
                // wait: job finished, NOTIFY, polling interval or shutdown
                let listen_due = listen_retry_at.is_none_or(|t| tokio::time::Instant::now() >= t);
                if listener.is_none() && listen_due {
                    // Rebuild after connection loss: sqlx' PgListener does not reconnect on its own after a
                    // backend terminated by the server (DROP DATABASE … WITH (FORCE), 57P01);
                    // recv() would otherwise fail endlessly.
                    match connect_listener(self.queue.pool()).await {
                        Ok(l) => {
                            // Reset the backoff only on the first received message: if
                            // the new connection drops again immediately, we do not reconnect every 0.5 s.
                            listener = Some(l);
                            listen_retry_at = None;
                            tracing::info!("LISTEN re-established");
                        }
                        Err(e) => {
                            let step = listen_backoff.fail();
                            tracing::warn!(error = %e, attempt = step.attempt, retry_in_ms = step.delay.as_millis() as u64, "LISTEN failed, polling meanwhile");
                            listen_retry_at = Some(tokio::time::Instant::now() + step.delay);
                        }
                    }
                }
                tokio::select! {
                    _ = shutdown.changed() => {}
                    Some(_) = running.join_next(), if !running.is_empty() => {}
                    r = async {
                        match listener.as_mut() {
                            Some(l) => l.recv().await.map(|_| ()),
                            None => std::future::pending().await,
                        }
                    }, if free > 0 => match r {
                        Ok(()) => {
                            listen_backoff.succeed();
                        }
                        Err(e) => {
                            // Discard the listener; rebuild after backoff (polling only until then).
                            let step = listen_backoff.fail();
                            tracing::warn!(error = %e, attempt = step.attempt, retry_in_ms = step.delay.as_millis() as u64, "LISTEN connection lost, polling meanwhile");
                            listener = None;
                            listen_retry_at = Some(tokio::time::Instant::now() + step.delay);
                        }
                    },
                    _ = tokio::time::sleep(poll) => {}
                }
                continue;
            }
            for job in jobs {
                let Ok(permit) = sem.clone().acquire_owned().await else { break };
                let ctx = JobRunner {
                    queue: self.queue.clone(),
                    handlers: self.handlers.clone(),
                    cfg: self.cfg.clone(),
                    events: self.events.clone(),
                    abort: abort_rx.clone(),
                };
                running.spawn(async move {
                    ctx.run(job).await;
                    drop(permit);
                });
            }
        }
        tracing::info!(running = running.len(), "worker shutting down");
        let grace = tokio::time::sleep(StdDuration::from_secs_f64(self.cfg.shutdown_grace_secs));
        tokio::pin!(grace);
        loop {
            tokio::select! {
                r = running.join_next() => { if r.is_none() { break; } }
                _ = &mut grace => {
                    let _ = abort_tx.send(true);
                    while running.join_next().await.is_some() {}
                    break;
                }
            }
        }
        tracing::info!("worker stopped");
    }
}

/// `LISTEN paperrust_queue` on its own connection.
async fn connect_listener(pool: &sqlx::PgPool) -> Result<sqlx::postgres::PgListener, sqlx::Error> {
    // FINDINGS B2: own connection, not from the (control) pool
    let mut l = pl_core::dbconn::listener(pool).await?;
    l.listen(crate::pg::QUEUE_NOTIFY_CHANNEL).await?;
    Ok(l)
}

#[derive(Clone)]
struct JobRunner {
    queue: PgQueue,
    handlers: Arc<Handlers>,
    cfg: QueueConfig,
    events: EventBus,
    abort: watch::Receiver<bool>,
}

impl JobRunner {
    async fn run(self, job: Job) {
        if let Err(e) = self.run_inner(&job).await {
            tracing::error!(error = %e, task_id = %job.task_id, task = %job.task_name, "task bookkeeping failed");
        }
    }

    async fn run_inner(&self, job: &Job) -> Result<(), crate::QueueError> {
        // Request.revoked(): revoked or expired → REVOKED without execution
        if job.revoke_requested {
            self.queue.revoke_job(job, RevokeReason::Revoked).await?;
            return Ok(());
        }
        if job.expires.is_some_and(|e| e <= Utc::now()) {
            tracing::info!(task_id = %job.task_id, "Discarding revoked task: {}[{}]", job.task_name, job.task_id);
            self.queue.revoke_job(job, RevokeReason::Expired).await?;
            return Ok(());
        }
        let Some(reg) = self.handlers.map.get(&job.task_name).cloned() else {
            // claim() filters on known names; if it happens anyway: release
            self.queue.release(job).await?;
            return Ok(());
        };
        // Lease timing (F1/F2): `last_ok` = send time of the last successful lease write, starting
        // with the claim's.
        let timing = LeaseTiming::new(self.cfg.visibility_timeout_secs);
        let vis = self.cfg.visibility_timeout_secs;
        let mut last_ok = job.claimed_at;
        let sent = Instant::now();
        if !timing.valid(last_ok, sent) {
            // Paused between claim and start (pool wait, suspended VM, ...): another worker may own
            // the task already. Do not start; the cleaner hands it out again (at-least-once).
            tracing::warn!(task_id = %job.task_id, "lease not provable before start; task not executed");
            return Ok(());
        }
        // F2: `mark_started` is fenced and extends the lease; the handler only starts if this
        // delivery still owns the row.
        let mut next_hb = match tokio::time::timeout(timing.budget(last_ok, sent), self.queue.mark_started(job, vis)).await {
            Ok(Ok(Heartbeat::Ok)) => {
                last_ok = sent;
                timing.next_after_ok(sent)
            }
            Ok(Ok(Heartbeat::Terminate)) => {
                // revoke(terminate=True) arrived between claim and start: do not run.
                self.queue.revoke_job(job, RevokeReason::Terminated).await?;
                return Ok(());
            }
            Ok(Ok(Heartbeat::Lost)) => {
                tracing::warn!(task_id = %job.task_id, "lease lost before start (another worker owns the task now); task not executed");
                return Ok(());
            }
            Ok(Err(e)) => {
                // Nothing written (or a fenced write that held the lease); the claim's lease still
                // counts, the fence timer below stops the run if no heartbeat succeeds.
                tracing::error!(error = %e, task_id = %job.task_id, "Setting PaperlessTask started failed");
                timing.next_after_fail(Instant::now())
            }
            Err(_) => {
                tracing::warn!(task_id = %job.task_id, "lease not provable before start (mark_started timed out); task not executed");
                return Ok(());
            }
        };
        if !timing.valid(last_ok, Instant::now()) {
            tracing::warn!(task_id = %job.task_id, "lease not provable before start; task not executed");
            return Ok(());
        }
        let ctx = TaskContext {
            task_id: job.task_id.clone(),
            task_name: job.task_name.clone(),
            args: job.args.clone(),
            kwargs: job.kwargs.clone(),
            headers: job.headers.clone(),
            retries: job.retries,
            events: self.events.clone(),
            threads_per_worker: self.cfg.threads_per_worker,
        };
        let mut handle = tokio::spawn(reg.handler.call(ctx));
        let limit = job.time_limit.filter(|t| t.is_finite() && *t > 0.0).unwrap_or(self.cfg.time_limit_secs);
        let deadline = tokio::time::sleep(secs_or(limit, 0.0));
        tokio::pin!(deadline);
        let mut abort = self.abort.clone();
        let outcome = loop {
            // Self-fencing (F1): own timer at `last_ok + 80 %`, armed anew whenever `last_ok`
            // moves; it fires even while heartbeats keep failing at once.
            let fence_at = tokio::time::Instant::from_std(timing.fence_at(last_ok));
            let hb_at = tokio::time::Instant::from_std(next_hb);
            tokio::select! {
                r = &mut handle => {
                    break match r {
                        Ok(res) => Outcome::Done(res),
                        Err(e) if e.is_panic() => {
                            let p = e.into_panic();
                            let msg = p.downcast_ref::<&str>().map(|s| s.to_string())
                                .or_else(|| p.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "panic".into());
                            Outcome::Panicked(msg)
                        }
                        Err(_) => Outcome::Shutdown,
                    };
                }
                _ = &mut deadline => { handle.abort(); break Outcome::TimeLimit(limit); }
                _ = tokio::time::sleep_until(fence_at) => { handle.abort(); break Outcome::Lost; }
                _ = tokio::time::sleep_until(hb_at) => {
                    let sent = Instant::now();
                    // A hanging heartbeat ends exactly at the fence deadline.
                    match tokio::time::timeout(timing.budget(last_ok, sent), self.queue.heartbeat(job, vis)).await {
                        Ok(Ok(Heartbeat::Ok)) => {
                            last_ok = sent;
                            next_hb = timing.next_after_ok(sent);
                        }
                        Ok(Ok(Heartbeat::Terminate)) => { handle.abort(); break Outcome::Terminated; }
                        Ok(Ok(Heartbeat::Lost)) => { handle.abort(); break Outcome::Lost; }
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, task_id = %job.task_id, "heartbeat failed");
                            next_hb = timing.next_after_fail(Instant::now());
                        }
                        Err(_) => {
                            tracing::warn!(task_id = %job.task_id, "heartbeat timed out");
                            next_hb = timing.next_after_fail(Instant::now());
                        }
                    }
                    if !timing.valid(last_ok, Instant::now()) {
                        handle.abort();
                        break Outcome::Lost;
                    }
                }
                _ = abort.changed() => {
                    if *abort.borrow() { handle.abort(); break Outcome::Shutdown; }
                }
            }
        };
        match outcome {
            Outcome::Done(Ok(v)) => {
                if !self.queue.complete(job, v).await? {
                    tracing::warn!(task_id = %job.task_id, "lease lost before completion; result discarded");
                }
            }
            Outcome::Done(Err(e)) => self.handle_error(job, &reg, e).await?,
            Outcome::Panicked(msg) => {
                self.queue.fail(job, &Failure::new("Panic", msg)).await?;
            }
            Outcome::TimeLimit(t) => {
                tracing::error!(task_id = %job.task_id, "Hard time limit ({t}s) exceeded for {}[{}]", job.task_name, job.task_id);
                self.queue.fail(job, &Failure::new("TimeLimitExceeded", format!("TimeLimitExceeded({},)", py_num(t)))).await?;
            }
            Outcome::Terminated => {
                self.queue.revoke_job(job, RevokeReason::Terminated).await?;
            }
            Outcome::Lost => {
                tracing::warn!(task_id = %job.task_id, "lease lost (another worker owns the task now); execution aborted");
            }
            Outcome::Shutdown => {
                self.queue.release(job).await?;
            }
        }
        Ok(())
    }

    async fn handle_error(&self, job: &Job, reg: &Registered, e: TaskError) -> Result<(), crate::QueueError> {
        let policy = match e.retry {
            RetryMode::No => None,
            RetryMode::Auto => reg.autoretry,
            RetryMode::Explicit { .. } => Some(reg.autoretry.unwrap_or(RetryPolicy::DEFAULT)),
        };
        let retries = u32::try_from(job.retries).unwrap_or(0);
        if let Some(p) = policy.filter(|p| !p.exhausted(retries)) {
            let countdown = match e.retry {
                RetryMode::Explicit { countdown: Some(c) } => c,
                _ => p.countdown(retries, &mut rand::thread_rng()) as f64,
            };
            tracing::info!(task_id = %job.task_id, "Task {}[{}] retry: Retry in {countdown}s: {}", job.task_name, job.task_id, e.message);
            self.queue.retry(job, countdown, &e.failure()).await?;
            return Ok(());
        }
        self.queue.fail(job, &e.failure()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing60() -> LeaseTiming {
        LeaseTiming::new(60.0)
    }

    #[test]
    fn lease_timing_defaults() {
        let t = timing60();
        assert_eq!(t.fence, StdDuration::from_secs(48));
        assert_eq!(t.every, StdDuration::from_secs(20));
        assert_eq!(t.retry, StdDuration::from_secs(4));
        // Degenerate values never panic and stay positive.
        for v in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e300] {
            let t = LeaseTiming::new(v);
            assert!(t.fence > StdDuration::ZERO && t.every > StdDuration::ZERO && t.retry > StdDuration::ZERO, "{v}");
        }
    }

    #[test]
    fn fence_deadline_is_80_percent_after_last_success() {
        let t = timing60();
        let claim = Instant::now();
        assert!(t.valid(claim, claim + StdDuration::from_secs(47)));
        assert!(!t.valid(claim, claim + StdDuration::from_secs(48)));
        assert_eq!(t.budget(claim, claim + StdDuration::from_secs(40)), StdDuration::from_secs(8));
        assert_eq!(t.budget(claim, claim + StdDuration::from_secs(50)), StdDuration::ZERO);
        // A success moves the deadline to its send time + 80 %.
        let hb = claim + StdDuration::from_secs(20);
        assert_eq!(t.fence_at(hb), claim + StdDuration::from_secs(68));
        assert_eq!(t.next_after_ok(hb), claim + StdDuration::from_secs(40));
    }

    /// F1 regression: heartbeats that fail at once (t = 20 s, 40 s) must not let the runner live
    /// past 80 % of the lease; the old code checked only after attempts (20/40/60 s) and stopped at
    /// 60 s = 100 %. Simulates the select loop: the runner stops at the first event at which the
    /// fence timer is due.
    #[test]
    fn fast_failing_heartbeats_stop_at_the_fence_not_at_lease_end() {
        let t = timing60();
        let claim = Instant::now();
        let last_ok = claim;
        let mut next_hb = t.next_after_ok(claim);
        let mut attempts = Vec::new();
        let stop = loop {
            let fence_at = t.fence_at(last_ok);
            if fence_at <= next_hb {
                break fence_at;
            }
            let now = next_hb; // attempt fails at once
            attempts.push(now - claim);
            next_hb = t.next_after_fail(now);
        };
        assert_eq!(stop - claim, StdDuration::from_secs(48));
        // Retries every 4 s after the first failure: 20, 24, ..., 44 s.
        assert_eq!(attempts.first(), Some(&StdDuration::from_secs(20)));
        assert_eq!(attempts.last(), Some(&StdDuration::from_secs(44)));
        assert_eq!(attempts.len(), 7);
        // Another host may reap only after 60 s minus its clock skew; with skew < 20 % (12 s) that
        // is strictly after 48 s.
        assert!(stop - claim < StdDuration::from_secs(60 - 12) + StdDuration::from_millis(1));
    }

    #[test]
    fn time_limit_text() {
        assert_eq!(format!("TimeLimitExceeded({},)", py_num(1800.0)), "TimeLimitExceeded(1800,)");
        assert_eq!(format!("TimeLimitExceeded({},)", py_num(0.5)), "TimeLimitExceeded(0.5,)");
    }

    #[test]
    fn handlers_use_upstream_policy() {
        let h = Handlers::new()
            .register("documents.tasks.index_document", |_ctx: TaskContext| async { Ok(Value::Null) })
            .register("documents.tasks.sanity_check", |_ctx: TaskContext| async { Ok(Value::Null) });
        assert_eq!(h.map["documents.tasks.index_document"].autoretry.unwrap().max_retries, Some(5));
        assert!(h.map["documents.tasks.sanity_check"].autoretry.is_none());
        assert_eq!(h.names(), vec!["documents.tasks.index_document", "documents.tasks.sanity_check"]);
    }
}
