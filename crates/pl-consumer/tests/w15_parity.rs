//! W15 parity: re-ingestion (`update_document_content_maybe_archive_file`), scheduled workflows
//! (`check_scheduled_workflows`) and the signal part of `bulk_update_documents` against upstream.
//! Same steps and files as `tests/parity/pl-consumer/w15/run_upstream.py`
//! (`fixtures.json`), here against `pl_w15_rs`. Comparison: `compare.py` (reads both DBs/media).
//!
//! ```text
//! tests/parity/pl-consumer/w15/reset_db.sh
//! tests/parity/pl-consumer/w15/run_upstream.sh
//! PAPERRUST_W15_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_w15_rs \
//!   cargo test -p pl-consumer -j 1 --test w15_parity -- --ignored
//! reference/venv/Scripts/python.exe tests/parity/pl-consumer/w15/compare.py
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use pl_consumer::native::{self, NativeParsing, RecordingProgress};
use pl_consumer::stages::{DocumentParsing, ParseRequest, ParsedOutput, ParserInfo, StageError};
use pl_consumer::{Consumer, ConsumerSettings};
use serde_json::{Map, Value, json};
use sqlx::PgPool;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn w15_dir() -> PathBuf {
    repo().join("tests/parity/pl-consumer/w15")
}

/// Enqueued tasks (name + kwargs).
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

/// Real parser, except for `fake_archive.filename` (test parser as in the upstream run).
struct FakeArchiveParsing {
    inner: NativeParsing,
    filename: String,
    text: String,
    thumbnail: Vec<u8>,
}

const FAKE: &str = "FakeParser";

impl DocumentParsing for FakeArchiveParsing {
    fn parser_for(&self, mime_type: &str, filename: &str, path: &Path, allow_remote: bool) -> Option<ParserInfo> {
        if filename == self.filename {
            return Some(ParserInfo {
                class_name: FAKE.into(),
                name: "fake".into(),
                can_produce_archive: true,
                requires_pdf_rendition: false,
                uses_remote_service: false,
            });
        }
        self.inner.parser_for(mime_type, filename, path, allow_remote)
    }
    fn should_produce_archive(&self, parser: &ParserInfo, mime_type: &str, path: &Path) -> bool {
        parser.class_name == FAKE || self.inner.should_produce_archive(parser, mime_type, path)
    }
    fn parse(&self, parser: &ParserInfo, req: &ParseRequest<'_>, produce_archive: bool) -> Result<ParsedOutput, StageError> {
        if parser.class_name != FAKE {
            return self.inner.parse(parser, req, produce_archive);
        }
        let a = req.tempdir.join("archive.pdf");
        std::fs::copy(req.path, &a).map_err(|e| StageError::Other(e.to_string()))?;
        Ok(ParsedOutput { text: Some(self.text.clone()), date: None, archive_path: Some(a), truncation: None })
    }
    fn thumbnail(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Result<PathBuf, StageError> {
        if parser.class_name != FAKE {
            return self.inner.thumbnail(parser, req, archive);
        }
        let t = req.tempdir.join("thumb.webp");
        std::fs::write(&t, &self.thumbnail).map_err(|e| StageError::Other(e.to_string()))?;
        Ok(t)
    }
    fn thumbnail_with_text(
        &self,
        parser: &ParserInfo,
        req: &ParseRequest<'_>,
        archive: Option<&Path>,
        text: Option<&str>,
    ) -> Result<PathBuf, StageError> {
        if parser.class_name == FAKE {
            return self.thumbnail(parser, req, archive);
        }
        self.inner.thumbnail_with_text(parser, req, archive, text)
    }
    fn page_count(&self, parser: &ParserInfo, req: &ParseRequest<'_>, archive: Option<&Path>) -> Option<i64> {
        if parser.class_name == FAKE { None } else { self.inner.page_count(parser, req, archive) }
    }
}

fn consumer(pool: PgPool, root: &Path, fx: &Value, queue: Arc<CapturingQueue>) -> Consumer {
    let mut s = ConsumerSettings::with_root(root.join("media"), root.join("data"), root.join("scratch"), root.join("consume"));
    // like reference/env.sh
    s.time_zone = chrono_tz::Europe::Zurich;
    s.templating.time_zone = chrono_tz::Europe::Zurich;
    s.secret_key = "paperless-rs-reference-secret-key-local-only-0123456789".into();
    s.audit_log_enabled = true;
    let mut ocr = pl_ocr::OcrConfig::defaults();
    ocr.threads_per_worker = 1;
    ocr.mode = pl_ocr::config::Mode::parse("off").unwrap();
    ocr.archive_file_generation = pl_ocr::config::ArchiveFileGeneration::parse("never").unwrap();
    let mut parsing = NativeParsing::new(ocr, s.time_zone);
    parsing.tesseract = None;
    parsing.pdfium = pl_ocr::render::PdfiumRenderer::load(&repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")).ok();
    if let Ok(f) = pl_pdfops::thumbnail::ThumbnailFont::load(Path::new("C:/Windows/Fonts/arial.ttf")) {
        parsing.thumbnail_font = Some(Arc::new(f));
    }
    let fake = &fx["fake_archive"];
    let parsing = FakeArchiveParsing {
        inner: parsing,
        filename: fake["filename"].as_str().unwrap().into(),
        text: fake["text"].as_str().unwrap().into(),
        thumbnail: fake["thumbnail"].as_str().unwrap().as_bytes().to_vec(),
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
        barcodes: Arc::new(native::NoBarcodes),
        pages: Arc::new(native::LopdfPages),
        effects: Arc::new(native::LoggingEffects),
        scripts: Arc::new(native::CommandScripts),
        queue: Some(queue),
        remote_ocr_by_default: false,
        events: Some(pl_queue::EventBus::new(64)),
        caches: Arc::new(pl_consumer::signals::NoCaches),
        deleter: Arc::new(pl_consumer::workflows::NativeSoftDelete),
        vision: None,
    }
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_W15_DATABASE_URL (frische DB pl_w15_rs aus tests/parity/pl-consumer/w15/reset_db.sh)"]
async fn w15_parity_steps() {
    let Ok(url) = std::env::var("PAPERRUST_W15_DATABASE_URL") else {
        eprintln!("SKIP: PAPERRUST_W15_DATABASE_URL not set");
        return;
    };
    assert!(url.ends_with("/pl_w15_rs"), "nur gegen die eigene Kopie pl_w15_rs");
    let fx: Value = serde_json::from_str(&std::fs::read_to_string(w15_dir().join("fixtures.json")).unwrap()).unwrap();
    let root = w15_dir().join("out/rs_root");
    let _ = std::fs::remove_dir_all(&root);
    let originals = root.join("media/documents/originals");
    for f in fx["files"].as_array().unwrap() {
        let p = originals.join(f["filename"].as_str().unwrap());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        match f.get("sample").and_then(Value::as_str) {
            Some(s) => {
                std::fs::copy(repo().join("tests/parity").join(s), &p).unwrap();
            }
            None => std::fs::write(&p, f["text"].as_str().unwrap()).unwrap(),
        }
    }
    let pool = PgPool::connect(&url).await.unwrap();
    let queue = Arc::new(CapturingQueue::default());
    let c = consumer(pool, &root, &fx, queue.clone());
    c.settings.ensure_directories().unwrap();
    let mut steps = Map::new();
    for st in fx["steps"].as_array().unwrap() {
        let name = st["name"].as_str().unwrap();
        let r: Result<(), (String, String)> = match st["op"].as_str().unwrap() {
            "reprocess" => c
                .update_document_content_maybe_archive_file(st["id"].as_i64().unwrap(), false)
                .await
                .map(|_| ())
                .map_err(|e| (e.error_type().to_string(), e.to_string())),
            "scheduled" => c.check_scheduled_workflows().await.map_err(|e| ("OperationalError".into(), e.to_string())),
            "bulk_update" => {
                let ids: Vec<i64> = st["ids"].as_array().unwrap().iter().filter_map(Value::as_i64).collect();
                c.bulk_update_signals(&ids).await.map(|_| ()).map_err(|e| ("OperationalError".into(), e.to_string()))
            }
            other => panic!("unbekannter Schritt {other}"),
        };
        let v = match r {
            Ok(()) => json!({"ok": true}),
            Err((t, m)) => json!({"ok": false, "error_type": t, "error": m}),
        };
        eprintln!("{name} {v}");
        steps.insert(name.into(), v);
    }
    let tasks = queue.jobs.lock().unwrap().clone();
    let res = json!({"media_root": root.join("media").to_string_lossy(), "steps": steps, "tasks": tasks});
    std::fs::write(w15_dir().join("out/rust.json"), serde_json::to_string_pretty(&res).unwrap()).unwrap();
}
