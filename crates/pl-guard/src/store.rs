//! Tables of pl-guard (migrations/0001_paperrust_guard.sql). Created on first access
//! ([`ensure`]) or with `paperrust manage migrate`.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};

/// Migration (name, SQL) for `paperrust manage migrate`.
pub const MIGRATION: (&str, &str) = ("pl_guard.0001_paperrust_guard", include_str!("../migrations/0001_paperrust_guard.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

static READY: AtomicBool = AtomicBool::new(false);

fn statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in sql.lines() {
        let l = line.split("--").next().unwrap_or("").trim_end();
        if l.trim().is_empty() {
            continue;
        }
        cur.push_str(l);
        cur.push('\n');
        if l.ends_with(';') {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Create tables (idempotent, advisory lock).
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_guard'))").execute(&mut *tx).await?;
    for stmt in statements(MIGRATION.1) {
        sqlx::query(&stmt).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// [`migrate`] once per process.
pub async fn ensure(pool: &PgPool) -> Result<(), sqlx::Error> {
    if !READY.load(Ordering::Relaxed) {
        migrate(pool).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------ State

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GuardState {
    pub read_only: bool,
    pub since: Option<DateTime<Utc>>,
    pub reason: String,
    pub event_id: Option<i64>,
    pub lifted_at: Option<DateTime<Utc>>,
    pub lifted_by: Option<i32>,
}

pub async fn state(pool: &PgPool) -> Result<GuardState, sqlx::Error> {
    ensure(pool).await?;
    let r = sqlx::query("SELECT read_only, since, reason, event_id, lifted_at, lifted_by FROM paperrust_guard_state WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    Ok(match r {
        Some(r) => GuardState {
            read_only: r.try_get("read_only")?,
            since: r.try_get("since")?,
            reason: r.try_get("reason")?,
            event_id: r.try_get("event_id")?,
            lifted_at: r.try_get("lifted_at")?,
            lifted_by: r.try_get("lifted_by")?,
        },
        None => GuardState { read_only: false, since: None, reason: String::new(), event_id: None, lifted_at: None, lifted_by: None },
    })
}

/// Switch read-only mode on (stays on until an admin lifts it).
pub async fn set_read_only(pool: &PgPool, reason: &str, event_id: Option<i64>) -> Result<(), sqlx::Error> {
    ensure(pool).await?;
    sqlx::query(
        "INSERT INTO paperrust_guard_state (id, read_only, since, reason, event_id) VALUES (1, true, now(), $1, $2)
         ON CONFLICT (id) DO UPDATE SET read_only = true, since = COALESCE(CASE WHEN paperrust_guard_state.read_only THEN paperrust_guard_state.since END, now()),
           reason = $1, event_id = COALESCE($2, paperrust_guard_state.event_id)",
    )
    .bind(reason)
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Lift read-only mode; open events count as reviewed.
pub async fn lift_read_only(pool: &PgPool, user_id: Option<i32>, note: &str) -> Result<(), sqlx::Error> {
    ensure(pool).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE paperrust_guard_state SET read_only = false, lifted_at = now(), lifted_by = $1 WHERE id = 1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE paperrust_guard_event SET status = 'resolved', resolved_at = now(), resolved_by = $1, note = $2 WHERE status = 'open'")
        .bind(user_id)
        .bind(note)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

// ------------------------------------------------------------------- Events and findings

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: i64,
    pub created: DateTime<Utc>,
    pub status: String,
    pub files: i32,
    pub summary: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<i32>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingRow {
    pub id: i64,
    pub created: DateTime<Utc>,
    pub event_id: Option<i64>,
    pub path: String,
    pub reason: String,
    pub detail: String,
    pub weight: i32,
}

pub async fn insert_finding(pool: &PgPool, path: &str, reason: &str, detail: &str, weight: i32, event_id: Option<i64>) -> Result<i64, sqlx::Error> {
    ensure(pool).await?;
    sqlx::query_scalar("INSERT INTO paperrust_guard_finding (path, reason, detail, weight, event_id) VALUES ($1, $2, $3, $4, $5) RETURNING id")
        .bind(path)
        .bind(reason)
        .bind(detail)
        .bind(weight)
        .bind(event_id)
        .fetch_one(pool)
        .await
}

/// Sum of the weights of all findings without an event in the last `minutes` minutes.
pub async fn recent_weight(pool: &PgPool, minutes: i64) -> Result<i64, sqlx::Error> {
    let v: Option<i64> = sqlx::query_scalar(
        "SELECT sum(weight)::bigint FROM paperrust_guard_finding WHERE event_id IS NULL AND created > now() - make_interval(mins => $1::int)",
    )
    .bind(minutes as i32)
    .fetch_one(pool)
    .await?;
    Ok(v.unwrap_or(0))
}

/// Open event (the most recent), if any.
pub async fn open_event(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM paperrust_guard_event WHERE status = 'open' ORDER BY id DESC LIMIT 1").fetch_optional(pool).await
}

/// Create an event and attach the free findings of the last `minutes` minutes to it.
pub async fn create_event(pool: &PgPool, minutes: i64, summary: &str) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO paperrust_guard_event (summary) VALUES ($1) RETURNING id").bind(summary).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE paperrust_guard_finding SET event_id = $1 WHERE event_id IS NULL AND created > now() - make_interval(mins => $2::int)")
        .bind(id)
        .bind(minutes as i32)
        .execute(&mut *tx)
        .await?;
    refresh_event_count(&mut tx, id).await?;
    tx.commit().await?;
    Ok(id)
}

async fn refresh_event_count(conn: &mut sqlx::PgConnection, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE paperrust_guard_event SET files = (SELECT count(DISTINCT path) FROM paperrust_guard_finding WHERE event_id = $1) WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Recount the file count of an event (after further findings).
pub async fn touch_event(pool: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    let mut c = pool.acquire().await?;
    refresh_event_count(&mut c, id).await
}

fn event_row(r: &sqlx::postgres::PgRow) -> Result<Event, sqlx::Error> {
    Ok(Event {
        id: r.try_get("id")?,
        created: r.try_get("created")?,
        status: r.try_get("status")?,
        files: r.try_get("files")?,
        summary: r.try_get("summary")?,
        resolved_at: r.try_get("resolved_at")?,
        resolved_by: r.try_get("resolved_by")?,
        note: r.try_get("note")?,
    })
}

pub async fn list_events(pool: &PgPool, limit: i64) -> Result<Vec<Event>, sqlx::Error> {
    ensure(pool).await?;
    let rows = sqlx::query("SELECT id, created, status, files, summary, resolved_at, resolved_by, note FROM paperrust_guard_event ORDER BY id DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(event_row).collect()
}

pub async fn get_event(pool: &PgPool, id: i64) -> Result<Option<Event>, sqlx::Error> {
    ensure(pool).await?;
    let r = sqlx::query("SELECT id, created, status, files, summary, resolved_at, resolved_by, note FROM paperrust_guard_event WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    r.as_ref().map(event_row).transpose()
}

/// Findings of an event (`Some(id)`) or the free ones of the recent past (`None`).
pub async fn findings(pool: &PgPool, event_id: Option<i64>, limit: i64) -> Result<Vec<FindingRow>, sqlx::Error> {
    ensure(pool).await?;
    let rows = match event_id {
        Some(id) => {
            sqlx::query("SELECT id, created, event_id, path, reason, detail, weight FROM paperrust_guard_finding WHERE event_id = $1 ORDER BY id LIMIT $2")
                .bind(id)
                .bind(limit)
                .fetch_all(pool)
                .await?
        }
        None => {
            sqlx::query("SELECT id, created, event_id, path, reason, detail, weight FROM paperrust_guard_finding WHERE event_id IS NULL ORDER BY id DESC LIMIT $1")
                .bind(limit)
                .fetch_all(pool)
                .await?
        }
    };
    rows.iter()
        .map(|r| {
            Ok(FindingRow {
                id: r.try_get("id")?,
                created: r.try_get("created")?,
                event_id: r.try_get("event_id")?,
                path: r.try_get("path")?,
                reason: r.try_get("reason")?,
                detail: r.try_get("detail")?,
                weight: r.try_get("weight")?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------- Backup runs

#[derive(Debug, Clone, Serialize, Default)]
pub struct BackupRun {
    pub id: i64,
    pub started: Option<DateTime<Utc>>,
    pub finished: Option<DateTime<Utc>>,
    pub status: String,
    pub trigger: String,
    pub target: String,
    pub snapshot: Option<String>,
    pub files_total: i64,
    pub files_new: i64,
    pub bytes_total: i64,
    pub bytes_new: i64,
    pub db_bytes: i64,
    pub pruned: i32,
    pub warnings: String,
    pub error: String,
}

pub async fn start_run(pool: &PgPool, trigger: &str, target: &str) -> Result<i64, sqlx::Error> {
    ensure(pool).await?;
    sqlx::query_scalar("INSERT INTO paperrust_backup_run (trig, target) VALUES ($1, $2) RETURNING id")
        .bind(trigger)
        .bind(target)
        .fetch_one(pool)
        .await
}

pub async fn finish_run(pool: &PgPool, r: &BackupRun) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE paperrust_backup_run SET finished = now(), status = $2, snapshot = $3, files_total = $4, files_new = $5,
           bytes_total = $6, bytes_new = $7, db_bytes = $8, pruned = $9, warnings = $10, error = $11 WHERE id = $1",
    )
    .bind(r.id)
    .bind(&r.status)
    .bind(&r.snapshot)
    .bind(r.files_total)
    .bind(r.files_new)
    .bind(r.bytes_total)
    .bind(r.bytes_new)
    .bind(r.db_bytes)
    .bind(r.pruned)
    .bind(&r.warnings)
    .bind(&r.error)
    .execute(pool)
    .await?;
    Ok(())
}

fn run_row(r: &sqlx::postgres::PgRow) -> Result<BackupRun, sqlx::Error> {
    Ok(BackupRun {
        id: r.try_get("id")?,
        started: r.try_get("started")?,
        finished: r.try_get("finished")?,
        status: r.try_get("status")?,
        trigger: r.try_get("trig")?,
        target: r.try_get("target")?,
        snapshot: r.try_get("snapshot")?,
        files_total: r.try_get("files_total")?,
        files_new: r.try_get("files_new")?,
        bytes_total: r.try_get("bytes_total")?,
        bytes_new: r.try_get("bytes_new")?,
        db_bytes: r.try_get("db_bytes")?,
        pruned: r.try_get("pruned")?,
        warnings: r.try_get("warnings")?,
        error: r.try_get("error")?,
    })
}

const RUN_COLS: &str = "id, started, finished, status, trig, target, snapshot, files_total, files_new, bytes_total, bytes_new, db_bytes, pruned, warnings, error";

pub async fn list_runs(pool: &PgPool, limit: i64) -> Result<Vec<BackupRun>, sqlx::Error> {
    ensure(pool).await?;
    let rows = sqlx::query(&format!("SELECT {RUN_COLS} FROM paperrust_backup_run ORDER BY id DESC LIMIT $1")).bind(limit).fetch_all(pool).await?;
    rows.iter().map(run_row).collect()
}

pub async fn last_success(pool: &PgPool) -> Result<Option<BackupRun>, sqlx::Error> {
    ensure(pool).await?;
    let r = sqlx::query(&format!("SELECT {RUN_COLS} FROM paperrust_backup_run WHERE status = 'ok' ORDER BY id DESC LIMIT 1")).fetch_optional(pool).await?;
    r.as_ref().map(run_row).transpose()
}

/// Mark runs that have been listed as "running" for longer than `hours` (process aborted) as failed.
pub async fn fail_stale_runs(pool: &PgPool, hours: i64) -> Result<u64, sqlx::Error> {
    ensure(pool).await?;
    let r = sqlx::query(
        "UPDATE paperrust_backup_run SET status = 'error', finished = now(), error = 'interrupted (process stopped)'
         WHERE status = 'running' AND started < now() - make_interval(hours => $1::int)",
    )
    .bind(hours as i32)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    #[test]
    fn migration_splits() {
        let s = super::statements(super::MIGRATION.1);
        assert!(s.len() >= 9, "{}", s.len());
        assert!(s.iter().all(|x| x.trim_end().ends_with(';')));
    }
}
