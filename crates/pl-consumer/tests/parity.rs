//! Parity with upstream: the same inputs (tests/parity/pl-consumer/cases.json) as
//! `run_upstream.py`, here against an own DB copy (`pl_consumer_test` or `pl_w9_test`).
//! Writes `tests/parity/pl-consumer/out/rust_results.json`; the comparison of the resulting
//! records is done in `compare.py` (reads both DBs). Procedure:
//!
//! ```text
//! tests/parity/pl-consumer/reset_db.sh
//! tests/parity/pl-consumer/run_upstream.sh
//! PAPERRUST_CONSUMER_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_consumer_test \
//!   cargo test -p pl-consumer -j 1 --test parity -- --ignored
//! reference/venv/Scripts/python.exe tests/parity/pl-consumer/compare.py
//! ```
//! (DB names via `PL_CONSUMER_DB`, see reset_db.sh / run_upstream.sh / compare.py.)
//!
//! Barcode cases: the splitting enqueues new `consume_file` tasks; an intercepting
//! queue ingests them directly afterwards in order (like Celery eager upstream).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;
use pl_consumer::native::{self, NativeParsing, RecordingProgress};
use pl_consumer::{ConsumableDocument, ConsumeOutcome, Consumer, ConsumerSettings, DocumentMetadataOverrides, DocumentSource};
use serde_json::{Map, Value, json};
use sqlx::PgPool;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Intercepts enqueued `consume_file` tasks.
#[derive(Default)]
struct CapturingQueue {
    jobs: Mutex<Vec<Map<String, Value>>>,
}

impl pl_queue::TaskQueue for CapturingQueue {
    fn enqueue<'a>(
        &'a self,
        task_name: &'a str,
        kwargs: Map<String, Value>,
        _opts: pl_queue::EnqueueOptions,
    ) -> BoxFuture<'a, Result<uuid::Uuid, pl_queue::QueueError>> {
        assert_eq!(task_name, pl_consumer::task::TASK_NAME);
        self.jobs.lock().unwrap().push(kwargs);
        Box::pin(async { Ok(uuid::Uuid::new_v4()) })
    }
    fn status(&self, _task_id: uuid::Uuid) -> BoxFuture<'_, Result<Option<pl_queue::TaskInfo>, pl_queue::QueueError>> {
        Box::pin(async { Ok(None) })
    }
}

fn consumer(pool: PgPool, root: &Path, case: &Value, queue: Arc<CapturingQueue>) -> Consumer {
    let mut s = ConsumerSettings::with_root(root.join("media"), root.join("data"), root.join("scratch"), root.join("consume"));
    // like reference/env.sh
    s.time_zone = chrono_tz::Europe::Zurich;
    s.templating.time_zone = chrono_tz::Europe::Zurich;
    s.secret_key = "paperless-rs-reference-secret-key-local-only-0123456789".into();
    s.consumer_delete_duplicates = case["delete_duplicates"].as_bool().unwrap_or(false);
    s.templating.filename_format = case["filename_format"].as_str().map(str::to_string);
    // Django settings of the case (override_settings upstream)
    if let Some(Value::Object(m)) = case.get("settings") {
        for (k, v) in m {
            let b = v.as_bool().unwrap_or(false);
            match k.as_str() {
                "CONSUMER_ENABLE_BARCODES" => s.barcodes.barcodes_enabled = b,
                "CONSUMER_ENABLE_ASN_BARCODE" => s.barcodes.barcode_enable_asn = b,
                "CONSUMER_BARCODE_TIFF_SUPPORT" => s.barcodes.barcode_enable_tiff_support = b,
                "CONSUMER_ENABLE_TAG_BARCODE" => s.barcodes.barcode_enable_tag = b,
                "CONSUMER_STORE_BARCODE_VALUES" => s.barcodes.barcode_store_values = b,
                "CONSUMER_BARCODE_RETAIN_SPLIT_PAGES" => s.barcodes.barcode_retain_split_pages = b,
                other => panic!("unbekannte Einstellung {other}"),
            }
        }
    }
    let mut ocr = pl_ocr::OcrConfig::defaults();
    ocr.threads_per_worker = 1;
    if let Some(m) = case["ocr_mode"].as_str() {
        ocr.mode = pl_ocr::config::Mode::parse(m).unwrap();
    }
    if let Some(a) = case["archive_file_generation"].as_str() {
        ocr.archive_file_generation = pl_ocr::config::ArchiveFileGeneration::parse(a).unwrap();
    }
    let mut parsing = NativeParsing::new(ocr, s.time_zone);
    parsing.tesseract = None;
    parsing.pdfium =
        pl_ocr::render::PdfiumRenderer::load(&repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")).ok();
    // like run_upstream.sh (PAPERLESS_THUMBNAIL_FONT_NAME)
    if let Ok(f) = pl_pdfops::thumbnail::ThumbnailFont::load(Path::new("C:/Windows/Fonts/arial.ttf")) {
        parsing.thumbnail_font = Some(Arc::new(f));
    }
    let barcodes: Arc<dyn pl_consumer::stages::BarcodeScanner> = match &parsing.pdfium {
        Some(p) => Arc::new(native::PdfOpsBarcodes::new(Arc::new(p.clone()))),
        None => Arc::new(native::NoBarcodes),
    };
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
        barcodes,
        pages: Arc::new(native::LopdfPages),
        effects: Arc::new(native::LoggingEffects),
        scripts: Arc::new(native::CommandScripts),
        queue: Some(queue),
        remote_ocr_by_default: false,
        events: None,
        caches: Arc::new(pl_consumer::signals::NoCaches),
        deleter: Arc::new(pl_consumer::workflows::NativeSoftDelete),
        vision: None,
    }
}

const APP_CONFIG_COLUMNS: &[&str] = &[
    "barcodes_enabled",
    "barcode_enable_tiff_support",
    "barcode_string",
    "barcode_retain_split_pages",
    "barcode_enable_asn",
    "barcode_asn_prefix",
    "barcode_upscale",
    "barcode_dpi",
    "barcode_max_pages",
    "barcode_enable_tag",
    "barcode_tag_mapping",
    "barcode_tag_split",
    "barcode_store_values",
];

/// `ApplicationConfiguration.objects.update(**values)` (barcode columns only; `null` = NULL).
async fn set_app_config(pool: &PgPool, values: &Map<String, Value>) {
    for (k, v) in values {
        assert!(APP_CONFIG_COLUMNS.contains(&k.as_str()), "unbekannte Spalte {k}");
        if v.is_null() {
            sqlx::query(&format!("UPDATE paperless_applicationconfiguration SET {k} = NULL")).execute(pool).await.unwrap();
            continue;
        }
        let sql = format!("UPDATE paperless_applicationconfiguration SET {k} = $1");
        let q = sqlx::query(&sql);
        let q = match (k.as_str(), v) {
            ("barcode_tag_mapping", v) => q.bind(v.clone()),
            (_, Value::Bool(b)) => q.bind(*b),
            ("barcode_upscale", Value::Number(n)) => q.bind(n.as_f64()),
            (_, Value::Number(n)) => q.bind(n.as_i64().map(|i| i as i16)),
            (_, Value::String(s)) => q.bind(s.clone()),
            other => panic!("unsupported {other:?}"),
        };
        q.execute(pool).await.unwrap();
    }
}

async fn max_doc_id(pool: &PgPool) -> i64 {
    let v: Option<i32> = sqlx::query_scalar("SELECT max(id) FROM documents_document").fetch_one(pool).await.unwrap();
    i64::from(v.unwrap_or(0))
}

async fn new_doc_ids(pool: &PgPool, after: i64) -> Vec<i64> {
    let v: Vec<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE id > $1 ORDER BY id")
        .bind(after as i32)
        .fetch_all(pool)
        .await
        .unwrap();
    v.into_iter().map(i64::from).collect()
}

fn outcome_json(r: Result<ConsumeOutcome, pl_consumer::ConsumeError>) -> Map<String, Value> {
    let mut e = Map::new();
    match r {
        Ok(ConsumeOutcome::Success { document_id }) => {
            e.insert("state".into(), json!("SUCCESS"));
            e.insert("result".into(), json!({"document_id": document_id}));
        }
        Ok(other) => {
            e.insert("state".into(), json!("SUCCESS"));
            e.insert("result".into(), other.to_value());
        }
        Err(err) => {
            e.insert("state".into(), json!("FAILURE"));
            e.insert("error".into(), json!(err.to_string()));
            e.insert("error_type".into(), json!(err.error_type()));
        }
    }
    e
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (frisch angelegte DB-Kopie pl_consumer_test bzw. pl_w9_test)"]
async fn parity_cases() {
    let Ok(url) = std::env::var("PAPERRUST_CONSUMER_TEST_DATABASE_URL") else {
        eprintln!("SKIP parity_cases: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let db = url.rsplit('/').next().unwrap_or_default();
    assert!(db.starts_with("pl_consumer_test") || db.starts_with("pl_w9_test"), "refusing {url}");
    let pool = PgPool::connect(&url).await.unwrap();
    let dir = repo().join("tests/parity/pl-consumer");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let root = std::env::temp_dir().join(format!("pl_consumer_parity_{}", std::process::id()));
    std::fs::create_dir_all(root.join("consume")).unwrap();
    let mut out = Map::new();
    for c in cases["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let queue = Arc::new(CapturingQueue::default());
        let cons = consumer(pool.clone(), &root, c, queue.clone());
        let p = root.join("consume").join(c["filename"].as_str().unwrap());
        match c["sample"].as_str() {
            Some(rel) => {
                std::fs::copy(repo().join("tests/parity").join(rel), &p).unwrap();
            }
            None => std::fs::write(&p, c["content"].as_str().unwrap()).unwrap(),
        }
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(c["mtime"].as_u64().unwrap());
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(mtime).set_accessed(mtime)).unwrap();
        drop(f);
        if let Some(Value::Object(ac)) = c.get("app_config") {
            set_app_config(&pool, ac).await;
        }
        let before = max_doc_id(&pool).await;
        let o: DocumentMetadataOverrides = serde_json::from_value(c["overrides"].clone()).unwrap();
        let source = DocumentSource::from_i64(c["source"].as_i64().unwrap()).unwrap();
        let doc = ConsumableDocument::new(source, &p).unwrap();
        let r = cons.consume_file(doc, Some(o), None).await;
        let mut e = outcome_json(r);
        // Partial documents of the barcode splitting (upstream: eager in the same call)
        let jobs: Vec<Map<String, Value>> = std::mem::take(&mut *queue.jobs.lock().unwrap());
        let mut children = Vec::new();
        for k in jobs {
            let (mut d, ov) = pl_consumer::task::parse_kwargs(&k).unwrap();
            if d.mime_type.is_empty() {
                d.mime_type = pl_parsers::mime::detect_file(&d.original_file).unwrap_or_default();
            }
            children.push(Value::Object(outcome_json(cons.consume_file(d, ov, None).await)));
        }
        if !children.is_empty() {
            e.insert("children".into(), Value::Array(children));
        }
        e.insert("new_ids".into(), json!(new_doc_ids(&pool, before).await));
        if let Some(Value::Object(ac)) = c.get("app_config") {
            let reset: Map<String, Value> = ac.keys().map(|k| (k.clone(), Value::Null)).collect();
            set_app_config(&pool, &reset).await;
        }
        e.insert("file_left".into(), json!(p.exists()));
        eprintln!("{name} {}", Value::Object(e.clone()));
        out.insert(name.to_string(), Value::Object(e));
    }
    let res = json!({"media_root": root.join("media").to_string_lossy(), "cases": out});
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out/rust_results.json"), serde_json::to_string_pretty(&res).unwrap()).unwrap();
}
