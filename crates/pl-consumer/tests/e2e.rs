//! End to end against an own DB copy and a temp data directory.
//!
//! Prerequisite: `PAPERRUST_CONSUMER_TEST_DATABASE_URL` pointing to a database whose name starts with
//! `pl_consumer_test` (copy of `paperless_snapshot`, see
//! `tests/parity/pl-consumer/reset_db.sh`). Without the variable: the `#[ignore]` test ends with SKIP.
//! Tools as in pl-ocr/tests/tools.rs (Tesseract from `tests/parity/pl-ocr/.tools`, PDFium from
//! `reference/venv`); if they are missing, the OCR cases are skipped.
//!
//! `cargo test -p pl-consumer -j 1 --test e2e -- --ignored --test-threads=1`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pl_consumer::native::{self, NativeParsing, RecordingProgress};
use pl_consumer::stages::ProgressStatus;
use pl_consumer::{ConsumableDocument, ConsumeOutcome, Consumer, ConsumerSettings, DocumentMetadataOverrides, DocumentSource};
use serde_json::Value;
use sqlx::{PgPool, Row};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample(rel: &str) -> PathBuf {
    repo().join("tests/parity").join(rel)
}

async fn pool() -> Option<PgPool> {
    let url = std::env::var("PAPERRUST_CONSUMER_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_consumer_test"), "refusing to run against database {name}");
    let pool = PgPool::connect(&url).await.expect("connect");
    // Copies of `paperless_snapshot` still have the django-treenode columns (NOT NULL without a
    // DB default); PaperRust creates tags without them (B-325: `get_or_create_tag` failed, the
    // watcher only logged and the tag was missing). Like the server: apply the migration (idempotent).
    sqlx::raw_sql(TAG_ADJACENCY).execute(&pool).await.expect("pl_objects tag adjacency migration");
    Some(pool)
}

const TAG_ADJACENCY: &str = include_str!("../../pl-objects/migrations/0001_paperrust_tag_adjacency.sql");

/// Processing is a large future in the debug build; on the default stack of the test thread
/// (Windows 1–2 MiB) `e2e_consume` overflowed (B-325, STATUS_STACK_OVERFLOW). Own thread
/// with 64 MiB stack and its own runtime.
fn run_big<F: std::future::Future<Output = ()>>(make: impl FnOnce() -> F + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(make()))
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

struct Env {
    root: tempfile::TempDir,
    consumer: Consumer,
    progress: Arc<RecordingProgress>,
    has_ocr: bool,
}

impl Env {
    fn consume_dir(&self) -> PathBuf {
        self.root.path().join("consume")
    }
    fn settings(&self) -> &ConsumerSettings {
        &self.consumer.settings
    }
    /// Copy a file into the input (optionally with new content at the end → own checksum).
    fn put(&self, src: &Path, name: &str) -> PathBuf {
        let dst = self.consume_dir().join(name);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(src, &dst).unwrap();
        dst
    }
    fn put_text(&self, name: &str, text: &str) -> PathBuf {
        let dst = self.consume_dir().join(name);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, text).unwrap();
        dst
    }
    async fn consume(&self, path: &Path, o: DocumentMetadataOverrides) -> Result<ConsumeOutcome, pl_consumer::ConsumeError> {
        let doc = ConsumableDocument::new(DocumentSource::ApiUpload, path).unwrap();
        self.consumer.consume_file(doc, Some(o), Some("e2e-task".into())).await
    }
}

fn make_env(pool: PgPool, f: impl FnOnce(&mut ConsumerSettings)) -> Env {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let mut s = ConsumerSettings::with_root(p.join("media"), p.join("data"), p.join("scratch"), p.join("consume"));
    s.time_zone = chrono_tz::Europe::Zurich;
    s.templating.time_zone = chrono_tz::Europe::Zurich;
    f(&mut s);
    let mut ocr = pl_ocr::OcrConfig::defaults();
    ocr.threads_per_worker = 1;
    let mut parsing = NativeParsing::new(ocr, s.time_zone);
    let tbase = repo().join("tests/parity/pl-ocr/.tools/tesseract");
    let exe = tbase.join("bin").join(if cfg!(windows) { "tesseract.exe" } else { "tesseract" });
    let t = pl_ocr::engine::TesseractCli { program: exe, tessdata_dir: Some(tbase.join("tessdata")), psm: None };
    parsing.tesseract = Some(t).filter(|t| t.is_available());
    parsing.pdfium = pl_ocr::render::PdfiumRenderer::load(&repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll")).ok();
    let has_ocr = parsing.tesseract.is_some() && parsing.pdfium.is_some();
    let index_dir = s.data_dir.join("index");
    std::fs::create_dir_all(&index_dir).unwrap();
    let index = native::TantivyIndex::open(pl_indexer::IndexerConfig { index_dir, language: None }).unwrap();
    let dates = native::RegexDates::from_settings(&s, "eng");
    let progress = Arc::new(RecordingProgress::default());
    let consumer = Consumer {
        pool,
        settings: Arc::new(s),
        parsing: Arc::new(parsing),
        dates: Arc::new(dates),
        classifier: Arc::new(native::NoClassifier),
        index: Arc::new(index),
        progress: progress.clone(),
        barcodes: Arc::new(native::NoBarcodes),
        pages: Arc::new(native::LopdfPages),
        effects: Arc::new(native::LoggingEffects),
        scripts: Arc::new(native::CommandScripts),
        queue: None,
        remote_ocr_by_default: false,
        events: None,
        caches: Arc::new(pl_consumer::signals::NoCaches),
        deleter: Arc::new(pl_consumer::workflows::NativeSoftDelete),
        vision: None,
    };
    Env { root, consumer, progress, has_ocr }
}

struct Doc {
    title: String,
    content: String,
    mime: String,
    checksum: String,
    archive_checksum: Option<String>,
    filename: Option<String>,
    archive_filename: Option<String>,
    original_filename: Option<String>,
    page_count: Option<i32>,
    created: chrono::NaiveDate,
    correspondent: Option<i32>,
    document_type: Option<i32>,
    storage_path: Option<i32>,
    owner: Option<i32>,
    asn: Option<i32>,
    tags: Vec<i32>,
}

async fn load(pool: &PgPool, id: i64) -> Doc {
    let r = sqlx::query("select * from documents_document where id = $1").bind(id as i32).fetch_one(pool).await.unwrap();
    let tags: Vec<i32> = sqlx::query_scalar("select tag_id from documents_document_tags where document_id = $1 order by tag_id")
        .bind(id as i32)
        .fetch_all(pool)
        .await
        .unwrap();
    Doc {
        title: r.get("title"),
        content: r.get("content"),
        mime: r.get("mime_type"),
        checksum: r.get("checksum"),
        archive_checksum: r.get("archive_checksum"),
        filename: r.get("filename"),
        archive_filename: r.get("archive_filename"),
        original_filename: r.get("original_filename"),
        page_count: r.get("page_count"),
        created: r.get("created"),
        correspondent: r.get("correspondent_id"),
        document_type: r.get("document_type_id"),
        storage_path: r.get("storage_path_id"),
        owner: r.get("owner_id"),
        asn: r.get("archive_serial_number"),
        tags,
    }
}

fn ok_id(r: Result<ConsumeOutcome, pl_consumer::ConsumeError>) -> i64 {
    match r {
        Ok(ConsumeOutcome::Success { document_id }) => document_id,
        other => panic!("unexpected {other:?}"),
    }
}

fn search(env: &Env, q: &str) -> Vec<i64> {
    let b = pl_search::SearchBackend::new(pl_search::SearchSettings {
        index_dir: env.settings().data_dir.join("index"),
        language: None,
        fuzzy_threshold: None,
        time_zone: chrono_tz::Europe::Zurich,
    });
    b.search_ids(q, None, None, false, pl_search::SearchMode::Query, None, Some(50)).unwrap()
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_consume() {
    run_big(e2e_consume_impl);
}

async fn e2e_consume_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP e2e_consume: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    // seeded workflow 1 (consumption trigger) must not interfere – its filters need */scans/*invoice*
    let env = make_env(pool.clone(), |_| {});

    // --- A: PDF with text -------------------------------------------------------------------
    let src = sample("pl-parsers/samples/pdf_text.pdf");
    let f = env.put(&src, "Kontoauszug Text.pdf");
    let id_a = ok_id(env.consume(&f, Default::default()).await);
    let d = load(&pool, id_a).await;
    assert_eq!(d.mime, "application/pdf");
    // Content "Rechnung 2025 …" → seed workflow 1 (Document Added, ANY "invoice rechnung") applies
    assert_eq!(d.title, "{correspondent} - {document_type}");
    assert_eq!((d.correspondent, d.owner), (Some(1), Some(3)));
    assert_eq!(d.original_filename.as_deref(), Some("Kontoauszug Text.pdf"));
    assert_eq!(d.checksum, pl_consumer::compute_checksum(&src).unwrap());
    assert!(!d.content.trim().is_empty(), "content");
    assert_eq!(d.filename.as_deref(), Some(format!("{id_a:07}.pdf").as_str()));
    assert!(d.archive_filename.is_none(), "born-digital PDF: no archive in auto mode");
    assert!(d.tags.contains(&6) && !d.tags.contains(&1), "workflow tags, inbox removed: {:?}", d.tags);
    assert!(!f.exists(), "original removed");
    let src_path = env.settings().originals_dir.join(d.filename.as_ref().unwrap());
    assert_eq!(pl_consumer::compute_checksum(&src_path).unwrap(), d.checksum);
    assert!(env.settings().thumbnail_path(id_a).is_file());
    assert_eq!(d.page_count, Some(1));
    let ups = env.progress.updates.lock().unwrap().clone();
    let msgs: Vec<&str> = ups.iter().map(|u| u.message.as_str()).collect();
    assert_eq!(msgs.first(), Some(&"new_file"));
    assert!(msgs.contains(&"parsing_document") && msgs.contains(&"generating_thumbnail") && msgs.contains(&"save_document"));
    assert_eq!(ups.last().unwrap().status, ProgressStatus::Success);
    assert_eq!(ups.last().unwrap().document_id, Some(id_a));
    let audit: i64 = sqlx::query_scalar("select count(*) from auditlog_logentry where object_id = $1 and content_type_id = 17")
        .bind(id_a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(audit >= 2, "audit create + tags");

    // --- D: duplicate ----------------------------------------------------------------------
    let f = env.put(&src, "again.pdf");
    let id_dup = ok_id(env.consume(&f, Default::default()).await); // without DELETE_DUPLICATES: is ingested
    assert_ne!(id_dup, id_a);
    let env_del = make_env(pool.clone(), |s| s.consumer_delete_duplicates = true);
    let f = env_del.put(&src, "third.pdf");
    match env_del.consume(&f, Default::default()).await {
        Ok(ConsumeOutcome::Duplicate { duplicate_of, duplicate_in_trash }) => {
            assert!(duplicate_of == id_a || duplicate_of == id_dup);
            assert!(!duplicate_in_trash);
        }
        other => panic!("expected duplicate, got {other:?}"),
    }
    assert!(!f.exists(), "duplicate deleted");
    let last = env_del.progress.updates.lock().unwrap().last().cloned().unwrap();
    assert_eq!((last.status, last.message.as_str()), (ProgressStatus::Failed, "document_already_exists"));

    // --- C: E-Mail ------------------------------------------------------------------------
    let f = env.put(&sample("pl-parsers/samples/mail_simple.eml"), "mail.eml");
    let id_c = ok_id(env.consume(&f, Default::default()).await);
    let d = load(&pool, id_c).await;
    assert_eq!(d.mime, "message/rfc822");
    assert!(d.content.starts_with("Subject: Einfache Nachricht"), "{}", d.content);
    assert_eq!(d.created, chrono::NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), "mail date");
    assert_eq!(d.filename.as_deref(), Some(format!("{id_c:07}.eml").as_str()));

    // --- E: Overrides ---------------------------------------------------------------------
    let f = env.put_text("override.txt", "Ein einfacher Text ohne Treffer zzqx.");
    let o = DocumentMetadataOverrides {
        title: Some("{{ correspondent }} – {{ original_filename }}".into()),
        correspondent_id: Some(4),
        document_type_id: Some(5),
        tag_ids: Some(vec![4]),
        owner_id: Some(2),
        view_users: Some(vec![3]),
        created: chrono::NaiveDate::from_ymd_opt(2020, 5, 6),
        asn: Some(987_654),
        custom_fields: Some([(1, Value::from("hello")), (5, Value::from(42))].into_iter().collect()),
        ..Default::default()
    };
    let id_e = ok_id(env.consume(&f, o).await);
    let d = load(&pool, id_e).await;
    assert_eq!(d.title, "Swisscom – override");
    assert_eq!((d.correspondent, d.document_type, d.owner, d.asn), (Some(4), Some(5), Some(2), Some(987_654)));
    assert_eq!(d.created, chrono::NaiveDate::from_ymd_opt(2020, 5, 6).unwrap());
    assert_eq!(d.tags, vec![1, 2, 3, 4], "tag + ancestors + inbox");
    let cf: Vec<(i32, Option<String>, Option<i32>)> = sqlx::query_as(
        "select field_id, value_text, value_int from documents_customfieldinstance where document_id = $1 order by field_id",
    )
    .bind(id_e as i32)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(cf, vec![(1, Some("hello".into()), None), (5, None, Some(42))]);
    let perms: i64 = sqlx::query_scalar(
        "select count(*) from guardian_userobjectpermission where object_pk = $1 and user_id = 3 and content_type_id = 17",
    )
    .bind(id_e.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(perms, 1, "view_document for alice");
    // ASN already assigned
    let f = env.put_text("asn2.txt", "Noch ein Text mit anderer Prüfsumme 1.");
    let e = env.consume(&f, DocumentMetadataOverrides { asn: Some(987_654), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.to_string(), "asn2.txt: Not consuming asn2.txt: Given ASN 987654 already exists!");
    let e = env.consume(&f, DocumentMetadataOverrides { asn: Some(-1), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.to_string(), "asn2.txt: Not consuming asn2.txt: Given ASN -1 is out of range [0, 4,294,967,295]");

    // --- F: assignment MATCH_ANY / MATCH_REGEX + storage path template -----------------------
    let f = env.put_text("bill.txt", "Your Swisscom mobile bill. Tax notice from the steueramt.\nDatum 15.03.2024");
    let id_f = ok_id(env.consume(&f, Default::default()).await);
    let d = load(&pool, id_f).await;
    assert_eq!(d.correspondent, Some(4), "Swisscom (ANY) sorts before Tax Office (REGEX)");
    assert!(d.tags.contains(&3) && d.tags.contains(&2), "Taxes (ANY tax) + Finance ancestor: {:?}", d.tags);
    assert_eq!(d.storage_path, Some(2), "storage path Taxes (ANY)");
    assert_eq!(d.created, chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(), "date from content");
    assert_eq!(d.filename.as_deref(), Some("taxes/2024/03-bill.txt"));
    assert!(env.settings().originals_dir.join("taxes/2024/03-bill.txt").is_file());

    let f = env.put_text("amt.txt", "Schreiben vom Steueramt Zürich");
    let id_r = ok_id(env.consume(&f, Default::default()).await);
    assert_eq!(load(&pool, id_r).await.correspondent, Some(6), "Tax Office via REGEX");

    // --- G: file name from template (PAPERLESS_FILENAME_FORMAT) + collision ------------------
    let env_fmt = make_env(pool.clone(), |s| s.templating.filename_format = Some("{correspondent}/{title}".into()));
    let f = env_fmt.put_text("Brief.txt", "Brief von Landlord ohne Treffer 1");
    let id_g1 = ok_id(env_fmt.consume(&f, Default::default()).await);
    let f = env_fmt.put_text("Brief.txt", "Brief von Landlord ohne Treffer 2");
    let id_g2 = ok_id(env_fmt.consume(&f, Default::default()).await);
    assert_eq!(load(&pool, id_g1).await.filename.as_deref(), Some("none/Brief.txt"));
    assert_eq!(load(&pool, id_g2).await.filename.as_deref(), Some("none/Brief_01.txt"));

    // --- Workflow "Document Added" (seed workflow 1: ANY "invoice rechnung") -----------------
    let f = env.put_text("wf.txt", "Diese Rechnung bitte bezahlen 7781");
    let id_w = ok_id(env.consume(&f, Default::default()).await);
    let d = load(&pool, id_w).await;
    assert_eq!(d.title, "{correspondent} - {document_type}");
    assert_eq!((d.correspondent, d.document_type, d.owner), (Some(1), Some(1), Some(3)));
    assert!(!d.tags.contains(&1), "removal action removed inbox tag: {:?}", d.tags);
    assert!(d.tags.contains(&6) && d.tags.contains(&8), "{:?}", d.tags);
    let runs: i64 = sqlx::query_scalar("select count(*) from documents_workflowrun where document_id = $1")
        .bind(id_w as i32)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);

    // --- Fehlerpfade ------------------------------------------------------------------------
    let missing = env.consume_dir().join("missing.txt");
    std::fs::write(&missing, "x").unwrap();
    let doc = ConsumableDocument::new(DocumentSource::ApiUpload, &missing).unwrap();
    std::fs::remove_file(&missing).unwrap();
    let e = env.consumer.consume_file(doc, None, None).await.unwrap_err();
    assert!(e.to_string().ends_with("File not found."), "{e}");
    let f = env.put(&sample("pl-parsers/samples/binary_garbage.bin"), "garbage.bin");
    let e = env.consume(&f, Default::default()).await.unwrap_err();
    assert_eq!(e.to_string(), "garbage.bin: Unsupported mime type application/octet-stream");
    assert!(f.exists(), "unsupported file stays");

    // --- B: scanned image (OCR) -------------------------------------------------------------
    if env.has_ocr {
        let f = env.put(&repo().join("upstream/src/paperless/tests/samples/tesseract/simple.png"), "scan.png");
        let id_b = ok_id(env.consume(&f, Default::default()).await);
        let d = load(&pool, id_b).await;
        assert_eq!(d.mime, "image/png");
        assert!(d.content.to_lowercase().contains("this is a test document"), "{}", d.content);
        assert_eq!(d.archive_filename.as_deref(), Some(format!("{id_b:07}.pdf").as_str()));
        let arch = env.settings().archive_dir.join(d.archive_filename.unwrap());
        assert_eq!(pl_consumer::compute_checksum(&arch).ok(), d.archive_checksum);
        assert!(search(&env, "test document").contains(&id_b));
    } else {
        eprintln!("SKIP OCR case: tesseract/pdfium not available");
    }

    // --- Search index ----------------------------------------------------------------------------
    assert!(search(&env, "swisscom").contains(&id_f), "search finds consumed document");
    assert!(search(&env, "Einfache").contains(&id_c));
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_watcher_oneshot_subdirs_as_tags() {
    run_big(e2e_watcher_oneshot_subdirs_as_tags_impl);
}

async fn e2e_watcher_oneshot_subdirs_as_tags_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let env = make_env(pool.clone(), |s| {
        s.consumer_recursive = true;
        s.consumer_subdirs_as_tags = true;
    });
    let f = env.put_text("Projekt Q/notiz.txt", "Watcher-Notiz zzz 5501");
    env.put_text("Projekt Q/.DS_Store", "x");
    env.put_text("Projekt Q/ignored.xyz", "x");
    let consumer = Arc::new(env.consumer.clone());
    let w = pl_consumer::watcher::FolderWatcher {
        opts: pl_consumer::watcher::WatchOptions {
            directory: pl_consumer::watcher::FolderWatcher::check_directory(&env.consume_dir()).unwrap(),
            recursive: true,
            subdirs_as_tags: true,
            polling_interval: 0.0,
            stability_delay: 0.0,
            rescan_interval: std::time::Duration::from_secs(300),
        },
        filter: pl_consumer::watcher::ConsumerFilter::with_registry(&[], &[]),
        pool: pool.clone(),
        sink: Arc::new(pl_consumer::watcher::DirectSink(consumer)),
    };
    let (_tx, rx) = tokio::sync::watch::channel(false);
    w.run(true, rx).await;
    assert!(!f.exists(), "consumed");
    let tag: i32 = sqlx::query_scalar("select id from documents_tag where name = 'Projekt Q'").fetch_one(&pool).await.unwrap();
    let id: i32 =
        sqlx::query_scalar("select max(id) from documents_document where original_filename = 'notiz.txt'").fetch_one(&pool).await.unwrap();
    let tags: Vec<i32> =
        sqlx::query_scalar("select tag_id from documents_document_tags where document_id = $1").bind(id).fetch_all(&pool).await.unwrap();
    assert!(tags.contains(&tag));
    assert!(env.consume_dir().join("Projekt Q/.DS_Store").exists(), "ignored file untouched");
}

// ------------------------------------------------------------------------------------------------
// Concurrent consumption (several task slots, PAPERLESS_TASK_WORKERS > 1)
// ------------------------------------------------------------------------------------------------

/// Like [`run_big`], but a multi-threaded runtime (big worker stacks), so spawned consume tasks
/// really run in parallel and `blocking_io` takes the `block_in_place` path.
fn run_big_mt<F: std::future::Future<Output = ()>>(make: impl FnOnce() -> F + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(64 << 20)
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(make())
        })
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

fn nonce() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// Consumes all `files` at the same time (one tokio task each, no task id).
async fn consume_parallel(
    consumer: &Consumer,
    files: Vec<(PathBuf, DocumentMetadataOverrides)>,
) -> Vec<Result<ConsumeOutcome, pl_consumer::ConsumeError>> {
    let barrier = Arc::new(tokio::sync::Barrier::new(files.len()));
    let mut handles = Vec::new();
    for (path, o) in files {
        let c = consumer.clone();
        let b = barrier.clone();
        handles.push(tokio::spawn(async move {
            b.wait().await;
            let doc = ConsumableDocument::new(DocumentSource::ConsumeFolder, &path).unwrap();
            c.consume_file(doc, Some(o), None).await
        }));
    }
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.expect("consume task panicked"));
    }
    out
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_concurrent_same_asn() {
    run_big_mt(e2e_concurrent_same_asn_impl);
}

/// Four different documents with the same ASN at once: exactly one is stored, the others fail
/// with the AsnCheck message (not a raw unique-constraint error), the ASN exists once.
async fn e2e_concurrent_same_asn_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let env = make_env(pool.clone(), |_| {});
    let n = nonce();
    let asn = 1_500_000_000 + (n % 100_000_000) as i64;
    let files = (0..4)
        .map(|i| {
            let p = env.put_text(&format!("asn-race-{i}.txt"), &format!("asn race {n} document {i}"));
            (p, DocumentMetadataOverrides { asn: Some(asn), ..Default::default() })
        })
        .collect();
    let results = consume_parallel(&env.consumer, files).await;
    let ok = results.iter().filter(|r| matches!(r, Ok(ConsumeOutcome::Success { .. }))).count();
    assert_eq!(ok, 1, "{results:?}");
    for r in &results {
        if let Err(e) = r {
            let m = e.to_string();
            assert!(m.contains(&format!("Given ASN {asn} already exists")), "{m}");
        }
    }
    let count: i64 = sqlx::query_scalar("select count(*) from documents_document where archive_serial_number = $1")
        .bind(asn as i32)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_concurrent_duplicates() {
    run_big_mt(e2e_concurrent_duplicates_impl);
}

/// Four identical files at once with CONSUMER_DELETE_DUPLICATES: one document, three
/// duplicates (originals deleted) - never two documents with the same checksum.
async fn e2e_concurrent_duplicates_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let env = make_env(pool.clone(), |s| s.consumer_delete_duplicates = true);
    let content = format!("duplicate race {}", nonce());
    let files: Vec<_> =
        (0..4).map(|i| (env.put_text(&format!("dup-race-{i}.txt"), &content), DocumentMetadataOverrides::default())).collect();
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
    let checksum = pl_consumer::compute_checksum(&paths[0]).unwrap();
    let results = consume_parallel(&env.consumer, files).await;
    let ok = results.iter().filter(|r| matches!(r, Ok(ConsumeOutcome::Success { .. }))).count();
    let dup = results.iter().filter(|r| matches!(r, Ok(ConsumeOutcome::Duplicate { .. }))).count();
    assert_eq!((ok, dup), (1, 3), "{results:?}");
    let count: i64 =
        sqlx::query_scalar("select count(*) from documents_document where checksum = $1").bind(&checksum).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    assert!(paths.iter().all(|p| !p.exists()), "originals of stored and rejected files are gone");
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_concurrent_same_filename() {
    run_big_mt(e2e_concurrent_same_filename_impl);
}

/// Six documents whose FILENAME_FORMAT yields the same name, at once: all stored, every
/// original file name unique and present on disk (MEDIA_LOCK + unique-name search).
async fn e2e_concurrent_same_filename_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let n = nonce();
    let env = make_env(pool.clone(), |s| s.templating.filename_format = Some("race/{{ title }}".into()));
    let files = (0..6)
        .map(|i| {
            let p = env.put_text(&format!("name-race-{i}.txt"), &format!("filename race {n} document {i}"));
            (p, DocumentMetadataOverrides { title: Some(format!("Same title {n}")), ..Default::default() })
        })
        .collect();
    let results = consume_parallel(&env.consumer, files).await;
    let ids: Vec<i64> = results.into_iter().map(ok_id).collect();
    let mut names = std::collections::BTreeSet::new();
    for id in ids {
        let d = load(&pool, id).await;
        let f = d.filename.expect("filename");
        assert!(env.settings().originals_dir.join(&f).is_file(), "{f} on disk");
        assert!(names.insert(f.clone()), "filename {f} used twice");
    }
    assert_eq!(names.len(), 6);
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_concurrent_get_or_create_tag() {
    run_big_mt(e2e_concurrent_get_or_create_tag_impl);
}

/// Barcode tags / sub-directories as tags from several tasks at once: one tag, same id for all.
async fn e2e_concurrent_get_or_create_tag_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let name = format!("Race Tag {}", nonce());
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut handles = Vec::new();
    for i in 0..8 {
        let (pool, b) = (pool.clone(), barrier.clone());
        let name = if i % 2 == 0 { name.clone() } else { name.to_uppercase() };
        handles.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.unwrap();
            b.wait().await;
            pl_consumer::db::get_or_create_tag(&mut conn, &name).await.unwrap()
        }));
    }
    let mut ids = std::collections::BTreeSet::new();
    for h in handles {
        ids.insert(h.await.unwrap());
    }
    assert_eq!(ids.len(), 1, "{ids:?}");
    let count: i64 =
        sqlx::query_scalar("select count(*) from documents_tag where upper(name) = upper($1)").bind(&name).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_upload_into_folder() {
    run_big(e2e_upload_into_folder_impl);
}

const FOLDER_TABLES: &str = include_str!("../../pl-api-v2/migrations/0002_paperrust_folder.sql");

/// Folder view stage 2: the override `folder_id` (upload field `folder`) files the new document into that folder;
/// a folder deleted since the upload leaves it unsorted instead of failing the ingestion.
async fn e2e_upload_into_folder_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    sqlx::raw_sql(FOLDER_TABLES).execute(&pool).await.expect("folder tables");
    let env = make_env(pool.clone(), |_| {});
    let name = format!("e2e-folder-{}", nonce());
    let fid: i64 =
        sqlx::query_scalar("INSERT INTO paperrust_folder (name) VALUES ($1) RETURNING id").bind(&name).fetch_one(&pool).await.unwrap();
    let p = env.put_text(&format!("{name}.txt"), &format!("folder upload {}", nonce()));
    let id = ok_id(env.consume(&p, DocumentMetadataOverrides { folder_id: Some(fid), ..Default::default() }).await);
    let got: Option<i64> = sqlx::query_scalar("SELECT folder_id FROM paperrust_document_folder WHERE document_id = $1")
        .bind(id as i32)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(got, Some(fid));
    // unknown folder → stored, unsorted
    let p = env.put_text(&format!("{name}-gone.txt"), &format!("folder gone {}", nonce()));
    let id2 = ok_id(env.consume(&p, DocumentMetadataOverrides { folder_id: Some(i64::from(i32::MAX)), ..Default::default() }).await);
    let got: Option<i64> = sqlx::query_scalar("SELECT folder_id FROM paperrust_document_folder WHERE document_id = $1")
        .bind(id2 as i32)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(got, None);
    sqlx::query("DELETE FROM paperrust_document_folder WHERE folder_id = $1").bind(fid).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM paperrust_folder WHERE id = $1").bind(fid).execute(&pool).await.unwrap();
}

/// Scan-like PDF (one noisy RGB image, no text) above 1 MB, so that `PAPERRUST_SHRINK_ON_INGEST_MB=1`
/// shrinks it (same construction as pl-pdftools tests/common `scan_pdf`). `seed` makes the content unique.
fn scan_pdf(path: &Path, seed: u32) {
    use lopdf::content::{Content, Operation};
    use lopdf::{Dictionary, Document, Object, Stream, dictionary};
    let (w, h) = (1200u32, 1600u32);
    let mut raw = Vec::with_capacity((w * h * 3) as usize);
    let mut s = seed | 1;
    for y in 0..h {
        for x in 0..w {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((s >> 16) & 0x1f) as u8;
            raw.push(((x * 255 / w) as u8).saturating_add(n));
            raw.push(((y * 255 / h) as u8).saturating_add(n));
            raw.push(200u8.saturating_sub(n));
        }
    }
    let mut img = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        raw,
    );
    img.compress().unwrap();
    let mut doc = Document::with_version("1.5");
    let img_id = doc.add_object(img);
    let mut xo = Dictionary::new();
    xo.set("Im0", img_id);
    let ops = vec![
        Operation::new("q", vec![]),
        Operation::new("cm", vec![432.into(), 0.into(), 0.into(), 576.into(), 0.into(), 0.into()]),
        Operation::new("Do", vec!["Im0".into()]),
        Operation::new("Q", vec![]),
    ];
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(Dictionary::new(), Content { operations: ops }.encode().unwrap()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 432.into(), 576.into()],
        "Contents" => content_id, "Resources" => dictionary! { "XObject" => xo },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

/// Resets the settings overlay of this process when the test ends (also on panic).
struct OverlayReset;
impl Drop for OverlayReset {
    fn drop(&mut self) {
        pl_core::config::set_live_overlay(Default::default());
    }
}

#[test]
#[ignore = "braucht PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB-Kopie pl_consumer_test*)"]
fn e2e_shrunk_original_is_duplicate() {
    run_big_mt(e2e_shrunk_original_is_duplicate_impl);
}

/// PaperRust shrinking on ingest stores a smaller file (its checksum in `documents_document.checksum`,
/// as the sanity checker expects). Uploading the same ORIGINAL again must still be a duplicate:
/// sequentially, and with two identical originals at the same time (exactly one stored).
async fn e2e_shrunk_original_is_duplicate_impl() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let env = make_env(pool.clone(), |s| s.consumer_delete_duplicates = true);
    let _reset = OverlayReset;
    pl_core::config::set_live_overlay([("PAPERRUST_SHRINK_ON_INGEST_MB".to_string(), "1".to_string())].into_iter().collect());
    let src_dir = tempfile::tempdir().unwrap();
    let n = nonce();

    // 1) sequential: store, then the same original again
    let src = src_dir.path().join("scan.pdf");
    scan_pdf(&src, n as u32);
    assert!(std::fs::metadata(&src).unwrap().len() > 1024 * 1024, "test scan must exceed the 1 MB shrink limit");
    let original_sum = pl_consumer::compute_checksum(&src).unwrap();
    let id = ok_id(env.consume(&env.put(&src, "shrink-a.pdf"), DocumentMetadataOverrides::default()).await);
    let d = load(&pool, id).await;
    if d.checksum == original_sum {
        eprintln!("SKIP: file was not shrunk (paperrust-pdftool missing next to the test binary?)");
        return;
    }
    // stored checksum = stored file (sanity checker invariant)
    let stored = env.settings().originals_dir.join(d.filename.as_deref().unwrap());
    assert_eq!(pl_consumer::compute_checksum(&stored).unwrap(), d.checksum);
    let again = env.put(&src, "shrink-b.pdf");
    let r = env.consume(&again, DocumentMetadataOverrides::default()).await;
    assert!(matches!(r, Ok(ConsumeOutcome::Duplicate { .. })), "same original again must be a duplicate: {r:?}");
    assert!(!again.exists(), "duplicate original deleted");

    // 2) concurrent: two identical new originals at once -> one stored, one duplicate
    let src2 = src_dir.path().join("scan2.pdf");
    scan_pdf(&src2, (n as u32).wrapping_add(7919));
    let sum2 = pl_consumer::compute_checksum(&src2).unwrap();
    let files = (0..2).map(|i| (env.put(&src2, &format!("shrink-race-{i}.pdf")), DocumentMetadataOverrides::default())).collect();
    let results = consume_parallel(&env.consumer, files).await;
    let ok = results.iter().filter(|r| matches!(r, Ok(ConsumeOutcome::Success { .. }))).count();
    let dup = results.iter().filter(|r| matches!(r, Ok(ConsumeOutcome::Duplicate { .. }))).count();
    assert_eq!((ok, dup), (1, 1), "{results:?}");
    let count: i64 = sqlx::query_scalar(
        "select count(*) from documents_document d join paperrust_source_checksum s on s.document_id = d.id where s.checksum = $1",
    )
    .bind(&sum2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "one document for the original");
}
