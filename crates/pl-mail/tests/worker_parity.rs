//! Mail fetch end to end against local fake IMAP servers and own DB copies; parity with the
//! unmodified upstream `MailAccountHandler` (Celery eager, own DB `pl_w6_test_up`).
//!
//! Run (reference PostgreSQL 127.0.0.1:54329 must be running; re-creates `pl_w6_test` and
//! `pl_w6_test_up` from `paperless_snapshot`):
//! ```text
//! PL_W6_TEST_DB=1 cargo test -p pl-mail -j 1 --test worker_parity -- --ignored --nocapture
//! ```
//! `PL_W6_SKIP_UPSTREAM=1` skips the Python run (Rust flow + checks only).

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_core::config::Env;
use pl_mail::fetch::{self, MailWorker, MailWorkerConfig};
use pl_queue::{Handlers, PgQueue, QueueConfig, TaskContext, TaskError, Worker};
use serde_json::{Value, json};
use sha2::Digest;
use sqlx::{PgPool, Row};
use support::fake_imap;

const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn db_url(name: &str) -> String {
    format!("postgres://paperless:paperless@127.0.0.1:54329/{name}")
}

async fn recreate_db(name: &str) {
    assert!(name.starts_with("pl_w6_test"), "refusing {name}");
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    admin.close().await;
}

/// Same starting point in both DBs: seed rules off, two accounts (standard / Gmail), rules.
async fn scenario(pool: &PgPool, port_std: u16, port_gmail: u16) {
    let sql = format!(
        r#"
UPDATE paperless_mail_mailrule SET enabled = false;
INSERT INTO paperless_mail_mailaccount (id, name, imap_server, imap_port, imap_security, username, password, character_set, is_token, owner_id, account_type, refresh_token, expiration)
VALUES (101, 'w6-standard', '127.0.0.1', {port_std}, 1, 'user', 'pass', 'UTF-8', false, 2, 1, NULL, NULL),
       (102, 'w6-gmail', '127.0.0.1', {port_gmail}, 1, 'user', 'pass', 'UTF-8', false, 2, 1, NULL, NULL);
INSERT INTO paperless_mail_mailrule (id, name, "order", account_id, enabled, folder, filter_from, filter_to, filter_subject, filter_body,
  filter_attachment_filename_include, filter_attachment_filename_exclude, maximum_age, action, action_parameter,
  assign_title_from, assign_correspondent_from, assign_correspondent_id, assign_document_type_id, assign_owner_from_rule,
  attachment_type, consumption_scope, pdf_layout, owner_id, stop_processing)
VALUES
 (201, 'attachments', 1, 101, true, 'INBOX', NULL, NULL, NULL, NULL, NULL, '*.asc', 0, 3, '', 1, 3, NULL, 1, true, 1, 1, 0, 3, false),
 (202, 'eml-invoices', 2, 101, true, 'INBOX', NULL, NULL, 'Invoice', NULL, NULL, NULL, 0, 4, '', 3, 2, NULL, NULL, false, 1, 2, 0, 2, false),
 (203, 'archive-everything', 3, 101, true, 'Archiv', NULL, NULL, NULL, NULL, '*.pdf,*.png', NULL, 0, 2, 'Done', 2, 4, 1, NULL, false, 2, 3, 0, NULL, false),
 (204, 'delete-grace', 4, 101, true, 'INBOX', 'grace', NULL, NULL, NULL, NULL, NULL, 3650, 1, '', 1, 1, NULL, NULL, false, 1, 1, 0, NULL, false),
 (205, 'disabled', 5, 101, false, 'INBOX', NULL, NULL, NULL, NULL, NULL, NULL, 0, 3, '', 1, 1, NULL, NULL, false, 1, 1, 0, NULL, false),
 (206, 'missing-folder', 6, 101, true, 'Nope', NULL, NULL, NULL, NULL, NULL, NULL, 0, 3, '', 1, 1, NULL, NULL, false, 1, 1, 0, NULL, false),
 (207, 'tag-keyword', 7, 101, true, 'INBOX', NULL, NULL, NULL, NULL, NULL, NULL, 0, 5, 'paperless', 1, 1, NULL, NULL, false, 1, 1, 0, NULL, true),
 (211, 'gmail-label', 1, 102, true, 'INBOX', NULL, NULL, 'Invoice', NULL, NULL, NULL, 0, 5, 'paperless', 1, 1, NULL, NULL, false, 1, 1, 0, NULL, false),
 (212, 'gmail-apple', 2, 102, true, 'INBOX', NULL, NULL, NULL, NULL, NULL, NULL, 0, 5, 'apple:green', 1, 2, NULL, NULL, false, 1, 1, 0, NULL, false);
INSERT INTO paperless_mail_mailrule_assign_tags (mailrule_id, tag_id) VALUES (201, 1), (201, 2);
"#
    );
    sqlx::raw_sql(&sql).execute(pool).await.expect("scenario");
}

fn mail(name: &str) -> Vec<u8> {
    std::fs::read(repo().join("tests/parity/pl-mail-worker/mails").join(name)).expect("mail")
}

/// Mailbox content (identical for both runs).
fn mailboxes() -> (fake_imap::Shared, fake_imap::Shared) {
    let mut std = fake_imap::State::default();
    std.add_folder("INBOX", 4242);
    std.add_folder("Archiv", 77);
    std.add_folder("Done", 78);
    for (n, flags) in [
        ("01_plain.eml", &[][..]),
        ("02_pdf_attachment.eml", &[]),
        ("03_long_headers.eml", &[]),
        ("04_8bit_headers.eml", &[]),
        ("05_nested_inline.eml", &[]),
        ("07_lf_only_no_close.eml", &[]),
        ("08_no_date_bad_from.eml", &["\\Seen"]),
        ("10_inline_only.eml", &[]),
        ("11_invoice_reminder.eml", &[]),
    ] {
        std.append("INBOX", mail(n), flags);
    }
    for n in ["06_forwarded.eml", "09_signed.eml", "05_nested_inline.eml"] {
        std.append("Archiv", mail(n), &[]);
    }
    let mut g = fake_imap::State::default();
    g.add_folder("INBOX", 9);
    for n in ["02_pdf_attachment.eml", "05_nested_inline.eml", "10_inline_only.eml", "11_invoice_reminder.eml"] {
        g.append("INBOX", mail(n), &[]);
    }
    (Arc::new(Mutex::new(std)), Arc::new(Mutex::new(g)))
}

fn gmail_cfg() -> fake_imap::Config {
    fake_imap::Config { capabilities: "IMAP4rev1 AUTH=PLAIN X-GM-EXT-1".into(), ..Default::default() }
}

#[derive(Debug, Default, Clone)]
struct Observed {
    consumed: Vec<Value>,
    processed: Vec<Value>,
    correspondents: Vec<Value>,
    audit: Vec<Value>,
    imap_main: Vec<Vec<String>>,
    imap_actions: Vec<Vec<String>>,
    mailbox: Value,
    results: Vec<Value>,
}

/// Bring raw data from DB/fake server into a comparable form.
async fn observe(pool: &PgPool, std: &fake_imap::Shared, g: &fake_imap::Shared, consumed: Vec<Value>, results: Vec<Value>) -> Observed {
    let rows = sqlx::query(
        "SELECT owner_id, rule_id, folder, uid, uid_validity, subject, received, status, error IS NOT NULL AS has_error \
         FROM paperless_mail_processedmail WHERE rule_id >= 200 ORDER BY rule_id, folder, uid, status",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let processed = rows
        .iter()
        .map(|r| {
            json!({
                "owner": r.get::<Option<i32>, _>("owner_id"),
                "rule": r.get::<i32, _>("rule_id"),
                "folder": r.get::<String, _>("folder"),
                "uid": r.get::<String, _>("uid"),
                "uid_validity": r.get::<Option<String>, _>("uid_validity"),
                "subject": r.get::<String, _>("subject"),
                "received": r.get::<chrono::DateTime<chrono::Utc>, _>("received").to_rfc3339(),
                "status": r.get::<String, _>("status"),
                "has_error": r.get::<bool, _>("has_error"),
            })
        })
        .collect();
    let rows = sqlx::query("SELECT id, name, match, matching_algorithm, is_insensitive, owner_id FROM documents_correspondent WHERE id > 12 ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap();
    let correspondents = rows
        .iter()
        .map(|r| {
            json!([r.get::<i32, _>("id"), r.get::<String, _>("name"), r.get::<String, _>("match"), r.get::<i16, _>("matching_algorithm"), r.get::<bool, _>("is_insensitive"), r.get::<Option<i32>, _>("owner_id")])
        })
        .collect();
    let rows = sqlx::query(
        "SELECT l.object_id, l.object_repr, l.action, l.changes::text AS changes FROM auditlog_logentry l JOIN django_content_type c ON c.id = l.content_type_id \
         WHERE c.model = 'correspondent' AND l.object_id > 12 ORDER BY l.id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let audit = rows
        .iter()
        .map(|r| {
            let ch: Value = serde_json::from_str(&r.get::<String, _>("changes")).unwrap_or(Value::Null);
            json!([r.get::<Option<i64>, _>("object_id"), r.get::<String, _>("object_repr"), r.get::<i16, _>("action"), ch])
        })
        .collect();
    let mut imap_main = Vec::new();
    let mut imap_actions = Vec::new();
    for st in [std, g] {
        let s = st.lock().unwrap();
        let mut per: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (c, l) in &s.log {
            per.entry(*c).or_default().push(normalize_cmd(l));
        }
        for (_, cmds) in per {
            if cmds.iter().any(|c| c.starts_with("UID SEARCH")) {
                imap_main.push(cmds);
            } else {
                imap_actions.push(cmds);
            }
        }
    }
    imap_actions.sort();
    let mailbox = json!({"std": std.lock().unwrap().summary(), "gmail": g.lock().unwrap().summary()});
    let mut consumed = consumed;
    consumed.sort_by_key(|v| v.to_string());
    Observed { consumed, processed, correspondents, audit, imap_main, imap_actions, mailbox, results }
}

/// imaplib writes `UID fetch` no differently than we do – but the date in SINCE depends on the day.
fn normalize_cmd(l: &str) -> String {
    l.to_string()
}

fn stub_consume(recorded: Arc<Mutex<Vec<Value>>>) -> impl pl_queue::TaskHandler + 'static {
    move |ctx: TaskContext| {
        let recorded = recorded.clone();
        async move {
            let (doc, ov) = pl_consumer::task::parse_kwargs(&ctx.kwargs).map_err(|e| TaskError::fail("ValueError", e))?;
            let data = std::fs::read(&doc.original_file).map_err(|e| TaskError::fail("OSError", e.to_string()))?;
            let name = doc.original_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // .eml: align line endings – PaperRust writes CRLF since upstream fix a1d76f3a1 (#14745),
            // the v3.4.2 reference LF (both sides normalised the same way, run_upstream.py too;
            // CRLF variant itself checked in tests/worker_messages.rs).
            let data = if name.ends_with(".eml") { crlf_to_lf(&data) } else { data };
            let ov = ov.unwrap_or_default();
            let mut o = serde_json::Map::new();
            if let Value::Object(m) = serde_json::to_value(&ov).unwrap() {
                for (k, v) in m {
                    let keep = match &v {
                        Value::Null => false,
                        Value::Bool(b) => *b,
                        _ => true,
                    };
                    if keep {
                        o.insert(k, v);
                    }
                }
            }
            recorded.lock().unwrap().push(json!({
                "source": 3,
                "mailrule_id": doc.mailrule_id,
                "mime_type": doc.mime_type,
                "file_name": if name.ends_with(".eml") { "<tmp>.eml".to_string() } else { name },
                "sha256": hex(&sha2::Sha256::digest(&data)),
                "overrides": Value::Object(o),
            }));
            Ok(json!("recorded"))
        }
    }
}

fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn wait_queue_idle(pool: &PgPool) {
    for _ in 0..600 {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM paperrust_task_queue WHERE state IN ('queued', 'running')").fetch_one(pool).await.unwrap();
        if n == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("queue did not drain");
}

/// Queue like Celery `task_always_eager` (upstream reference run): chord head immediately,
/// then `apply_mail_action`; error in the head → exception to the caller (errback does not run).
struct EagerQueue {
    worker: std::sync::OnceLock<Arc<MailWorker>>,
    consume: Arc<dyn pl_queue::TaskHandler>,
}

type Bf<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, pl_queue::QueueError>> + Send + 'a>>;

impl pl_queue::TaskQueue for EagerQueue {
    fn enqueue<'a>(&'a self, _t: &'a str, _k: serde_json::Map<String, Value>, _o: pl_queue::EnqueueOptions) -> Bf<'a, uuid::Uuid> {
        Box::pin(async { Err(pl_queue::QueueError::Unsupported("eager enqueue".into())) })
    }
    fn status(&self, _id: uuid::Uuid) -> Bf<'_, Option<pl_queue::TaskInfo>> {
        Box::pin(async { Ok(None) })
    }
    fn enqueue_chord(&self, header: Vec<pl_queue::Signature>, body: pl_queue::Signature, _on_error: Option<pl_queue::Signature>) -> Bf<'_, uuid::Uuid> {
        Box::pin(async move {
            for sig in header {
                let ctx = TaskContext {
                    task_id: "eager".into(),
                    task_name: sig.task.clone(),
                    args: sig.args.clone(),
                    kwargs: sig.kwargs.clone(),
                    headers: Default::default(),
                    retries: 0,
                    events: Default::default(),
                    threads_per_worker: 1,
                };
                if let Err(e) = self.consume.call(ctx).await {
                    return Err(pl_queue::QueueError::Invalid(format!("{}: {}", e.error_type, e.message)));
                }
            }
            let w = self.worker.get().expect("worker").clone();
            let _ = fetch::apply_mail_action(&w, &body.kwargs).await;
            Ok(uuid::Uuid::nil())
        })
    }
}

async fn run_rust(runs: usize, eager: bool) -> (Observed, PgPool) {
    let db = "pl_w6_test";
    recreate_db(db).await;
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(8).connect(&db_url(db)).await.unwrap();
    pl_queue::migrations::apply(&pool).await.unwrap();
    let (std, g) = mailboxes();
    let p1 = fake_imap::start(std.clone(), fake_imap::Config::default()).await;
    let p2 = fake_imap::start(g.clone(), gmail_cfg()).await;
    scenario(&pool, p1, p2).await;

    let core = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-w6-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
    ]))
    .unwrap();
    let scratch = tempdir();
    let mut cfg = MailWorkerConfig::from_core(&core);
    cfg.scratch_dir = scratch.clone();
    cfg.connect.allow_internal_hosts = true;
    cfg.connect.timeout = Duration::from_secs(10);
    let qcfg = QueueConfig::from_env(&Env::from_pairs([
        ("PAPERRUST_QUEUE_BACKEND", "postgres"),
        ("PAPERRUST_QUEUE_POLL_INTERVAL", "0.05"),
        ("PAPERLESS_TASK_WORKERS", "1"),
    ]))
    .unwrap();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let eager_q = Arc::new(EagerQueue { worker: std::sync::OnceLock::new(), consume: Arc::new(stub_consume(recorded.clone())) });
    let queue: Arc<dyn pl_queue::TaskQueue> =
        if eager { eager_q.clone() } else { Arc::new(PgQueue::new(pool.clone(), qcfg.max_deliveries)) };
    let worker = Arc::new(MailWorker::new(pool.clone(), queue.clone(), cfg));
    let _ = eager_q.worker.set(worker.clone());
    let webhooks = Arc::new(pl_mail::effects::webhook::WebhookSettings::from_env(&Env::from_pairs::<&str, &str>([])).unwrap());
    let handlers = fetch::register_mail_handlers(Handlers::new(), worker.clone(), webhooks)
        .register(pl_consumer::task::TASK_NAME, stub_consume(recorded.clone()));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let w = Worker::new(PgQueue::new(pool.clone(), qcfg.max_deliveries), handlers, qcfg, Default::default());
    let jh = tokio::spawn(w.run(rx));
    let mut results = Vec::new();
    for _ in 0..runs {
        let r = fetch::process_mail_accounts(&worker, None).await.unwrap();
        results.push(Value::String(r));
        wait_queue_idle(&pool).await;
    }
    let _ = tx.send(true);
    let _ = jh.await;
    let consumed = recorded.lock().unwrap().clone();
    let obs = observe(&pool, &std, &g, consumed, results).await;
    let _ = std::fs::remove_dir_all(&scratch);
    (obs, pool)
}

fn tempdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("pl-w6-{}", uuid_like()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn uuid_like() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

async fn run_upstream(runs: usize) -> Observed {
    let db = "pl_w6_test_up";
    recreate_db(db).await;
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&db_url(db)).await.unwrap();
    let (std, g) = mailboxes();
    let p1 = fake_imap::start(std.clone(), fake_imap::Config::default()).await;
    let p2 = fake_imap::start(g.clone(), gmail_cfg()).await;
    scenario(&pool, p1, p2).await;
    let out = std::env::temp_dir().join(format!("pl-w6-up-{}.json", uuid_like()));
    let script = repo().join("tests/parity/pl-mail-worker/run_upstream.sh");
    let status = tokio::process::Command::new("bash")
        .arg(&script)
        .env("PL_W6_OUT", &out)
        .env("PL_W6_RUNS", runs.to_string())
        .env("PL_W6_UP_DB", db)
        .status()
        .await
        .expect("spawn bash");
    assert!(status.success(), "upstream run failed");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let _ = std::fs::remove_file(&out);
    let consumed: Vec<Value> = v["consumed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            // Python overrides: only non-None, without False flags and without "_" fields
            let mut c = c.clone();
            if let Some(Value::Object(o)) = c.get_mut("overrides") {
                o.retain(|k, v| !k.starts_with('_') && !matches!(v, Value::Bool(false)));
            }
            c
        })
        .collect();
    let results = v["results"].as_array().cloned().unwrap_or_default();
    observe(&pool, &std, &g, consumed, results).await
}

fn diff(label: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    if a != b {
        out.push(format!("{label}:\n  upstream: {a}\n  rust:     {b}"));
    }
}

/// Reference on Windows: libmagic cannot open paths with non-ASCII characters
/// (`ConsumableDocument.mime_type` = error text). These values are not compared.
fn normalize(up: &mut Observed, rust: &mut Observed) {
    for c in &mut up.consumed {
        if c["mime_type"].as_str().is_some_and(|m| m.starts_with("cannot open")) {
            c["mime_type"] = json!("<libmagic-windows-path-error>");
        }
    }
    up.consumed.sort_by_key(|v| v.to_string());
    for r in &mut rust.consumed {
        let key = |v: &Value| (v["mailrule_id"].clone(), v["file_name"].clone(), v["sha256"].clone());
        if up.consumed.iter().any(|u| key(u) == key(r) && u["mime_type"] == json!("<libmagic-windows-path-error>")) {
            r["mime_type"] = json!("<libmagic-windows-path-error>");
        }
    }
    rust.consumed.sort_by_key(|v| v.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "braucht die Referenz-PostgreSQL (eigene DB-Kopien pl_w6_test*) und reference/venv"]
async fn mail_worker_parity() {
    if std::env::var("PL_W6_TEST_DB").is_err() {
        eprintln!("PL_W6_TEST_DB nicht gesetzt – übersprungen");
        return;
    }
    let runs = 2;
    let (mut rust, _pool) = run_rust(runs, true).await;
    assert!(rust.results[0].as_str().is_some_and(|s| s.starts_with("Added")));
    let mut up = run_upstream(runs).await;
    normalize(&mut up, &mut rust);
    let mut d = Vec::new();
    diff("results", &Value::from(up.results.clone()), &Value::from(rust.results.clone()), &mut d);
    diff("consumed", &Value::from(up.consumed.clone()), &Value::from(rust.consumed.clone()), &mut d);
    diff("processed_mail", &Value::from(up.processed.clone()), &Value::from(rust.processed.clone()), &mut d);
    diff("correspondents", &Value::from(up.correspondents.clone()), &Value::from(rust.correspondents.clone()), &mut d);
    diff("correspondent_audit", &Value::from(up.audit.clone()), &Value::from(rust.audit.clone()), &mut d);
    diff("imap_main", &json!(up.imap_main), &json!(rust.imap_main), &mut d);
    diff("imap_actions", &json!(up.imap_actions), &json!(rust.imap_actions), &mut d);
    diff("mailbox_after", &up.mailbox, &rust.mailbox, &mut d);
    let n = 8;
    let report = repo().join("tests/parity/pl-mail-worker/out");
    let _ = std::fs::create_dir_all(&report);
    let text = if d.is_empty() { format!("{n} comparisons, 0 differences\n") } else { d.join("\n\n") };
    let _ = std::fs::write(report.join("last_run.txt"), text);
    let obs = json!({
        "results": rust.results, "consumed": rust.consumed, "processed": rust.processed,
        "correspondents": rust.correspondents, "audit": rust.audit, "imap_main": rust.imap_main,
        "imap_actions": rust.imap_actions, "mailbox": rust.mailbox,
    });
    let _ = std::fs::write(report.join("rust_observed.json"), serde_json::to_string_pretty(&obs).unwrap());
    assert!(d.is_empty(), "{} of {n} comparisons differ:\n{}", d.len(), d.join("\n\n"));
    eprintln!(
        "mail worker parity: {n} comparisons, 0 differences ({} consumed files, {} ProcessedMail rows, {} action connections)",
        rust.consumed.len(),
        rust.processed.len(),
        rust.imap_actions.len()
    );
}

/// Same flow via the real PostgreSQL queue (chords, workers), plus the fetch lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "braucht die Referenz-PostgreSQL (eigene DB-Kopie pl_w6_test)"]
async fn mail_worker_queue_e2e() {
    if std::env::var("PL_W6_TEST_DB").is_err() {
        return;
    }
    let (o, pool) = run_rust(2, false).await;
    assert!(o.results[0].as_str().is_some_and(|s| s.starts_with("Added")), "{:?}", o.results);
    let (n_failed_jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM paperrust_task_queue WHERE state = 'failed' AND task_name = 'paperless_mail.mail.apply_mail_action'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let failed_rows = o.processed.iter().filter(|p| p["status"] == "FAILED").count() as i64;
    assert!(n_failed_jobs >= 1 && failed_rows >= n_failed_jobs, "failed rows {failed_rows}, failed jobs {n_failed_jobs}");
    let mb = &o.mailbox;
    assert!(mb["gmail"]["INBOX"].as_array().unwrap().iter().any(|m| m[2].as_array().unwrap().contains(&json!("paperless"))));
    assert!(mb["std"]["Done"].as_array().unwrap().len() >= 3);
    assert!(!mb["std"]["INBOX"].as_array().unwrap().iter().any(|m| m[0] == json!(6)), "deleted mail must be gone");
    // lock: second parallel run is skipped
    let mut lock = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)").bind(fetch::tasks::MAIL_FETCH_LOCK_ID).execute(&mut *lock).await.unwrap();
    let core = pl_core::Config::from_env_map(Env::from_pairs([(
        "PAPERLESS_SECRET_KEY",
        "pl-w6-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
    )]))
    .unwrap();
    let q: Arc<dyn pl_queue::TaskQueue> = Arc::new(PgQueue::new(pool.clone(), 3));
    let w = MailWorker::new(pool.clone(), q, MailWorkerConfig::from_core(&core));
    assert_eq!(fetch::process_mail_accounts(&w, None).await.unwrap(), fetch::tasks::SKIPPED);
    sqlx::query("SELECT pg_advisory_unlock($1)").bind(fetch::tasks::MAIL_FETCH_LOCK_ID).execute(&mut *lock).await.unwrap();
    eprintln!("queue e2e ok: {:?}, {} consumed, {} ProcessedMail", o.results, o.consumed.len(), o.processed.len());
}
