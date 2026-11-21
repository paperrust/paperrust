//! Equality of bundled follow-up ↔ single path (`bulk_update_documents`, PERF-SCALE §9).
//!
//! Two fresh copies of `paperless_snapshot` (reference data, 200 documents + 4 versions + 5 in the
//! trash) with identical additional setup ("document updated" workflows that only match from the
//! second workflow on and depend on each other, a storage path with deliberate
//! name collisions, a storage path with `tag_list`, tag change like `bulk_edit add_tag`) and
//! one copy of the reference media files each. A runs via the single path, B via the bundled
//! path; afterwards DB state (without `modified` timestamps), audit log, workflow runs,
//! enqueued tasks, file paths, file names and file contents must be identical.
//!
//! ```text
//! PAPERRUST_BULK_EQ_SERVER=postgres://paperless:paperless@127.0.0.1:54329 \
//!   cargo test -p pl-consumer -j 2 --test bulk_equal -- --ignored --nocapture
//! ```
//! Creates the DBs `pl_bulk_eq_a`/`pl_bulk_eq_b` (DROP … WITH (FORCE) beforehand) and leaves them
//! in place for inspection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use pl_consumer::native::{self, NativeParsing, RecordingProgress};
use pl_consumer::{Consumer, ConsumerSettings};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Reference media are in the main working tree (reference/ is not versioned).
fn reference_media() -> PathBuf {
    if let Ok(p) = std::env::var("PAPERRUST_BULK_EQ_MEDIA") {
        return PathBuf::from(p);
    }
    let local = repo().join("reference/data/media");
    if local.exists() {
        return local;
    }
    // Working tree under .claude/worktrees/<name> → main working tree three levels up
    repo().join("../../../reference/data/media")
}

#[derive(Default)]
struct CapturingQueue {
    jobs: Mutex<Vec<Value>>,
}

impl pl_queue::TaskQueue for CapturingQueue {
    fn enqueue<'a>(
        &'a self,
        task_name: &'a str,
        kwargs: Map<String, Value>,
        _opts: pl_queue::EnqueueOptions,
    ) -> BoxFuture<'a, Result<uuid::Uuid, pl_queue::QueueError>> {
        self.jobs.lock().unwrap().push(json!({"task": task_name, "kwargs": kwargs}));
        Box::pin(async { Ok(uuid::Uuid::new_v4()) })
    }
    fn status(&self, _task_id: uuid::Uuid) -> BoxFuture<'_, Result<Option<pl_queue::TaskInfo>, pl_queue::QueueError>> {
        Box::pin(async { Ok(None) })
    }
}

const TAG_ADJACENCY: &str = include_str!("../../pl-objects/migrations/0001_paperrust_tag_adjacency.sql");

const SETUP: &str = r#"
BEGIN;
UPDATE documents_workflow SET enabled = false;
INSERT INTO documents_tag (id, name, match, matching_algorithm, is_insensitive, color, is_inbox_tag, owner_id, tn_parent_id)
SELECT id, name, '', 0, true, '#a6cee3', false, NULL, NULL
FROM (VALUES (9501, 'eq-bulk'), (9502, 'eq-trigger'), (9503, 'eq-assigned'), (9504, 'eq-never'), (9505, 'eq-title')) AS t(id, name);
INSERT INTO documents_correspondent (id, name, match, matching_algorithm, is_insensitive, owner_id) VALUES
  (9501, 'EQ Corr', '', 0, true, NULL);
INSERT INTO documents_storagepath (id, name, match, matching_algorithm, is_insensitive, path, owner_id) VALUES
  (9501, 'eq collide', '', 0, true, 'eq/{{ document_type }}', NULL);
-- Speicherpfad mit Tags in der Vorlage (Tag-Änderung → neue Dateinamen)
UPDATE documents_storagepath SET path = path || '/{{ tag_list }}'
 WHERE id = (SELECT min(id) FROM documents_storagepath WHERE id < 9000);
-- Kollisionen: viele Dokumente → derselbe Name (Reihenfolge entscheidet über _01, _02 …)
UPDATE documents_document SET storage_path_id = 9501 WHERE id BETWEEN 40 AND 55 AND root_document_id IS NULL;
-- vorhandene Tags für den Workflow-Auslöser
INSERT INTO documents_document_tags (document_id, tag_id)
SELECT id, 9502 FROM documents_document WHERE id BETWEEN 20 AND 30 OR id BETWEEN 40 AND 43;
INSERT INTO documents_document_tags (document_id, tag_id)
SELECT id, 9505 FROM documents_document WHERE id IN (24, 25, 41);
-- Workflows „Dokument geändert“: W-never (order 1, passt nie), W-assign (order 2: Tag eq-trigger →
-- Korrespondent EQ Corr + Tag eq-assigned), W-chain (order 3: passt erst nach W-assign; entfernt
-- eq-trigger, setzt Dokumenttyp → bei Speicherpfad eq/{{ document_type }} neuer Dateiname mitten in
-- der Kette), W-title (order 4: nur Tag eq-title; Titel aus Vorlage = nicht „einfach“ → Einzelweg)
INSERT INTO documents_workflow (id, name, "order", enabled) VALUES
  (9501, 'EQ never', 1, true), (9502, 'EQ assign', 2, true), (9503, 'EQ chain', 3, true), (9504, 'EQ title', 4, true);
INSERT INTO documents_workflowtrigger (id, type, sources, match, matching_algorithm, is_insensitive,
  schedule_offset_days, schedule_is_recurring, schedule_recurring_interval_days, schedule_date_field, schedule_date_custom_field_id) VALUES
  (9501, 3, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL),
  (9502, 3, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL),
  (9503, 3, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL),
  (9504, 3, '1,2,3,4', '', 0, true, 0, false, 1, 'added', NULL);
INSERT INTO documents_workflowtrigger_filter_has_tags (workflowtrigger_id, tag_id) VALUES
  (9501, 9504), (9502, 9502), (9503, 9503), (9504, 9505);
INSERT INTO documents_workflow_triggers (workflow_id, workflowtrigger_id) VALUES (9501, 9501), (9502, 9502), (9503, 9503), (9504, 9504);
INSERT INTO documents_workflowaction (id, type, "order", assign_title, remove_all_tags, remove_all_document_types,
  remove_all_correspondents, remove_all_storage_paths, remove_all_owners, remove_all_permissions, remove_all_custom_fields,
  assign_correspondent_id, ai_create_missing, ai_overwrite_existing) VALUES
  (9501, 1, 0, 'never', false, false, false, false, false, false, false, NULL, false, false),
  (9502, 1, 0, NULL, false, false, false, false, false, false, false, 9501, false, false),
  (9503, 2, 0, NULL, false, false, false, false, false, false, false, NULL, false, false),
  (9504, 1, 0, 'EQ chained', false, false, false, false, false, false, false, NULL, false, false),
  (9505, 1, 1, NULL, false, false, false, false, false, false, false, NULL, false, false);
UPDATE documents_workflowaction SET assign_document_type_id = (SELECT min(id) FROM documents_documenttype) WHERE id = 9505;
INSERT INTO documents_workflowaction_assign_tags (workflowaction_id, tag_id) VALUES (9502, 9503);
INSERT INTO documents_workflowaction_remove_tags (workflowaction_id, tag_id) VALUES (9503, 9502);
INSERT INTO documents_workflow_actions (workflow_id, workflowaction_id) VALUES
  (9501, 9501), (9502, 9502), (9503, 9503), (9503, 9505), (9504, 9504);
-- wie bulk_edit add_tag (vor dem Nachlauf)
INSERT INTO documents_document_tags (document_id, tag_id)
SELECT id, 9501 FROM documents_document WHERE id BETWEEN 10 AND 60 OR id BETWEEN 190 AND 204;
COMMIT;
"#;

fn consumer(pool: PgPool, root: &Path, queue: Arc<CapturingQueue>) -> Consumer {
    let mut s = ConsumerSettings::with_root(root.join("media"), root.join("data"), root.join("scratch"), root.join("consume"));
    s.time_zone = chrono_tz::Europe::Zurich;
    s.templating.time_zone = chrono_tz::Europe::Zurich;
    s.audit_log_enabled = true;
    let mut ocr = pl_ocr::OcrConfig::defaults();
    ocr.mode = pl_ocr::config::Mode::parse("off").unwrap();
    let mut parsing = NativeParsing::new(ocr, s.time_zone);
    parsing.tesseract = None;
    let index_dir = s.data_dir.join("index");
    std::fs::create_dir_all(&index_dir).unwrap();
    let index = native::TantivyIndex::open(pl_indexer::IndexerConfig { index_dir, language: None }).unwrap();
    let dates = native::RegexDates::from_settings(&s, "eng");
    Consumer {
        pool,
        settings: Arc::new(s),
        parsing: Arc::new(parsing),
        dates: Arc::new(dates),
        classifier: Arc::new(native::NoClassifier),
        index: Arc::new(index),
        progress: Arc::new(RecordingProgress::default()),
        barcodes: Arc::new(native::NoBarcodes),
        pages: Arc::new(native::LopdfPages),
        effects: Arc::new(native::LoggingEffects),
        scripts: Arc::new(native::CommandScripts),
        queue: Some(queue),
        remote_ocr_by_default: false,
        events: Some(pl_queue::EventBus::new(4096)),
        caches: Arc::new(pl_consumer::signals::NoCaches),
        deleter: Arc::new(pl_consumer::workflows::NativeSoftDelete),
        vision: None,
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// Relative path → SHA-256 of all files under `root`.
fn tree(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, pl_consumer::compute_checksum(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Empty directories (delete_empty_directories must have the same effect).
fn dirs(root: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                walk(base, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

async fn rows_json(pool: &PgPool, sql: &str) -> Vec<Value> {
    let wrapped = format!("SELECT to_jsonb(x)::text FROM ({sql}) x");
    sqlx::query(&wrapped).fetch_all(pool).await.unwrap().iter().map(|r| serde_json::from_str(&r.get::<String, _>(0)).unwrap()).collect()
}

const COMPARE: &[(&str, &str)] = &[
    (
        "documents",
        "SELECT id, title, content, correspondent_id, document_type_id, storage_path_id, owner_id, filename, \
         archive_filename, checksum, archive_checksum, deleted_at IS NULL AS live, root_document_id, \
         modified > (SELECT max(t0) FROM eq_t0) AS touched FROM documents_document ORDER BY id",
    ),
    ("tags", "SELECT document_id, tag_id FROM documents_document_tags ORDER BY 1, 2"),
    ("workflowruns", "SELECT workflow_id, document_id, type FROM documents_workflowrun ORDER BY id"),
    // PaperRust history (workflow_ext): same entries in both paths
    (
        "paperrust_workflow_runs",
        "SELECT workflow_id, document_id, trigger_type, result, branch, code, changed_fields::text FROM paperrust_workflow_run ORDER BY id",
    ),
    (
        "auditlog",
        "SELECT object_pk, action, content_type_id, (changes::jsonb - 'modified') AS changes FROM auditlog_logentry \
         WHERE id > (SELECT max(audit_max) FROM eq_t0) ORDER BY id",
    ),
];

async fn reset_db(server: &str, name: &str) -> PgPool {
    let admin = PgPool::connect(&format!("{server}/postgres")).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.unwrap();
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE paperless_snapshot")).execute(&admin).await.unwrap();
    admin.close().await;
    let pool = PgPool::connect(&format!("{server}/{name}")).await.unwrap();
    // v3.4.2 snapshot → PaperRust tag migration (adjacency list) as at startup
    sqlx::raw_sql(TAG_ADJACENCY).execute(&pool).await.unwrap();
    sqlx::raw_sql(SETUP).execute(&pool).await.unwrap();
    sqlx::raw_sql("CREATE TABLE eq_t0 AS SELECT now() AS t0, (SELECT coalesce(max(id), 0) FROM auditlog_logentry) AS audit_max")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_BULK_EQ_SERVER (Referenz-PostgreSQL mit paperless_snapshot) und reference/data/media"]
async fn batched_equals_single() {
    let Ok(server) = std::env::var("PAPERRUST_BULK_EQ_SERVER") else {
        eprintln!("SKIP: PAPERRUST_BULK_EQ_SERVER not set");
        return;
    };
    assert!(server.contains("127.0.0.1") || server.contains("localhost"), "nur lokal");
    let media = reference_media();
    assert!(media.join("documents/originals").is_dir(), "Referenzmedien fehlen: {}", media.display());
    let work = std::env::temp_dir().join("paperrust_bulk_eq");
    let _ = std::fs::remove_dir_all(&work);

    let mut results = Vec::new();
    for (name, batched) in [("pl_bulk_eq_a", false), ("pl_bulk_eq_b", true)] {
        let pool = reset_db(&server, name).await;
        let root = work.join(name);
        for sub in ["originals", "archive"] {
            copy_tree(&media.join("documents").join(sub), &root.join("media/documents").join(sub));
        }
        let queue = Arc::new(CapturingQueue::default());
        let c = consumer(pool.clone(), &root, queue.clone());
        c.settings.ensure_directories().unwrap();
        let ids: Vec<i64> = sqlx::query_scalar::<_, i32>("SELECT id FROM documents_document ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(i64::from)
            .collect();
        let t = std::time::Instant::now();
        if batched {
            let st = c.bulk_update_signals(&ids).await.unwrap();
            eprintln!("{name}: batched {st:?} in {:?}", t.elapsed());
            assert!(st.workflow_deferred > 0 && st.filename_unchanged > 0 && st.filename_slow > 0, "{st:?}");
        } else {
            c.bulk_update_signals_single(&ids).await.unwrap();
            eprintln!("{name}: single in {:?}", t.elapsed());
        }
        let mut snap = BTreeMap::new();
        for (k, sql) in COMPARE {
            snap.insert(k.to_string(), Value::Array(rows_json(&pool, sql).await));
        }
        let jobs = queue.jobs.lock().unwrap().clone();
        snap.insert("queue".into(), Value::Array(jobs));
        let docs = root.join("media/documents");
        snap.insert("files".into(), serde_json::to_value(tree(&docs)).unwrap());
        snap.insert("dirs".into(), serde_json::to_value(dirs(&docs)).unwrap());
        results.push(snap);
        pool.close().await;
    }
    let (a, b) = (&results[0], &results[1]);
    for k in a.keys() {
        if a[k] != b[k] {
            let (va, vb) = (a[k].as_array(), b[k].as_array());
            if let (Some(va), Some(vb)) = (va, vb) {
                for (i, (x, y)) in va.iter().zip(vb.iter()).enumerate() {
                    if x != y {
                        eprintln!("{k}[{i}]\n  single:  {x}\n  batched: {y}");
                        break;
                    }
                }
                eprintln!("{k}: {} vs {} Einträge", va.len(), vb.len());
            } else {
                eprintln!("{k}:\n  single:  {}\n  batched: {}", a[k], b[k]);
            }
            panic!("Unterschied in {k}");
        }
    }
    // Plausibility: the setup really moved, collided and executed workflows
    let files = a["files"].as_object().unwrap();
    assert!(files.keys().any(|f| f.contains("eq/") && f.contains("_01")), "keine Namenskollision: {:?}", files.keys());
    assert!(a["workflowruns"].as_array().unwrap().len() >= 2);
    assert!(a["documents"].as_array().unwrap().iter().any(|d| d["title"] == "EQ chained"));
    eprintln!(
        "gleich: {} Dokumente, {} Dateien, {} Audit-Einträge, {} Workflow-Läufe",
        a["documents"].as_array().unwrap().len(),
        files.len(),
        a["auditlog"].as_array().unwrap().len(),
        a["workflowruns"].as_array().unwrap().len()
    );
}
