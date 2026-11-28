//! Scheduler like `celery beat` with `CELERY_BEAT_SCHEDULE` from paperless.
//!
//! upstream: src/paperless/settings/custom.py (`parse_beat_schedule`),
//! src/paperless/settings/__init__.py (`CELERY_BEAT_SCHEDULE`, `CELERY_BEAT_SCHEDULE_FILENAME`,
//! `CELERY_TIMEZONE`), celery/beat.py@5.6.3 (`ScheduleEntry`, `Scheduler.tick/reserve/apply_entry`,
//! `PersistentScheduler.setup_schedule/merge_inplace`).
//!
//! State (last run per entry) lives in `paperrust_beat_state` instead of
//! `celerybeat-schedule.db`; "due?" and "reserve run" happen in one transaction with a
//! row lock, so that several processes with a scheduler enqueue each run only once.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_core::config::{ConfigError, Env};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tokio::sync::watch;

use crate::cron::Crontab;
use crate::queue::{EnqueueOptions, TaskQueue};

/// An entry of `CELERY_BEAT_SCHEDULE`.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatEntry {
    /// Key, e.g. `"Check all e-mail accounts"`.
    pub name: String,
    pub task: String,
    pub schedule: Crontab,
    /// `options.expires` (seconds).
    pub expires: f64,
    /// `options.headers` = `{"trigger_source": "scheduled"}`.
    pub headers: Map<String, Value>,
}

/// Default schedules (`tasks` in `parse_beat_schedule`): (name, environment variable, default,
/// task, expires).
pub const DEFAULT_ENTRIES: &[(&str, &str, &str, &str, f64)] = &[
    ("Check all e-mail accounts", "PAPERLESS_EMAIL_TASK_CRON", "*/10 * * * *", "paperless_mail.tasks.process_mail_accounts", 9.0 * 60.0),
    ("Train the classifier", "PAPERLESS_TRAIN_TASK_CRON", "5 */1 * * *", "documents.tasks.train_classifier", 59.0 * 60.0),
    ("Optimize the index", "PAPERLESS_INDEX_TASK_CRON", "0 0 * * *", "documents.tasks.index_optimize", 23.0 * 60.0 * 60.0),
    (
        "Perform sanity check",
        "PAPERLESS_SANITY_TASK_CRON",
        "30 0 * * sun",
        "documents.tasks.sanity_check",
        ((7.0 * 24.0) - 1.0) * 60.0 * 60.0,
    ),
    ("Empty trash", "PAPERLESS_EMPTY_TRASH_TASK_CRON", "0 1 * * *", "documents.tasks.empty_trash", 23.0 * 60.0 * 60.0),
    (
        "Check and run scheduled workflows",
        "PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON",
        "5 */1 * * *",
        "documents.tasks.check_scheduled_workflows",
        59.0 * 60.0,
    ),
    ("Rebuild LLM index", "PAPERLESS_LLM_INDEX_TASK_CRON", "10 2 * * *", "documents.tasks.llmindex_index", 23.0 * 60.0 * 60.0),
    (
        "Cleanup expired share link bundles",
        "PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON",
        "0 2 * * *",
        "documents.tasks.cleanup_expired_share_link_bundles",
        23.0 * 60.0 * 60.0,
    ),
    // upstream fix a139bdff4 (#14722, paperless-ngx `dev` after v3.4.2): delete expired sessions
    // daily (Django does not clean up `django_session` itself).
    (
        "Clear expired sessions",
        "PAPERLESS_SESSION_CLEANUP_TASK_CRON",
        "0 3 * * *",
        "documents.tasks.clear_expired_sessions",
        23.0 * 60.0 * 60.0,
    ),
];

/// `int.from_bytes(sha256(secret).digest()) % 10` (big-endian).
pub fn email_offset(secret: &str) -> u32 {
    let digest = Sha256::digest(secret.as_bytes());
    digest.iter().fold(0u32, |acc, b| (acc * 256 + u32::from(*b)) % 10)
}

/// `parse_beat_schedule()`. Errors as upstream at startup (invalid expression, missing
/// `PAPERLESS_SECRET_KEY` with the default mail schedule).
pub fn parse_beat_schedule(env: &Env) -> Result<Vec<BeatEntry>, ConfigError> {
    let mut out = Vec::new();
    for (name, key, default, task, expires) in DEFAULT_ENTRIES {
        let mut value = env.get(key).unwrap_or(default).to_string();
        if value == "disable" {
            continue;
        }
        if *key == "PAPERLESS_EMAIL_TASK_CRON" && !env.contains(key) {
            let secret = env.get("PAPERLESS_SECRET_KEY").ok_or_else(|| ConfigError::Invalid("KeyError: 'PAPERLESS_SECRET_KEY'".into()))?;
            let offset = email_offset(secret);
            let minutes: Vec<String> = (offset..60).step_by(10).map(|m| m.to_string()).collect();
            value = format!("{} * * * *", minutes.join(","));
        }
        let schedule = Crontab::from_paperless(&value).map_err(|e| ConfigError::Invalid(format!("{key}: {e}")))?;
        let mut headers = Map::new();
        headers.insert("trigger_source".into(), Value::String("scheduled".into()));
        out.push(BeatEntry { name: (*name).into(), task: (*task).into(), schedule, expires: *expires, headers });
    }
    Ok(out)
}

/// Result of [`BeatStore::run_due`]: (`None` = not due, else the enqueue result; seconds until
/// the next run).
pub type RunDue = (Option<Result<uuid::Uuid, crate::QueueError>>, f64);

/// Store for `last_run_at`.
pub trait BeatStore: Send + Sync {
    /// Checks under lock whether `entry` is due and reserves the run (`last_run_at = now`).
    /// Returns `(due, seconds until the next run)`.
    fn reserve<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
    ) -> futures::future::BoxFuture<'a, Result<(bool, f64), crate::QueueError>>;
    /// Remove entries that are no longer in the schedule (`merge_inplace`).
    fn retain<'a>(&'a self, names: &'a [String]) -> futures::future::BoxFuture<'a, Result<(), crate::QueueError>>;
    /// Checks under lock whether `entry` is due and, if so, enqueues it via `queue`; the run
    /// (`last_run_at`, `total_run_count`) is recorded ONLY together with a successful enqueue.
    /// After an enqueue error nothing is recorded, so the next tick tries again. Returns
    /// (`None` = not due, else the enqueue result; seconds until the next run).
    fn run_due<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
        queue: &'a dyn TaskQueue,
    ) -> futures::future::BoxFuture<'a, Result<RunDue, crate::QueueError>>;
}

/// Enqueue options of a scheduled run (`apply_entry`).
fn entry_opts(entry: &BeatEntry) -> EnqueueOptions {
    EnqueueOptions { expires: Some(entry.expires), headers: entry.headers.clone(), ..Default::default() }
}

/// Store in PostgreSQL (`paperrust_beat_state`).
#[derive(Debug, Clone)]
pub struct PgBeatStore {
    pool: PgPool,
}

impl PgBeatStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Tasks that would run immediately on the very first start (no state stored yet).
/// Empty (like Celery: `last_run_at = now`, first run at the next due time). Until 2026-11-17
/// this held `documents.tasks.empty_trash` (A-131); after a database takeover it immediately emptied the
/// imported old trash entries before the user could see them, and the
/// migration check reported them as missing (desktop test 2026-11-18). Missed due times are caught up
/// once at the next start by the stored state anyway. docs/contract/pl-queue.md.
pub const CATCH_UP_ON_FIRST_START: &[&str] = &[];

/// `last_run_at` for a new entry: now (Celery `default_now`), for
/// [`CATCH_UP_ON_FIRST_START`] far in the past → due on the first tick.
pub fn initial_last_run(entry: &BeatEntry, now: DateTime<Utc>) -> DateTime<Utc> {
    if CATCH_UP_ON_FIRST_START.contains(&entry.task.as_str()) { now - chrono::Duration::days(400) } else { now }
}

/// Creates the state row if missing, locks it (`FOR UPDATE`, held until the caller's commit) and
/// returns `last_run_at`; a changed time zone resets the state. Shared by `reserve` and `run_due`
/// so each SQL text exists once (sqlx statement cache by SQL text, FINDINGS B3).
async fn lock_state(
    conn: &mut sqlx::PgConnection,
    entry: &BeatEntry,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<DateTime<Utc>, crate::QueueError> {
    // New entry: last_run_at = now (ScheduleEntry.default_now) → first run at the
    // next matching time; exception CATCH_UP_ON_FIRST_START.
    sqlx::query("INSERT INTO paperrust_beat_state (name, task, last_run_at, tz) VALUES ($1, $2, $3, $4) ON CONFLICT (name) DO NOTHING")
        .bind(&entry.name)
        .bind(&entry.task)
        .bind(initial_last_run(entry, now))
        .bind(tz.name())
        .execute(&mut *conn)
        .await?;
    let row = sqlx::query("SELECT last_run_at, tz FROM paperrust_beat_state WHERE name = $1 FOR UPDATE")
        .bind(&entry.name)
        .fetch_one(&mut *conn)
        .await?;
    let last: DateTime<Utc> = row.try_get("last_run_at")?;
    let stored_tz: String = row.try_get("tz")?;
    if stored_tz == tz.name() {
        return Ok(last);
    }
    // "Reset: Timezone changed" → discard state
    tracing::warn!("Reset: Timezone changed from {stored_tz:?} to {:?}", tz.name());
    sqlx::query("UPDATE paperrust_beat_state SET last_run_at = $1, tz = $2, total_run_count = 0 WHERE name = $3")
        .bind(now)
        .bind(tz.name())
        .bind(&entry.name)
        .execute(&mut *conn)
        .await?;
    Ok(now)
}

/// Records a run of `entry` at `now` on the caller's transaction (row locked by [`lock_state`]).
async fn record_run(conn: &mut sqlx::PgConnection, entry: &BeatEntry, now: DateTime<Utc>) -> Result<(), crate::QueueError> {
    sqlx::query("UPDATE paperrust_beat_state SET last_run_at = $1, task = $2, total_run_count = total_run_count + 1 WHERE name = $3")
        .bind(now)
        .bind(&entry.task)
        .bind(&entry.name)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

impl BeatStore for PgBeatStore {
    fn reserve<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
    ) -> futures::future::BoxFuture<'a, Result<(bool, f64), crate::QueueError>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let last = lock_state(&mut tx, entry, now, tz).await?;
            let (due, rem) = entry.schedule.is_due(last, now, tz);
            if due {
                record_run(&mut tx, entry, now).await?;
            }
            tx.commit().await?;
            Ok((due, rem))
        })
    }

    fn retain<'a>(&'a self, names: &'a [String]) -> futures::future::BoxFuture<'a, Result<(), crate::QueueError>> {
        Box::pin(async move {
            sqlx::query("DELETE FROM paperrust_beat_state WHERE NOT (name = ANY($1))").bind(names).execute(&self.pool).await?;
            Ok(())
        })
    }

    fn run_due<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
        queue: &'a dyn TaskQueue,
    ) -> futures::future::BoxFuture<'a, Result<RunDue, crate::QueueError>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let last = lock_state(&mut tx, entry, now, tz).await?;
            let (due, rem) = entry.schedule.is_due(last, now, tz);
            if !due {
                tx.commit().await?;
                return Ok((None, rem));
            }
            // Same transaction as the record (row lock held): either both or neither. A queue
            // without transactional enqueue is sent separately while the lock is held; then a
            // crash between its commit and ours repeats the run (never loses it).
            let sent = match queue.enqueue_in_conn(&mut tx, &entry.task, Map::new(), entry_opts(entry)).await {
                Err(crate::QueueError::Unsupported(_)) => queue.enqueue(&entry.task, Map::new(), entry_opts(entry)).await,
                r => r,
            };
            if sent.is_err() {
                // nothing recorded: due again on the next tick
                let _ = tx.rollback().await;
                return Ok((Some(sent), rem));
            }
            record_run(&mut tx, entry, now).await?;
            tx.commit().await?;
            Ok((Some(sent), rem))
        })
    }
}

/// In-process store (desktop with a single process, tests). Like Celery without a schedule file:
/// no catch-up after a restart.
#[derive(Debug, Default)]
pub struct MemoryBeatStore {
    last: std::sync::Mutex<std::collections::HashMap<String, DateTime<Utc>>>,
}

impl BeatStore for MemoryBeatStore {
    fn reserve<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
    ) -> futures::future::BoxFuture<'a, Result<(bool, f64), crate::QueueError>> {
        Box::pin(async move {
            let mut m = self.last.lock().unwrap_or_else(|p| p.into_inner());
            let last = *m.entry(entry.name.clone()).or_insert_with(|| initial_last_run(entry, now));
            let (due, rem) = entry.schedule.is_due(last, now, tz);
            if due {
                m.insert(entry.name.clone(), now);
            }
            Ok((due, rem))
        })
    }
    fn retain<'a>(&'a self, names: &'a [String]) -> futures::future::BoxFuture<'a, Result<(), crate::QueueError>> {
        Box::pin(async move {
            self.last.lock().unwrap_or_else(|p| p.into_inner()).retain(|k, _| names.contains(k));
            Ok(())
        })
    }

    fn run_due<'a>(
        &'a self,
        entry: &'a BeatEntry,
        now: DateTime<Utc>,
        tz: Tz,
        queue: &'a dyn TaskQueue,
    ) -> futures::future::BoxFuture<'a, Result<RunDue, crate::QueueError>> {
        Box::pin(async move {
            // the lock is not held across the enqueue (single process: ticks run one after another)
            let (due, rem) = {
                let mut m = self.last.lock().unwrap_or_else(|p| p.into_inner());
                let last = *m.entry(entry.name.clone()).or_insert_with(|| initial_last_run(entry, now));
                entry.schedule.is_due(last, now, tz)
            };
            if !due {
                return Ok((None, rem));
            }
            let sent = queue.enqueue(&entry.task, Map::new(), entry_opts(entry)).await;
            if sent.is_ok() {
                self.last.lock().unwrap_or_else(|p| p.into_inner()).insert(entry.name.clone(), now);
            }
            Ok((Some(sent), rem))
        })
    }
}

/// `celery beat` (`DEFAULT_MAX_INTERVAL = 300` s between two checks).
pub struct Beat {
    entries: Vec<BeatEntry>,
    store: Arc<dyn BeatStore>,
    queue: Arc<dyn TaskQueue>,
    tz: Tz,
    max_interval: StdDuration,
}

impl Beat {
    pub fn new(entries: Vec<BeatEntry>, store: Arc<dyn BeatStore>, queue: Arc<dyn TaskQueue>, tz: Tz) -> Self {
        Self { entries, store, queue, tz, max_interval: StdDuration::from_secs(300) }
    }

    /// One round (`Scheduler.tick`): reserve due entries and enqueue them. Returns:
    /// seconds until the next check (at most `max_interval`).
    pub async fn tick(&self, now: DateTime<Utc>) -> f64 {
        self.tick_report(now).await.0
    }

    /// Like [`Self::tick`], plus the first state error of the round (`None` = everything read).
    /// State errors are not logged per entry but once per round by [`Self::run`]
    /// (throttled via backoff).
    async fn tick_report(&self, now: DateTime<Utc>) -> (f64, Option<(String, crate::QueueError)>) {
        let mut next = self.max_interval.as_secs_f64();
        let mut first_err = None;
        for e in &self.entries {
            match self.store.run_due(e, now, self.tz, self.queue.as_ref()).await {
                Ok((None, rem)) => next = next.min(rem),
                Ok((Some(Ok(_)), rem)) => {
                    tracing::info!("Scheduler: Sending due task {} ({})", e.name, e.task);
                    next = next.min(rem);
                }
                // PaperRust (Known difference): Celery keeps the run reserved and only logs; here
                // nothing is recorded and the next tick (after the backoff in `run`) retries.
                Ok((Some(Err(err)), _)) => {
                    tracing::error!(error = %err, "Message Error: Couldn't apply scheduled task {}", e.name);
                    first_err.get_or_insert((e.name.clone(), err));
                }
                Err(err) => {
                    first_err.get_or_insert((e.name.clone(), err));
                }
            }
        }
        (next.max(0.0), first_err)
    }

    /// Loop until `shutdown`. State errors (DB gone, table missing) → next attempt after
    /// exponential backoff (0.5 s → 30 s, never later than the regular due time), one log line per
    /// step. The worker recreates the missing table via migration (same migration 0001).
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let names: Vec<String> = self.entries.iter().map(|e| e.name.clone()).collect();
        if let Err(e) = self.store.retain(&names).await {
            tracing::error!(error = %e, "beat state cleanup failed");
        }
        let mut backoff = crate::backoff::Backoff::default();
        loop {
            if *shutdown.borrow() {
                return;
            }
            let (mut wait, err) = self.tick_report(Utc::now()).await;
            match err {
                Some((entry, e)) => {
                    let step = backoff.fail();
                    tracing::error!(error = %e, entry = %entry, attempt = step.attempt, retry_in_ms = step.delay.as_millis() as u64, "beat state error");
                    wait = wait.min(step.delay.as_secs_f64());
                }
                None => {
                    let n = backoff.succeed();
                    if n > 0 {
                        tracing::info!(failures = n, "beat state readable again");
                    }
                }
            }
            // small margin so that we do not wake up just before the minute
            let wait = StdDuration::from_secs_f64(wait.min(self.max_interval.as_secs_f64()) + 0.01);
            tokio::select! {
                _ = shutdown.changed() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{QueueError, TaskInfo};
    use futures::future::BoxFuture;
    use uuid::Uuid;

    fn env(pairs: &[(&str, &str)]) -> Env {
        let mut v: Vec<(String, String)> = vec![("PAPERLESS_SECRET_KEY".into(), "test-secret".into())];
        v.extend(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Env::from_pairs(v)
    }

    #[test]
    fn default_schedule_like_upstream_test() {
        // upstream: paperless/tests/settings/test_custom_parsers.py::TestParseBeatSchedule
        // (golden offset from Python: sha256("test-secret") % 10 == 6)
        assert_eq!(email_offset("test-secret"), 6);
        assert_eq!(email_offset("paperless-rs-reference-secret-key-local-only-0123456789"), 5);
        let s = parse_beat_schedule(&env(&[])).unwrap();
        assert_eq!(s.len(), 9);
        // upstream fix a139bdff4 (#14722): "Clear expired sessions" daily 03:00, expires 23 h
        let clear = s.iter().find(|e| e.name == "Clear expired sessions").unwrap();
        assert_eq!(clear.task, "documents.tasks.clear_expired_sessions");
        assert_eq!(clear.schedule, Crontab::new("0", "3", "*", "*", "*").unwrap());
        assert_eq!(clear.expires, 82_800.0);
        let names: Vec<&str> = s.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], "Check all e-mail accounts");
        assert_eq!(s[0].schedule, Crontab::new("6,16,26,36,46,56", "*", "*", "*", "*").unwrap());
        assert_eq!(s[1].schedule, Crontab::new("5", "*/1", "*", "*", "*").unwrap());
        assert_eq!(s[3].schedule, Crontab::new("30", "0", "sun", "*", "*").unwrap());
        assert_eq!(s[3].expires, 601_200.0);
        assert_eq!(s[0].expires, 540.0);
        for e in &s {
            assert_eq!(e.headers["trigger_source"], "scheduled");
        }
        let s = parse_beat_schedule(&env(&[("PAPERLESS_EMAIL_TASK_CRON", "*/10 * * * *")])).unwrap();
        assert_eq!(s[0].schedule, Crontab::new("*/10", "*", "*", "*", "*").unwrap());
        let s = parse_beat_schedule(&env(&[("PAPERLESS_EMAIL_TASK_CRON", "*/50 * * * mon")])).unwrap();
        assert_eq!(s[0].schedule, Crontab::new("*/50", "*", "mon", "*", "*").unwrap());
        let s = parse_beat_schedule(&env(&[("PAPERLESS_INDEX_TASK_CRON", "disable")])).unwrap();
        assert!(!s.iter().any(|e| e.name == "Optimize the index"));
        let all: Vec<(&str, &str)> = DEFAULT_ENTRIES.iter().map(|e| (e.1, "disable")).collect();
        assert!(parse_beat_schedule(&env(&all)).unwrap().is_empty());
        assert!(parse_beat_schedule(&env(&[("PAPERLESS_TRAIN_TASK_CRON", "5 * * *")])).is_err());
        assert!(parse_beat_schedule(&Env::from_pairs([("X", "y")])).is_err());
    }

    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<(String, EnqueueOptions)>>);
    impl TaskQueue for Recorder {
        fn enqueue<'a>(&'a self, n: &'a str, _k: Map<String, Value>, o: EnqueueOptions) -> BoxFuture<'a, Result<Uuid, QueueError>> {
            self.0.lock().unwrap().push((n.to_string(), o));
            Box::pin(async { Ok(Uuid::new_v4()) })
        }
        fn status(&self, _id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>> {
            Box::pin(async { Ok(None) })
        }
    }

    #[tokio::test]
    async fn tick_reserves_once_per_slot() {
        let q = Arc::new(Recorder::default());
        let entries = vec![BeatEntry {
            name: "Train".into(),
            task: "documents.tasks.train_classifier".into(),
            schedule: Crontab::from_paperless("5 */1 * * *").unwrap(),
            expires: 3540.0,
            headers: parse_beat_schedule(&env(&[])).unwrap()[0].headers.clone(),
        }];
        let beat = Beat::new(entries, Arc::new(MemoryBeatStore::default()), q.clone(), "UTC".parse().unwrap());
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        // Start 10:00 → not due, 300 s (max_interval)
        assert_eq!(beat.tick(t("2024-01-01T10:00:00Z")).await, 300.0);
        assert_eq!(beat.tick(t("2024-01-01T10:04:00Z")).await, 60.0);
        assert!(q.0.lock().unwrap().is_empty());
        // 10:05:00 due → enqueued once, next run 11:05
        assert_eq!(beat.tick(t("2024-01-01T10:05:00Z")).await, 300.0);
        assert_eq!(beat.tick(t("2024-01-01T10:05:30Z")).await, 300.0);
        let sent = q.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "documents.tasks.train_classifier");
        assert_eq!(sent[0].1.expires, Some(3540.0));
        assert_eq!(sent[0].1.headers["trigger_source"], "scheduled");
        // three missed runs → exactly one catch-up
        beat.tick(t("2024-01-01T13:30:00Z")).await;
        assert_eq!(q.0.lock().unwrap().len(), 2);
    }

    /// Queue whose first `fail` enqueues fail.
    struct FlakyQueue {
        fail: std::sync::Mutex<usize>,
        sent: std::sync::Mutex<Vec<String>>,
    }
    impl TaskQueue for FlakyQueue {
        fn enqueue<'a>(&'a self, n: &'a str, _k: Map<String, Value>, _o: EnqueueOptions) -> BoxFuture<'a, Result<Uuid, QueueError>> {
            let failing = {
                let mut f = self.fail.lock().unwrap();
                let failing = *f > 0;
                *f = f.saturating_sub(1);
                failing
            };
            if !failing {
                self.sent.lock().unwrap().push(n.to_string());
            }
            Box::pin(async move { if failing { Err(QueueError::Invalid("queue down".into())) } else { Ok(Uuid::new_v4()) } })
        }
        fn status(&self, _id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// Regression: an enqueue error must not consume the scheduled run; the next tick retries it
    /// and the error is reported to `run` (backoff) instead of only being logged.
    #[tokio::test]
    async fn failed_enqueue_is_retried_not_lost() {
        let q = Arc::new(FlakyQueue { fail: std::sync::Mutex::new(1), sent: Default::default() });
        let entries = vec![BeatEntry {
            name: "Train".into(),
            task: "documents.tasks.train_classifier".into(),
            schedule: Crontab::from_paperless("5 */1 * * *").unwrap(),
            expires: 3540.0,
            headers: Map::new(),
        }];
        let beat = Beat::new(entries, Arc::new(MemoryBeatStore::default()), q.clone(), "UTC".parse().unwrap());
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        beat.tick(t("2024-01-01T10:00:00Z")).await;
        let (_, err) = beat.tick_report(t("2024-01-01T10:05:00Z")).await;
        assert!(err.is_some(), "enqueue error is reported");
        assert!(q.sent.lock().unwrap().is_empty());
        // next tick (same slot): still due, now sent exactly once
        beat.tick(t("2024-01-01T10:05:02Z")).await;
        beat.tick(t("2024-01-01T10:05:30Z")).await;
        assert_eq!(*q.sent.lock().unwrap(), vec!["documents.tasks.train_classifier".to_string()]);
    }

    /// "Empty trash" does NOT run immediately on first start (after a takeover old
    /// trash entries would otherwise stay invisible), but like Celery at the next due time.
    #[tokio::test]
    async fn empty_trash_waits_for_schedule_on_first_start() {
        let q = Arc::new(Recorder::default());
        let entries = parse_beat_schedule(&env(&[])).unwrap();
        let beat = Beat::new(entries, Arc::new(MemoryBeatStore::default()), q.clone(), "UTC".parse().unwrap());
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let n_trash = |q: &Recorder| q.0.lock().unwrap().iter().filter(|s| s.0 == "documents.tasks.empty_trash").count();
        beat.tick(t("2024-01-01T10:00:00Z")).await;
        assert_eq!(n_trash(&q), 0);
        beat.tick(t("2024-01-02T01:00:00Z")).await;
        assert_eq!(n_trash(&q), 1);
    }
}
