//! `GET /api/status/` – `SystemStatusView`, switched over to PaperRust components.
//!
//! upstream: src/documents/views.py@v3.4.2 (`SystemStatusView.get`),
//! src/paperless/config.py@v3.4.2 (`AIConfig.llm_index_enabled`).
//! UI: upstream/src-ui/src/app/data/system-status.ts (keys read).
//!
//! PaperRust (R4, 2026-11-09, contract docs/contract/pl-system.md "System status"):
//!
//! * All checks run **in parallel**, each at most [`CHECK_TIMEOUT`] (1 s).
//! * The keys of the UI and of the apps stay (`pngx_version`, `database`,
//!   `tasks.redis_*`, `tasks.celery_*`, …), with PaperRust meaning (Celery/Redis no longer exist
//!   since 2026-11-14):
//!   - `redis_*` = **cache** (process-local → `OK` without connection; `PAPERRUST_CACHE=postgres` →
//!     query of the table `paperrust_cache`),
//!   - `celery_*` = **PaperRust worker** (`paperrust_task_queue`: connected worker, lease,
//!     waiting tasks).
//! * Additionally `paperrust` with all PaperRust checks (database, worker, cache, search index,
//!   classifier, sanity check, OCR, AI, storage) and the runtime.
//! * An error of a check (also database) is reported instead of ending as a 500.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use pl_authz::{AuthUser, perms};
use pl_core::sqlbind::SqlxBinder as _;
use pl_core::{AppState, Config, Drf, RequestContext};
use sea_query::{Alias, Order, PostgresQueryBuilder, Query};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row as _};

use crate::error::{Result, insufficient_permissions};
use crate::sysinfo;
use pl_core::brand::{PAPERRUST_VERSION, PRODUCT_NAME, UPSTREAM_VERSION};

#[path = "migrations_generated.rs"]
#[allow(clippy::type_complexity)]
mod migrations_generated;

/// `SystemStatusView.TASK_SUMMARY_DAYS`
pub const TASK_SUMMARY_DAYS: i64 = 30;
/// Upper limit per check.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(1);
/// A waiting task counts as "stuck" from this age (via `eta`).
pub const QUEUE_OVERDUE_SECS: i64 = 60;
/// A worker heartbeat younger than this counts as a sign of life.
pub const WORKER_HEARTBEAT_FRESH_SECS: i64 = 120;
/// Below this share of free storage `paperrust.checks.storage` reports `WARNING`.
pub const STORAGE_WARN_FRACTION: f64 = 0.05;
/// How long the OCR tool search stays valid.
const OCR_CACHE_TTL: Duration = Duration::from_secs(60);

const OK: &str = "OK";
const WARNING: &str = "WARNING";
const ERROR: &str = "ERROR";
const DISABLED: &str = "DISABLED";

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Raw datetime as rendered by DRF's JSON encoder for an aware UTC value.
fn raw_dt(v: Option<DateTime<Utc>>) -> Value {
    pl_core::datetime::format_datetime_opt(v.as_ref(), &chrono_tz::UTC)
}

/// `fut` at most [`CHECK_TIMEOUT`]; `None` on timeout.
async fn bounded<T>(fut: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(CHECK_TIMEOUT, fut).await.ok()
}

/// Blocking file system check outside the runtime thread, at most [`CHECK_TIMEOUT`].
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::time::timeout(CHECK_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(v)) => Some(v),
        _ => None,
    }
}

fn ms(d: Duration) -> Value {
    Value::from((d.as_secs_f64() * 1000.0 * 10.0).round() / 10.0)
}

fn timed_out(what: &str) -> String {
    tracing::error!("System status: {what} check timed out after {CHECK_TIMEOUT:?}");
    format!("{what} check timed out.")
}

// ------------------------------------------------------------------------------------------
// database
// ------------------------------------------------------------------------------------------

/// `MigrationLoader.graph.nodes` with squash handling (`replaces`), given the applied set.
pub fn graph_nodes(applied: &[String]) -> Vec<String> {
    let is_applied = |app: &str, name: &str| applied.iter().any(|x| x == &format!("{app}.{name}"));
    let mut removed: Vec<(String, String)> = Vec::new();
    let mut nodes: Vec<(String, String)> = Vec::new();
    for (app, name, replaces) in migrations_generated::MIGRATIONS {
        if replaces.is_empty() {
            continue;
        }
        let done = replaces.iter().filter(|(ra, rn)| is_applied(ra, rn)).count();
        if done == 0 || done == replaces.len() {
            // squashed migration used, replaced ones removed
            removed.extend(replaces.iter().map(|(ra, rn)| (ra.to_string(), rn.to_string())));
        } else {
            removed.push((app.to_string(), name.to_string()));
        }
    }
    for (app, name, _) in migrations_generated::MIGRATIONS {
        let key = (app.to_string(), name.to_string());
        if !removed.contains(&key) {
            nodes.push(key);
        }
    }
    nodes.into_iter().map(|(a, n)| format!("{a}.{n}")).collect()
}

async fn applied_migrations(db: &PgPool) -> std::result::Result<Vec<String>, sqlx::Error> {
    let q = Query::select().column(a("app")).column(a("name")).from(a("django_migrations")).order_by(a("id"), Order::Asc).to_owned();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter().map(|r| Ok(format!("{}.{}", r.try_get::<String, _>("app")?, r.try_get::<String, _>("name")?))).collect()
}

/// (`database` object of the UI, check for `paperrust`).
async fn database_status(db: &PgPool, name: &str) -> (Value, Value) {
    let t0 = Instant::now();
    let applied = match bounded(applied_migrations(db)).await {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => {
            tracing::error!("System status detected a possible problem while connecting to the database: {e}");
            Err(None)
        }
        None => Err(Some(timed_out("Database"))),
    };
    let elapsed = t0.elapsed();
    let mut m = Map::new();
    m.insert("type".into(), json!("postgresql"));
    m.insert("url".into(), json!(name));
    let check = match applied {
        Ok(applied) => {
            let unapplied: Vec<String> = graph_nodes(&applied).into_iter().filter(|n| !applied.contains(n)).collect();
            m.insert("status".into(), json!(OK));
            m.insert("error".into(), Value::Null);
            let pending = unapplied.len();
            m.insert("migration_status".into(), json!({"latest_migration": applied.last(), "unapplied_migrations": unapplied}));
            let (st, err) = if pending == 0 { (OK, Value::Null) } else { (WARNING, json!(format!("{pending} unapplied migration(s)."))) };
            json!({"status": st, "error": err, "latency_ms": ms(elapsed)})
        }
        Err(timeout) => {
            let msg = "Error connecting to database, check logs for more detail.";
            m.insert("status".into(), json!(ERROR));
            m.insert("error".into(), json!(msg));
            m.insert("migration_status".into(), json!({"latest_migration": null, "unapplied_migrations": []}));
            json!({"status": ERROR, "error": timeout.unwrap_or_else(|| msg.to_string()), "latency_ms": ms(elapsed)})
        }
    };
    (Value::Object(m), check)
}

// ------------------------------------------------------------------------------------------
// PaperRust-Worker (PgQueue)
// ------------------------------------------------------------------------------------------

/// State of `paperrust_task_queue` (one query).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QueueStats {
    pub queued: i64,
    pub running: i64,
    /// running, but lease expired (worker lost)
    pub stale: i64,
    /// waiting longer than [`QUEUE_OVERDUE_SECS`]
    pub overdue: i64,
    /// heartbeat younger than [`WORKER_HEARTBEAT_FRESH_SECS`]
    pub fresh_heartbeats: i64,
    /// Sessions with `LISTEN paperrust_task_queue` (waiting workers)
    pub listeners: i64,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub last_finished: Option<DateTime<Utc>>,
}

async fn queue_stats(db: &PgPool) -> std::result::Result<QueueStats, sqlx::Error> {
    // Constants in the SQL (no user input). pg_stat_activity shows the last statement for idle
    // sessions – for the worker listener `LISTEN "paperrust_task_queue"`.
    let sql = format!(
        "SELECT \
           COUNT(*) FILTER (WHERE state = 'queued') AS queued, \
           COUNT(*) FILTER (WHERE state = 'running') AS running, \
           COUNT(*) FILTER (WHERE state = 'running' AND locked_until < now()) AS stale, \
           COUNT(*) FILTER (WHERE state = 'queued' AND eta < now() - interval '{QUEUE_OVERDUE_SECS} seconds') AS overdue, \
           COUNT(*) FILTER (WHERE heartbeat_at > now() - interval '{WORKER_HEARTBEAT_FRESH_SECS} seconds') AS fresh, \
           MAX(heartbeat_at) AS last_heartbeat, \
           MAX(finished_at) AS last_finished, \
           (SELECT COUNT(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND query ILIKE 'LISTEN%paperrust_task_queue%') AS listeners \
         FROM paperrust_task_queue"
    );
    let r = sqlx::query(&sql).fetch_one(db).await?;
    Ok(QueueStats {
        queued: r.try_get("queued")?,
        running: r.try_get("running")?,
        stale: r.try_get("stale")?,
        overdue: r.try_get("overdue")?,
        fresh_heartbeats: r.try_get("fresh")?,
        listeners: r.try_get("listeners")?,
        last_heartbeat: r.try_get("last_heartbeat")?,
        last_finished: r.try_get("last_finished")?,
    })
}

/// Evaluation of the queue → (status, error text).
pub fn worker_verdict(s: &QueueStats) -> (&'static str, Option<&'static str>) {
    let alive = s.listeners > 0 || s.fresh_heartbeats > 0 || s.running > s.stale;
    if s.stale > 0 {
        (WARNING, Some("A PaperRust worker stopped responding; its tasks will be retried."))
    } else if !alive {
        (WARNING, Some("No PaperRust worker is connected to the task queue. This may be temporary."))
    } else if s.overdue > 0 {
        (WARNING, Some("Tasks have been waiting in the queue for more than a minute."))
    } else {
        (OK, None)
    }
}

/// (`celery_status`, `celery_url`, `celery_error`, check for `paperrust`).
async fn worker_status_pg(db: &PgPool, db_name: &str) -> (String, Value, Value, Value) {
    let url = json!(format!("postgresql://{db_name}/paperrust_task_queue"));
    match bounded(queue_stats(db)).await {
        Some(Ok(s)) => {
            let (st, err) = worker_verdict(&s);
            let check = json!({
                "backend": "postgres",
                "status": st,
                "error": err,
                "connected_workers": s.listeners,
                "queued": s.queued,
                "running": s.running,
                "stale": s.stale,
                "overdue": s.overdue,
                "last_heartbeat": raw_dt(s.last_heartbeat),
                "last_finished": raw_dt(s.last_finished),
            });
            (st.into(), url, err.map_or(Value::Null, Value::from), check)
        }
        Some(Err(e)) => {
            tracing::error!("System status detected a possible problem with the task queue: {e}");
            let msg = "Error checking the task queue, check logs for more detail.";
            (ERROR.into(), url, json!(msg), json!({"backend": "postgres", "status": ERROR, "error": msg}))
        }
        None => {
            let msg = timed_out("Task queue");
            (ERROR.into(), url, json!(msg), json!({"backend": "postgres", "status": ERROR, "error": msg}))
        }
    }
}

// ------------------------------------------------------------------------------------------
// tasks (one query for classifier, sanity check, LLM index, overview, AI configuration)
// ------------------------------------------------------------------------------------------

struct LastTask {
    status: String,
    date_done: Option<DateTime<Utc>>,
    result_data: Option<Value>,
}

#[derive(Default)]
struct TaskInfo {
    classifier: Option<LastTask>,
    sanity: Option<LastTask>,
    llm_index: Option<LastTask>,
    summary: [i64; 4],
    ai_enabled: Option<bool>,
    embedding_backend: Option<String>,
    llm_backend: Option<String>,
}

const TASK_INFO_SQL: &str = "\
SELECT c.status AS c_status, c.date_done AS c_done, c.result_data AS c_res, \
       s.status AS s_status, s.date_done AS s_done, s.result_data AS s_res, \
       l.status AS l_status, l.date_done AS l_done, l.result_data AS l_res, \
       sm.total_count, sm.pending_count, sm.success_count, sm.failure_count, \
       ac.ai_enabled, ac.llm_embedding_backend, ac.llm_backend \
FROM (SELECT 1) AS one \
LEFT JOIN LATERAL (SELECT status, date_done, result_data FROM documents_paperlesstask \
    WHERE task_type = 'train_classifier' AND status = ANY($1) ORDER BY date_done DESC LIMIT 1) c ON true \
LEFT JOIN LATERAL (SELECT status, date_done, result_data FROM documents_paperlesstask \
    WHERE task_type = 'sanity_check' AND status = ANY($1) ORDER BY date_done DESC LIMIT 1) s ON true \
LEFT JOIN LATERAL (SELECT status, date_done, result_data FROM documents_paperlesstask \
    WHERE task_type = 'llm_index' ORDER BY date_done DESC LIMIT 1) l ON true \
CROSS JOIN LATERAL (SELECT COUNT(id) AS total_count, \
    COUNT(id) FILTER (WHERE status = 'pending') AS pending_count, \
    COUNT(id) FILTER (WHERE status = 'success') AS success_count, \
    COUNT(id) FILTER (WHERE status = 'failure') AS failure_count \
    FROM documents_paperlesstask WHERE date_created >= $2) sm \
LEFT JOIN LATERAL (SELECT ai_enabled, llm_embedding_backend, llm_backend \
    FROM paperless_applicationconfiguration ORDER BY id LIMIT 1) ac ON true";

fn last_from(r: &sqlx::postgres::PgRow, p: &str) -> std::result::Result<Option<LastTask>, sqlx::Error> {
    let status: Option<String> = r.try_get(format!("{p}_status").as_str())?;
    Ok(match status {
        None => None,
        Some(status) => Some(LastTask {
            status,
            date_done: r.try_get(format!("{p}_done").as_str())?,
            result_data: r.try_get::<Option<Value>, _>(format!("{p}_res").as_str())?.filter(|v| !v.is_null()),
        }),
    })
}

async fn task_info(db: &PgPool) -> std::result::Result<TaskInfo, sqlx::Error> {
    let cutoff = Utc::now() - chrono::Duration::days(TASK_SUMMARY_DAYS);
    let complete: Vec<String> = crate::labels::COMPLETE_STATUSES.iter().map(|s| s.to_string()).collect();
    let r = sqlx::query(TASK_INFO_SQL).bind(complete).bind(cutoff).fetch_one(db).await?;
    Ok(TaskInfo {
        classifier: last_from(&r, "c")?,
        sanity: last_from(&r, "s")?,
        llm_index: last_from(&r, "l")?,
        summary: [r.try_get("total_count")?, r.try_get("pending_count")?, r.try_get("success_count")?, r.try_get("failure_count")?],
        ai_enabled: r.try_get("ai_enabled")?,
        embedding_backend: r.try_get("llm_embedding_backend")?,
        llm_backend: r.try_get("llm_backend")?,
    })
}

/// `result_data.get("error_message") if result_data else None`
fn error_message(t: &LastTask) -> Value {
    match &t.result_data {
        Some(v) if crate::tasks::serialize::truthy(v) => v.get("error_message").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// classifier / sanity check: (status, last, error)
fn task_check(t: Option<&LastTask>, missing: &str) -> (Value, Value, Value) {
    match t {
        None => (json!(WARNING), Value::Null, json!(missing)),
        Some(t) if t.status != "success" => (json!(ERROR), raw_dt(t.date_done), error_message(t)),
        Some(t) => (json!(OK), raw_dt(t.date_done), Value::Null),
    }
}

/// `PAPERLESS_AI_LLM_EMBEDDING_BACKEND` (`get_choice_from_env`).
fn env_embedding_backend(env: &pl_core::config::Env) -> Option<String> {
    env.get("PAPERLESS_AI_LLM_EMBEDDING_BACKEND").filter(|v| ["huggingface", "openai-like", "ollama"].contains(v)).map(str::to_string)
}

/// `PAPERLESS_AI_LLM_BACKEND`.
fn env_llm_backend(env: &pl_core::config::Env) -> Option<String> {
    env.get("PAPERLESS_AI_LLM_BACKEND").filter(|v| !v.is_empty()).map(str::to_string)
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|b| !b.is_empty())
}

// ------------------------------------------------------------------------------------------
// File system: index, classifier file, storage, OCR tools
// ------------------------------------------------------------------------------------------

/// `index_status`, `index_last_modified`, `index_error`.
fn index_status(index_dir: &Path, tz: &chrono_tz::Tz) -> (Value, Value, Value) {
    let ok = index_dir.join("meta.json").is_file();
    let newest = if ok { sysinfo::newest_file_mtime_ns(index_dir) } else { Err(std::io::Error::other("index missing")) };
    match newest {
        Ok(ns) => {
            let last = ns.and_then(sysinfo::fromtimestamp_local).map(|n| sysinfo::make_aware(n, *tz)).map(|d| {
                let s = pl_core::datetime::python_isoformat(&d);
                match s.strip_suffix("+00:00") {
                    Some(b) => format!("{b}Z"),
                    None => s,
                }
            });
            (json!(OK), last.map_or(Value::Null, Value::from), Value::Null)
        }
        Err(e) => {
            tracing::error!("System status detected a possible problem while opening the index: {e}");
            (json!(ERROR), Value::Null, json!("Error opening index, check logs for more detail."))
        }
    }
}

/// Classifier file: (present, modification time).
fn model_file(path: &Path) -> (bool, Option<DateTime<Utc>>) {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => (true, m.modified().ok().map(DateTime::<Utc>::from)),
        _ => (false, None),
    }
}

/// Found OCR tools (file search only, no process start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrTools {
    pub tesseract: bool,
    pub pdfium: bool,
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

fn pdfium_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Like `pl_ocr::TesseractCli::from_env`: `PAPERRUST_TESSERACT_PATH`, otherwise `tesseract` in `PATH`.
fn find_tesseract(configured: Option<&str>) -> bool {
    if let Some(p) = configured.filter(|p| !p.is_empty()) {
        let p = Path::new(p);
        if p.components().count() > 1 || p.is_absolute() {
            return p.is_file();
        }
        let name = p.to_string_lossy();
        return path_dirs().iter().any(|d| d.join(&*name).is_file() || d.join(exe_name(&name)).is_file());
    }
    let name = exe_name("tesseract");
    path_dirs().iter().any(|d| d.join(&name).is_file())
}

/// Like `pl_ocr::PdfiumRenderer::from_env`: `PAPERRUST_PDFIUM_PATH` (file/directory), otherwise the
/// usual search locations of the library loader (program directory, `PATH`, system libraries).
fn find_pdfium(configured: Option<&str>) -> bool {
    let lib = pdfium_name();
    if let Some(p) = configured.filter(|p| !p.is_empty()) {
        let p = Path::new(p);
        return if p.is_dir() { p.join(lib).is_file() } else { p.is_file() };
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        // Like the loader (pl_ocr::render::pdfium_candidates): next to the program and in the
        // subfolder `pdfium/` (layout of the desktop package).
        dirs.push(d.join("pdfium"));
        dirs.push(d);
    }
    if let Ok(d) = std::env::current_dir() {
        dirs.push(d);
    }
    dirs.extend(path_dirs());
    if !cfg!(windows) {
        if let Some(p) = std::env::var_os("LD_LIBRARY_PATH") {
            dirs.extend(std::env::split_paths(&p));
        }
        for d in [
            "/usr/lib",
            "/usr/local/lib",
            "/usr/lib64",
            "/lib",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/opt/homebrew/lib",
        ] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs.iter().any(|d| d.join(lib).is_file())
}

/// Path of the PaperRust classifier model (like `pl_consumer::ConsumerSettings` and
/// `pl_cli::classifier::model_file`).
pub fn classifier_model_path(cfg: &Config) -> std::path::PathBuf {
    pl_core::config::classifier_model_file(|k| env_or_process(cfg, k), &cfg.data_dir)
}

fn env_or_process(cfg: &Config, key: &str) -> Option<String> {
    cfg.env.get(key).map(str::to_string).or_else(|| std::env::var(key).ok())
}

/// OCR tools, cached for 60 s (PATH search costs a few file accesses).
fn ocr_tools(tesseract: Option<String>, pdfium: Option<String>) -> OcrTools {
    static CACHE: OnceLock<Mutex<Option<(Instant, OcrTools)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(g) = cache.lock()
        && let Some((at, t)) = *g
        && at.elapsed() < OCR_CACHE_TTL
    {
        return t;
    }
    let t = OcrTools { tesseract: find_tesseract(tesseract.as_deref()), pdfium: find_pdfium(pdfium.as_deref()) };
    if let Ok(mut g) = cache.lock() {
        *g = Some((Instant::now(), t));
    }
    t
}

fn ocr_check(tools: Option<OcrTools>) -> Value {
    let Some(t) = tools else {
        return json!({"status": ERROR, "error": timed_out("OCR"), "tesseract": null, "pdfium": null});
    };
    let (st, err) = match (t.tesseract, t.pdfium) {
        (true, true) => (OK, None),
        (false, _) => (WARNING, Some("Tesseract not found; OCR is unavailable.")),
        (true, false) => (WARNING, Some("PDFium not found; PDF pages cannot be rendered for OCR.")),
    };
    json!({"status": st, "error": err, "tesseract": t.tesseract, "pdfium": t.pdfium})
}

fn storage_check(r: &Option<std::result::Result<(u64, u64), String>>) -> Value {
    match r {
        Some(Ok((total, available))) => {
            let low = *total > 0 && (*available as f64) < (*total as f64) * STORAGE_WARN_FRACTION;
            let (st, err) = if low { (WARNING, Some("Less than 5 % of the media storage is free.")) } else { (OK, None) };
            json!({"status": st, "error": err, "total": total, "available": available})
        }
        Some(Err(e)) => {
            tracing::error!("System status: reading the media storage failed: {e}");
            json!({"status": ERROR, "error": "Error reading the media storage, check logs for more detail.", "total": 0, "available": 0})
        }
        None => json!({"status": ERROR, "error": timed_out("Storage"), "total": 0, "available": 0}),
    }
}

fn server_os_cached() -> &'static str {
    static OS: OnceLock<String> = OnceLock::new();
    OS.get_or_init(sysinfo::server_os)
}

// ------------------------------------------------------------------------------------------
// Route
// ------------------------------------------------------------------------------------------

/// (`redis_url`, `redis_status`, `redis_error`, cache check): the fields are named like upstream,
/// but describe the PaperRust cache (process-local or `paperrust_cache`).
async fn cache_status(state: &AppState) -> (Value, &'static str, Value, Value) {
    match state.cache().backend() {
        // process-local: no connection
        pl_core::cache::Backend::Local => {
            (json!("memory://local"), OK, Value::Null, json!({"backend": "local", "status": OK, "error": null, "latency_ms": 0.0}))
        }
        pl_core::cache::Backend::Postgres => {
            let url = json!(format!("postgresql://{}/paperrust_cache", state.config().db.name));
            let t0 = Instant::now();
            let r = bounded(sqlx::query("SELECT 1 FROM paperrust_cache LIMIT 1").fetch_optional(state.db())).await;
            let el = ms(t0.elapsed());
            let err = match r {
                Some(Ok(_)) => None,
                Some(Err(e)) => {
                    tracing::error!("System status detected a possible problem with the shared cache (paperrust_cache): {e}");
                    Some(json!("Error reading the shared cache, check logs for more detail."))
                }
                None => Some(json!(timed_out("Cache"))),
            };
            match err {
                None => (url, OK, Value::Null, json!({"backend": "postgres", "status": OK, "error": null, "latency_ms": el})),
                Some(e) => (url, ERROR, e.clone(), json!({"backend": "postgres", "status": ERROR, "error": e, "latency_ms": el})),
            }
        }
    }
}

/// `GET /api/status/`
pub async fn get(State(state): State<AppState>, _ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    if !perms::has_system_status_permission(Some(auth.user())) {
        return Err(insufficient_permissions());
    }
    let started = Instant::now();
    let cfg = state.config();
    let db = state.db();

    let install_type = sysinfo::install_type(cfg.env.get("KUBERNETES_SERVICE_HOST"), cfg.env.get("PNGX_CONTAINERIZED"));

    // --- all checks in parallel --------------------------------------------------------------
    let media_root = cfg.media_root.clone();
    let index_dir = cfg.index_dir.clone();
    // C3: what is checked is the model that PaperRust actually loads (pl-consumer/pl-cli:
    // PAPERRUST_CLASSIFIER_MODEL_FILE or DATA_DIR/paperrust_classifier.model), not the
    // upstream pickle from PAPERLESS_MODEL_FILE (different format, PaperRust does not read it).
    let model_path = classifier_model_path(cfg);
    let tz = *state.tz();
    let tess = env_or_process(cfg, "PAPERRUST_TESSERACT_PATH");
    let pdfium = env_or_process(cfg, "PAPERRUST_PDFIUM_PATH");

    let (database, cache_redis, worker, tasks, storage, index, model, ocr) = tokio::join!(
        database_status(db, &cfg.db.name),
        cache_status(&state),
        worker_status_pg(db, &cfg.db.name),
        bounded(task_info(db)),
        blocking(move || sysinfo::storage(&media_root)),
        blocking(move || index_status(&index_dir, &tz)),
        blocking(move || model_file(&model_path)),
        blocking(move || ocr_tools(tess, pdfium)),
    );
    let (database, db_check) = database;
    let (redis_url, redis_status, redis_error, cache_check) = cache_redis;
    let (celery_active, celery_url, celery_error, worker_check) = worker;

    let (index_status, index_last_modified, index_error) = index.unwrap_or_else(|| (json!(ERROR), Value::Null, json!(timed_out("Index"))));

    let tasks_err = match &tasks {
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            tracing::error!("System status: reading task results failed: {e}");
            Some("Error reading task results, check logs for more detail.".to_string())
        }
        None => Some(timed_out("Task results")),
    };
    let info = match tasks {
        Some(Ok(i)) => i,
        _ => TaskInfo::default(),
    };
    let task_or_err = |t: Option<&LastTask>, missing: &str| match &tasks_err {
        Some(e) => (json!(ERROR), Value::Null, json!(e)),
        None => task_check(t, missing),
    };
    let (classifier_status, classifier_last_trained, classifier_error) =
        task_or_err(info.classifier.as_ref(), "No classifier training tasks found");
    let (sanity_status, sanity_last_run, sanity_error) = task_or_err(info.sanity.as_ref(), "No sanity check tasks found");

    let ai_enabled = info.ai_enabled.unwrap_or(cfg.ai_enabled);
    let embedding = nonempty(info.embedding_backend.clone()).or_else(|| env_embedding_backend(&cfg.env));
    let llm_backend = nonempty(info.llm_backend.clone()).or_else(|| env_llm_backend(&cfg.env));
    let llm_enabled = ai_enabled && embedding.is_some();
    let (llm_status, llm_last, llm_error) = if let Some(e) = &tasks_err {
        (json!(ERROR), Value::Null, json!(e))
    } else if !llm_enabled {
        (json!(DISABLED), Value::Null, Value::Null)
    } else {
        match &info.llm_index {
            None => (json!(WARNING), Value::Null, json!("No LLM index update tasks found")),
            Some(t) if t.status == "failure" => (json!(ERROR), raw_dt(t.date_done), error_message(t)),
            Some(t) => (json!(OK), raw_dt(t.date_done), Value::Null),
        }
    };
    let [total, pending, success, failure] = info.summary;
    let summary = json!({
        "days": TASK_SUMMARY_DAYS,
        "total_count": total,
        "pending_count": pending,
        "success_count": success,
        "failure_count": failure,
    });

    let (total_bytes, available_bytes) = match &storage {
        Some(Ok(v)) => *v,
        _ => (0, 0),
    };

    let mut tasks = Map::new();
    tasks.insert("redis_url".into(), redis_url);
    tasks.insert("redis_status".into(), json!(redis_status));
    tasks.insert("redis_error".into(), redis_error);
    tasks.insert("celery_status".into(), json!(celery_active));
    tasks.insert("celery_url".into(), celery_url);
    tasks.insert("celery_error".into(), celery_error);
    tasks.insert("index_status".into(), index_status.clone());
    tasks.insert("index_last_modified".into(), index_last_modified.clone());
    tasks.insert("index_error".into(), index_error.clone());
    tasks.insert("classifier_status".into(), classifier_status.clone());
    tasks.insert("classifier_last_trained".into(), classifier_last_trained.clone());
    tasks.insert("classifier_error".into(), classifier_error.clone());
    tasks.insert("sanity_check_status".into(), sanity_status.clone());
    tasks.insert("sanity_check_last_run".into(), sanity_last_run.clone());
    tasks.insert("sanity_check_error".into(), sanity_error.clone());
    tasks.insert("llmindex_status".into(), llm_status.clone());
    tasks.insert("llmindex_last_modified".into(), llm_last);
    tasks.insert("llmindex_error".into(), llm_error);
    tasks.insert("summary".into(), summary);

    // --- PaperRust sub-object -----------------------------------------------------------------
    let (model_present, model_modified) = model.unwrap_or((false, None));
    let ai_check = {
        let (st, err) = if !ai_enabled {
            (DISABLED, None)
        } else if llm_backend.is_none() {
            (WARNING, Some("AI is enabled but no LLM backend is configured."))
        } else {
            (OK, None)
        };
        json!({
            "status": st,
            "error": err,
            "enabled": ai_enabled,
            "llm_backend": llm_backend,
            "embedding_backend": embedding,
            "llmindex_status": llm_status,
        })
    };
    let mut checks = Map::new();
    checks.insert("database".into(), db_check);
    checks.insert("worker".into(), worker_check);
    checks.insert("cache".into(), cache_check);
    checks.insert("index".into(), json!({"status": index_status, "error": index_error, "last_modified": index_last_modified}));
    checks.insert(
        "classifier".into(),
        json!({
            "status": classifier_status,
            "error": classifier_error,
            "last_trained": classifier_last_trained,
            "model_file_present": model_present,
            "model_file_modified": raw_dt(model_modified),
        }),
    );
    checks.insert("sanity_check".into(), json!({"status": sanity_status, "error": sanity_error, "last_run": sanity_last_run}));
    checks.insert("ocr".into(), ocr_check(ocr));
    checks.insert("ai".into(), ai_check);
    checks.insert("storage".into(), storage_check(&storage));

    let mut pr = Map::new();
    pr.insert("version".into(), json!(PAPERRUST_VERSION));
    pr.insert("version_string".into(), json!(format!("{PRODUCT_NAME} {PAPERRUST_VERSION}")));
    pr.insert("upstream_version".into(), json!(UPSTREAM_VERSION));
    // fixed since 2026-11-14 (own queue, no forwarding to Django); keys stay
    // for tools that read them
    pr.insert("queue_backend".into(), json!("postgres"));
    pr.insert("django_proxy".into(), json!(false));
    pr.insert("checks".into(), Value::Object(checks));
    pr.insert("elapsed_ms".into(), ms(started.elapsed()));

    let mut out = Map::new();
    // UI: comparison with environment.version → compatibility level, not PaperRust version
    out.insert("pngx_version".into(), json!(UPSTREAM_VERSION));
    out.insert("server_os".into(), json!(server_os_cached()));
    out.insert("install_type".into(), json!(install_type));
    out.insert("storage".into(), json!({"total": total_bytes, "available": available_bytes}));
    out.insert("database".into(), database);
    out.insert("tasks".into(), Value::Object(tasks));
    out.insert("paperrust".into(), Value::Object(pr));
    Ok(Drf::ok(Value::Object(out)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C3: status checks the model that PaperRust loads, not PAPERLESS_MODEL_FILE.
    #[test]
    fn classifier_model_path_is_the_paperrust_model() {
        let cfg = |pairs: &[(&str, &str)]| {
            let mut v = vec![("PAPERLESS_SECRET_KEY", "k"), ("PAPERLESS_DATA_DIR", "/srv/data")];
            v.extend_from_slice(pairs);
            Config::from_env_map(pl_core::config::Env::from_pairs(v)).unwrap()
        };
        let c = cfg(&[("PAPERLESS_MODEL_FILE", "/srv/data/classification_model.pickle")]);
        assert_eq!(classifier_model_path(&c), c.data_dir.join("paperrust_classifier.model"));
        let c = cfg(&[("PAPERRUST_CLASSIFIER_MODEL_FILE", "/models/m.bin")]);
        assert_eq!(classifier_model_path(&c), std::path::PathBuf::from("/models/m.bin"));
    }

    #[tokio::test]
    async fn bounded_cuts_off() {
        let start = Instant::now();
        let r = bounded(tokio::time::sleep(Duration::from_secs(10))).await;
        assert!(r.is_none());
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(bounded(async { 5 }).await, Some(5));
    }

    #[test]
    fn squash_handling() {
        // fresh DB: squashed nodes present, replaced ones not
        let nodes = graph_nodes(&[]);
        assert!(nodes.contains(&"documents.0001_squashed".to_string()));
        assert!(!nodes.iter().any(|n| n == "documents.0001_initial"));
        assert!(nodes.contains(&"paperless.0001_initial".to_string()));
    }

    #[test]
    fn checks() {
        assert_eq!(task_check(None, "No sanity check tasks found").0, "WARNING");
        let t = LastTask { status: "failure".into(), date_done: None, result_data: Some(json!({"error_message": "x"})) };
        let (s, _, e) = task_check(Some(&t), "");
        assert_eq!((s, e), (json!("ERROR"), json!("x")));
    }

    #[test]
    fn worker_verdicts() {
        let idle_connected = QueueStats { listeners: 1, ..Default::default() };
        assert_eq!(worker_verdict(&idle_connected), (OK, None));
        let none = QueueStats::default();
        assert_eq!(worker_verdict(&none).0, WARNING);
        let busy = QueueStats { running: 2, ..Default::default() };
        assert_eq!(worker_verdict(&busy).0, OK);
        let lost = QueueStats { running: 1, stale: 1, listeners: 1, ..Default::default() };
        assert!(worker_verdict(&lost).1.unwrap().contains("stopped responding"));
        let waiting = QueueStats { listeners: 1, queued: 3, overdue: 2, ..Default::default() };
        assert!(worker_verdict(&waiting).1.unwrap().contains("waiting"));
        let polling = QueueStats { fresh_heartbeats: 1, ..Default::default() };
        assert_eq!(worker_verdict(&polling).0, OK);
    }

    #[test]
    fn ocr_and_storage_checks() {
        let both = OcrTools { tesseract: true, pdfium: true };
        assert_eq!(ocr_check(Some(both))["status"], OK);
        let no_tess = OcrTools { tesseract: false, pdfium: true };
        assert_eq!(ocr_check(Some(no_tess))["status"], WARNING);
        assert_eq!(ocr_check(None)["status"], ERROR);
        assert_eq!(storage_check(&Some(Ok((100, 50))))["status"], OK);
        assert_eq!(storage_check(&Some(Ok((100, 1))))["status"], WARNING);
        assert_eq!(storage_check(&Some(Err("x".into())))["status"], ERROR);
        assert_eq!(storage_check(&None)["status"], ERROR);
    }

    #[test]
    fn tool_lookup_configured_paths() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(pdfium_name());
        std::fs::write(&f, b"x").unwrap();
        assert!(find_pdfium(Some(dir.path().to_str().unwrap())));
        assert!(find_pdfium(Some(f.to_str().unwrap())));
        assert!(!find_pdfium(Some(dir.path().join("nope").to_str().unwrap())));
        let t = dir.path().join(exe_name("tesseract"));
        std::fs::write(&t, b"x").unwrap();
        assert!(find_tesseract(Some(t.to_str().unwrap())));
        assert!(!find_tesseract(Some(dir.path().join("missing.exe").to_str().unwrap())));
    }
}
