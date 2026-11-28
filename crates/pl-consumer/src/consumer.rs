// Control of ingestion (Python-free replacement for the Celery task `consume_file`).
//
// upstream: src/documents/tasks.py@v3.4.2 (consume_file: plugin order, error handling),
//           src/documents/consumer.py@v3.4.2 (ConsumerPreflightPlugin, AsnCheckPlugin,
//           WorkflowTriggerPlugin, ConsumerPlugin, ConsumerStatusShortMessage, _store,
//           apply_overrides, _create_version_from_root, _parse_title_placeholders, _write),
//           src/documents/double_sided.py@v3.4.2 (CollatePlugin), src/documents/barcodes.py@v3.4.2
//           (BarcodePlugin), src/documents/signals/handlers.py@v3.4.2 (add_inbox_tags,
//           set_correspondent, set_document_type, set_tags, set_storage_path, run_workflows_added,
//           add_to_index, update_filename_and_move_files), src/documents/file_handling.py@v3.4.2
//
// For flow and texts see docs/contract/pl-consumer.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use pl_core::sqlbind::SqlxBinder as _;
use sea_query::{Expr, Order, PostgresQueryBuilder, Query};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};

use crate::audit::{self, AuditLog, CreatedAttr};
use crate::data::{ConsumableDocument, ConsumeOutcome, DocumentMetadataOverrides, StoredBarcode, absolute};
use crate::db::{self, DocRow, a, now_micros};
use crate::docops::{self, DocInstance};
use crate::native;
use crate::settings::{BarcodeAppConfig, BarcodeSettings, ConsumerSettings};
use crate::stages::*;
use crate::workflows;

/// E17: barcode raster with pdftoppm when PDFium is missing (file src/consumer/barcode_raster.rs).
pub mod barcode_raster;

/// `ConsumerStatusShortMessage`.
pub mod msg {
    pub const DOCUMENT_ALREADY_EXISTS: &str = "document_already_exists";
    pub const DOCUMENT_ALREADY_EXISTS_IN_TRASH: &str = "document_already_exists_in_trash";
    pub const ASN_ALREADY_EXISTS: &str = "asn_already_exists";
    pub const ASN_ALREADY_EXISTS_IN_TRASH: &str = "asn_already_exists_in_trash";
    pub const ASN_RANGE: &str = "asn_value_out_of_range";
    pub const FILE_NOT_FOUND: &str = "file_not_found";
    pub const PRE_CONSUME_SCRIPT_NOT_FOUND: &str = "pre_consume_script_not_found";
    pub const PRE_CONSUME_SCRIPT_ERROR: &str = "pre_consume_script_error";
    pub const POST_CONSUME_SCRIPT_NOT_FOUND: &str = "post_consume_script_not_found";
    pub const POST_CONSUME_SCRIPT_ERROR: &str = "post_consume_script_error";
    pub const NEW_FILE: &str = "new_file";
    pub const UNSUPPORTED_TYPE: &str = "unsupported_type";
    pub const PARSING_DOCUMENT: &str = "parsing_document";
    pub const GENERATING_THUMBNAIL: &str = "generating_thumbnail";
    pub const PARSE_DATE: &str = "parse_date";
    pub const SAVE_DOCUMENT: &str = "save_document";
    pub const FINISHED: &str = "finished";
    pub const FAILED: &str = "failed";
}

/// `Document.ARCHIVE_SERIAL_NUMBER_MIN/MAX`.
pub const ASN_MIN: i64 = 0;
pub const ASN_MAX: i64 = 0xFF_FF_FF_FF;
/// `Document.MAX_STORED_FILENAME_LENGTH`.
pub const MAX_STORED_FILENAME_LENGTH: usize = 1024;
/// `double_sided.TIMEOUT_SECONDS`, `STAGING_FILE_NAME`.
pub const COLLATE_TIMEOUT_SECONDS: u64 = 30 * 60;
pub const STAGING_FILE_NAME: &str = "double-sided-staging.pdf";

/// Ingestion error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsumeError {
    /// `ConsumerError(f"{filename}: {message}")` (also double-sided errors without prefix).
    #[error("{0}")]
    Consumer(String),
    /// `ConsumeFileDuplicateError` (only with `CONSUMER_DELETE_DUPLICATES`).
    #[error("{message}")]
    Duplicate { message: String, duplicate_id: i64, in_trash: bool },
    /// `StopConsumeTaskError`.
    #[error("{0}")]
    Stopped(String),
    /// Database/system error outside `_fail` (upstream: exception of the task).
    #[error("{0}")]
    Other(String),
    /// PaperRust (tests/chaos C1): the database was unreachable during ingestion. The
    /// input file stays in place, the task is retried (upstream: exception, upload gone).
    #[error("{0}")]
    Transient(String),
}

impl ConsumeError {
    /// Python class name (`PaperlessTask.result_data.error_type`).
    pub fn error_type(&self) -> &'static str {
        match self {
            ConsumeError::Consumer(_) => "ConsumerError",
            ConsumeError::Duplicate { .. } => "ConsumeFileDuplicateError",
            ConsumeError::Stopped(_) => "StopConsumeTaskError",
            ConsumeError::Other(_) => "Exception",
            ConsumeError::Transient(_) => "OperationalError",
        }
    }
}

/// Message when an ingestion task runs again later because of an unreachable database.
pub const MSG_DB_UNAVAILABLE_RETRY: &str =
    "Database, remote service or storage temporarily not available - the file is kept and will be processed again automatically.";

/// Error text that indicates a lost/unreachable database connection (sqlx texts:
/// `Error::Io` "error communicating with database", `PoolTimedOut`, `PoolClosed`, SQLSTATE 57P0x
/// or class 08 in the PostgreSQL message).
pub fn is_db_unavailable_text(msg: &str) -> bool {
    const MARKERS: &[&str] = &[
        "error communicating with database",
        "pool timed out while waiting for an open connection",
        "attempted to acquire a connection on a closed pool",
        "terminating connection due to",
        "the database system is starting up",
        "the database system is shutting down",
        "the database system is in recovery mode",
        "server closed the connection unexpectedly",
        "connection refused",
    ];
    let m = msg.to_ascii_lowercase();
    MARKERS.iter().any(|k| m.contains(k))
}

/// Error text of an unreachable external service (remote OCR, tests/chaos C4): connection setup
/// failed or timeout (reqwest texts). Then a later retry is worthwhile; HTTP
/// error responses of the service (wrong key, invalid document) do not count.
pub fn is_service_unreachable_text(msg: &str) -> bool {
    const MARKERS: &[&str] = &["error sending request", "operation timed out", "tcp connect error", "dns error", "connection reset"];
    let m = msg.to_ascii_lowercase();
    (m.contains("azure ai vision parsing failed") || m.contains("remote ocr")) && MARKERS.iter().any(|k| m.contains(k))
}

impl From<sqlx::Error> for ConsumeError {
    fn from(e: sqlx::Error) -> Self {
        ConsumeError::Other(e.to_string())
    }
}

/// All building blocks of ingestion.
#[derive(Clone)]
pub struct Consumer {
    pub pool: PgPool,
    pub settings: Arc<ConsumerSettings>,
    pub parsing: Arc<dyn DocumentParsing>,
    pub dates: Arc<dyn DateDetector>,
    pub classifier: Arc<dyn ClassifierSource>,
    pub index: Arc<dyn SearchIndex>,
    pub progress: Arc<dyn ProgressSink>,
    pub barcodes: Arc<dyn BarcodeScanner>,
    pub pages: Arc<dyn PdfPageOps>,
    pub effects: Arc<dyn WorkflowEffects>,
    pub scripts: Arc<dyn ScriptRunner>,
    /// For barcode splitting (new `consume_file` tasks) and deferred index updates.
    pub queue: Option<Arc<dyn pl_queue::TaskQueue>>,
    /// Remote OCR by default (`RemoteOCRConfig().remote_ocr_by_default`).
    pub remote_ocr_by_default: bool,
    /// WebSocket events `document_updated` (worker tasks on existing documents).
    pub events: Option<pl_queue::EventBus>,
    /// `clear_document_caches` (Django cache in Redis; the worker plugs in the implementation).
    pub caches: Arc<dyn crate::signals::DocumentCaches>,
    /// `document.delete()` of the workflow action "move to trash" for existing documents.
    pub deleter: Arc<dyn workflows::DocumentDeleter>,
    /// PaperRust: AI text recognition (pl-core `vision`, implementation from pl-ai, attached by
    /// pl-server). `None` = unchanged behavior.
    pub vision: Option<Arc<dyn pl_core::vision::VisionOcr>>,
}

impl Consumer {
    /// Standard setup with the PaperRust building blocks; OCR configuration from environment + DB.
    pub async fn from_env(pool: PgPool, events: pl_queue::EventBus) -> Result<Self, String> {
        let settings = ConsumerSettings::from_env().map_err(|e| e.0)?;
        let ocr_settings = pl_ocr::OcrSettings::from_process_env().map_err(|e| e.to_string())?;
        let app = load_app_config(&pool).await.map_err(|e| e.to_string())?;
        let ocr = pl_ocr::OcrConfig::new(&ocr_settings, &app).map_err(|e| e.to_string())?;
        let remote = pl_ocr::config::RemoteOcrConfig::new(&ocr_settings, &app).map_err(|e| e.to_string())?;
        let dates = native::RegexDates::from_settings(&settings, &ocr.language);
        let mut parsing = native::NativeParsing::new(ocr, settings.time_zone);
        parsing.pool = Some(pool.clone());
        let barcodes: Arc<dyn BarcodeScanner> = match &parsing.pdfium {
            Some(p) => Arc::new(native::PdfOpsBarcodes::new(Arc::new(p.clone()))),
            // E17: without PDFium rasterize with pdftoppm (like upstream pdf2image)
            None => match barcode_raster::PdftoppmRenderer::detect(&settings.scratch_dir) {
                Some(r) => Arc::new(native::PdfOpsBarcodes::new(Arc::new(r))),
                None => Arc::new(native::NoBarcodes),
            },
        };
        let by_default = remote.remote_ocr_by_default();
        // Remote OCR parser (Azure) like upstream `register_defaults`; invisible without a valid engine.
        crate::remote::install(&mut parsing.registry, &remote);
        parsing.remote = Some(remote);
        let classifier = native::FileClassifier::new(
            settings.classifier_model_file.clone(),
            settings.secret_key.clone().into_bytes(),
            settings.classifier_match_threshold,
        );
        let index: Arc<dyn SearchIndex> = match native::TantivyIndex::open(pl_indexer::IndexerConfig::from_env()) {
            Ok(i) => Arc::new(i),
            Err(e) => {
                tracing::warn!(target: "paperless.consumer", "Search index not available: {e}");
                Arc::new(native::NoIndex)
            }
        };
        Ok(Self {
            pool,
            settings: Arc::new(settings),
            parsing: Arc::new(parsing),
            dates: Arc::new(dates),
            classifier: Arc::new(classifier),
            index,
            progress: Arc::new(native::EventBusProgress { bus: events.clone() }),
            barcodes,
            pages: Arc::new(native::LopdfPages),
            effects: Arc::new(native::LoggingEffects),
            scripts: Arc::new(native::CommandScripts),
            queue: None,
            remote_ocr_by_default: by_default,
            events: Some(events),
            caches: Arc::new(crate::signals::NoCaches),
            deleter: Arc::new(workflows::NativeSoftDelete),
            vision: None,
        })
    }

    /// PaperRust: AI transcription ("KI-Abschrift", pl-core `vision`, docs/OCR-NEXT.md 2.8) after the
    /// normal recognition. The service decides mode/sensitivity/limits (`VisionOcr::plan`); pl-ocr
    /// `uncertain` selects whole pages and/or uncertain line crops from the classic OCR line data
    /// (`ocr-lines.json` in `parser_dir`, written by the OCR pipeline; without it the old rule "fewer
    /// than 50 characters per page" applies to the whole document). Accepted texts replace the page
    /// (whole pages) or the line at its place (crops); loops/token-limit hits keep the classic text.
    /// Without line data, whole-page texts only replace the content if every page was accepted.
    ///
    /// Returns the transcript (for `VisionOcr::record` after storing) if anything was sent; `text` is
    /// changed only if AI text went into it (`Transcript::accepted() > 0`). Errors are logged, never
    /// passed on. Page images: original for PDF/PNG/JPEG/WebP, otherwise archive PDF; crops preferably
    /// from the archive PDF (rotation/deskew as the OCR saw it).
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_vision(
        &self,
        requested: bool,
        path: &Path,
        mime_type: &str,
        archive: Option<&Path>,
        parser_dir: Option<&Path>,
        text: &mut Option<String>,
        page_count: Option<i64>,
    ) -> Option<pl_core::vision::Transcript> {
        use pl_core::vision::Transcript;
        let vision = self.vision.clone()?;
        let plan = vision.plan(requested).await?;
        let chars = pl_core::vision::text_chars(text.as_deref());
        let pages = page_count.and_then(|p| usize::try_from(p).ok()).filter(|p| *p > 0).unwrap_or(1);
        let lines = parser_dir.and_then(pl_ocr::uncertain::read_for_dir);
        let sel = pl_ocr::uncertain::select(plan.mode, plan.sensitivity, lines.as_ref(), chars, pages, plan.max_pages);
        if sel.is_empty() {
            return None;
        }
        if sel.skipped_pages > 0 {
            tracing::info!(target: "paperless.consumer", "AI transcription: {} page(s) over the page limit not sent", sel.skipped_pages);
        }
        let direct = matches!(mime_type, "application/pdf" | "image/png" | "image/jpeg" | "image/webp");
        let archive_file = archive.filter(|a| a.is_file()).map(Path::to_path_buf);
        let (page_src, page_mime) = match &archive_file {
            Some(a) if !direct => (a.clone(), "application/pdf".to_string()),
            _ => (path.to_path_buf(), mime_type.to_string()),
        };
        let (crop_src, crop_mime) = match &archive_file {
            Some(a) => (a.clone(), "application/pdf".to_string()),
            None => (path.to_path_buf(), mime_type.to_string()),
        };
        let parsing = self.parsing.clone();
        let (sel_c, lines_c) = (sel.clone(), lines.clone());
        let built = tokio::task::spawn_blocking(move || {
            let src = pl_ocr::uncertain::RequestSources { pages: (&page_src, &page_mime), crops: (&crop_src, &crop_mime) };
            parsing.vision_request(&src, &sel_c, lines_c.as_ref(), plan.max_side)
        })
        .await;
        let request = match built {
            Ok(Ok(r)) if !(r.pages.is_empty() && r.crops.is_empty()) => r,
            Ok(Ok(_)) => {
                tracing::info!(target: "paperless.consumer", "AI transcription skipped: no page images for {mime_type}");
                return None;
            }
            Ok(Err(e)) => {
                tracing::warn!(target: "paperless.consumer", "AI transcription skipped: page images failed: {e}");
                return None;
            }
            Err(e) => {
                tracing::warn!(target: "paperless.consumer", "AI transcription skipped: {e}");
                return None;
            }
        };
        let (n_pages, n_crops) = (request.pages.len(), request.crops.len());
        let resp = match vision.recognize(request).await {
            Ok(Some(r)) => r,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(target: "paperless.consumer", "AI transcription failed, keeping the OCR text: {e}");
                return None;
            }
        };
        if resp.remote {
            tracing::info!(target: "paperless.consumer", "AI transcription: page images/crops were sent to a non-local service");
        }
        let (entries, merged) = merge_vision_result(lines.as_ref(), &resp.items, pages);
        if let Some(t) = merged {
            *text = Some(t);
        }
        let transcript = Transcript { model: resp.model, mode: plan.mode, entries };
        let new_chars = pl_core::vision::text_chars(text.as_deref());
        tracing::info!(
            target: "paperless.consumer",
            "AI transcription: {n_pages} page(s), {n_crops} crop(s) sent, {} accepted ({chars} -> {new_chars} characters)",
            transcript.accepted()
        );
        Some(transcript)
    }

    /// PaperRust: keeps the AI transcript of a stored document (`VisionOcr::record`).
    pub async fn record_vision(&self, document_id: i64, transcript: Option<pl_core::vision::Transcript>) {
        if let (Some(v), Some(t)) = (self.vision.clone(), transcript) {
            v.record(document_id, t).await;
        }
    }

    /// Ingestion: document ID or error (duplicate/abort as error).
    pub async fn consume(&self, input: ConsumableDocument, overrides: DocumentMetadataOverrides) -> Result<i64, ConsumeError> {
        match self.consume_file(input, Some(overrides), None).await? {
            ConsumeOutcome::Success { document_id } => Ok(document_id),
            ConsumeOutcome::Stopped { reason } => Err(ConsumeError::Stopped(reason)),
            ConsumeOutcome::Duplicate { duplicate_of, duplicate_in_trash } => Err(ConsumeError::Duplicate {
                message: format!("duplicate of #{duplicate_of}"),
                duplicate_id: duplicate_of,
                in_trash: duplicate_in_trash,
            }),
        }
    }

    /// `tasks.consume_file(input_doc, overrides)`. An ingestion run has its own
    /// lookup store ([`crate::lookup`]) for immutable names and ids.
    pub async fn consume_file(
        &self,
        input: ConsumableDocument,
        overrides: Option<DocumentMetadataOverrides>,
        task_id: Option<String>,
    ) -> Result<ConsumeOutcome, ConsumeError> {
        let original = input.original_file.clone();
        let status_filename = overrides.as_ref().and_then(|o| o.filename.clone()).unwrap_or_else(|| input.file_name());
        let progress_task = task_id.clone();
        let r = Box::pin(crate::lookup::scope(self.consume_file_inner(input, overrides, task_id))).await;
        // tests/chaos C1: database gone (connection dropped, pool timeout) → do NOT
        // delete the file, but retry; otherwise an upload would be lost on a DB restart
        // (the retry no longer found the spool file: "File not found").
        if let Err(e) = &r
            && self.is_transient_failure(e).await
        {
            tracing::warn!(
                target: "paperless.consumer",
                "Database, remote service or storage not available while consuming {status_filename} ({e}); keeping the file, the task will be retried"
            );
            self.progress.send(ProgressUpdate {
                filename: Some(status_filename),
                task_id: progress_task,
                status: ProgressStatus::Working,
                message: MSG_DB_UNAVAILABLE_RETRY.to_string(),
                current_progress: 0,
                max_progress: 100,
                document_id: None,
                perms: ViewPermissions::default(),
            });
            return Err(ConsumeError::Transient(format!("{MSG_DB_UNAVAILABLE_RETRY} ({e})")));
        }
        cleanup_upload_spool(&self.settings.scratch_dir, &original);
        r
    }

    /// Whether a failure was due to the (temporarily) missing database: error text of a
    /// dropped connection or the database is not answering right now (short test, max. 5 s).
    async fn is_transient_failure(&self, e: &ConsumeError) -> bool {
        let msg = match e {
            ConsumeError::Consumer(m) | ConsumeError::Other(m) | ConsumeError::Transient(m) => m,
            ConsumeError::Duplicate { .. } | ConsumeError::Stopped(_) => return false,
        };
        if is_db_unavailable_text(msg) || is_service_unreachable_text(msg) || msg.contains(MSG_STORAGE_FAILED) {
            return true;
        }
        let ping = sqlx::query("SELECT 1").execute(&self.pool);
        !matches!(tokio::time::timeout(Duration::from_secs(5), ping).await, Ok(Ok(_)))
    }

    async fn consume_file_inner(
        &self,
        input: ConsumableDocument,
        overrides: Option<DocumentMetadataOverrides>,
        task_id: Option<String>,
    ) -> Result<ConsumeOutcome, ConsumeError> {
        let overrides = overrides.unwrap_or_default();
        let status_filename = overrides.filename.clone().unwrap_or_else(|| input.file_name());
        std::fs::create_dir_all(&self.settings.scratch_dir).map_err(|e| ConsumeError::Other(e.to_string()))?;
        let tmp = TempDir::new(&self.settings.scratch_dir, "tmp").map_err(|e| ConsumeError::Other(e.to_string()))?;
        // BarcodeConfig() per plugin instance: environment + ApplicationConfiguration (DB takes precedence).
        let barcodes = match load_barcode_app_config(&self.pool).await {
            Ok(c) => self.settings.barcodes.with_app_config(&c),
            Err(e) => {
                tracing::warn!(target: "paperless.barcodes", "Could not read application configuration, using environment: {e}");
                self.settings.barcodes.clone()
            }
        };
        let mut st = State {
            input,
            metadata: overrides,
            tmp_dir: tmp.path.clone(),
            task_id,
            status_filename,
            barcodes,
            checksum: None,
            shrunk: false,
        };
        // FINDINGS B7: redelivered task (crash/hard abort after the commit of the document,
        // before the task completed) – the document already exists: report as successful instead of
        // ingesting the (meanwhile deleted) file again.
        if let Some(task_id) = st.task_id.clone() {
            match task_document(&self.pool, &task_id).await {
                Ok(Some(doc_id)) => {
                    // saved: noted files belong to the document
                    self.drop_pending_files(&task_id);
                    return Ok(self.finish_redelivered(&st, &task_id, doc_id).await);
                }
                Ok(None) => self.resolve_pending_files(&task_id).await,
                Err(e) => tracing::warn!(target: "paperless.tasks", "Could not check task {task_id} for an already stored document: {e}"),
            }
        }
        let is_version = st.input.root_document_id.is_some();
        let plugins: &[Plugin] = if is_version {
            &[Plugin::Preflight, Plugin::Consumer]
        } else {
            &[
                Plugin::Preflight,
                Plugin::AsnCheck,
                Plugin::Collate,
                Plugin::Barcode,
                Plugin::AsnCheck,
                Plugin::WorkflowTrigger,
                Plugin::Consumer,
            ]
        };
        let mut result = None;
        for p in plugins {
            if !self.able_to_run(*p, &st) {
                tracing::debug!(target: "paperless.tasks", "Skipping plugin {}", p.name());
                continue;
            }
            tracing::debug!(target: "paperless.tasks", "Executing plugin {}", p.name());
            let r = match p {
                Plugin::Preflight => self.preflight(&mut st).await.map(|_| None),
                Plugin::AsnCheck => self.asn_check(&mut st).await.map(|_| None),
                Plugin::Collate => self.collate(&mut st).await,
                Plugin::Barcode => self.barcode(&mut st).await,
                Plugin::WorkflowTrigger => self.workflow_trigger(&mut st).await,
                Plugin::Consumer => self.run_consumer(&mut st).await.map(|id| {
                    result = Some(id);
                    None
                }),
            };
            match r {
                Ok(Some(m)) => tracing::info!(target: "paperless.tasks", "{} completed with: {m}", p.name()),
                Ok(None) => tracing::info!(target: "paperless.tasks", "{} completed with no message", p.name()),
                Err(ConsumeError::Stopped(reason)) => {
                    tracing::info!(target: "paperless.tasks", "{} requested task exit: {reason}", p.name());
                    return Ok(ConsumeOutcome::Stopped { reason });
                }
                Err(ConsumeError::Duplicate { message, duplicate_id, in_trash }) => {
                    tracing::info!(target: "paperless.tasks", "{} rejected duplicate: {message}", p.name());
                    return Ok(ConsumeOutcome::Duplicate { duplicate_of: duplicate_id, duplicate_in_trash: in_trash });
                }
                Err(e) => {
                    tracing::error!(target: "paperless.tasks", "{} failed: {e}", p.name());
                    self.progress.send(ProgressUpdate {
                        filename: Some(st.status_filename.clone()),
                        task_id: st.task_id.clone(),
                        status: ProgressStatus::Failed,
                        message: e.to_string(),
                        current_progress: 100,
                        max_progress: 100,
                        document_id: None,
                        perms: ViewPermissions::default(),
                    });
                    return Err(e);
                }
            }
        }
        match result {
            Some(id) => Ok(ConsumeOutcome::Success { document_id: id }),
            None => Err(ConsumeError::Other("consumer plugin did not run".into())),
        }
    }

    /// Note file for the files of a save attempt with an uncertain outcome (tests/chaos C1-3).
    fn pending_files_path(&self, task_id: &str) -> PathBuf {
        let safe: String = task_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.settings.scratch_dir.join(format!(".pending-{safe}.json"))
    }

    fn write_pending_files(&self, task_id: &str, written: &[PathBuf]) -> std::io::Result<()> {
        let p = self.pending_files_path(task_id);
        let mut all: Vec<String> = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()).unwrap_or_default();
        for w in written {
            let s = w.to_string_lossy().into_owned();
            if !all.contains(&s) {
                all.push(s);
            }
        }
        std::fs::create_dir_all(&self.settings.scratch_dir)?;
        std::fs::write(&p, serde_json::to_vec(&all).unwrap_or_default())
    }

    fn drop_pending_files(&self, task_id: &str) {
        let _ = std::fs::remove_file(self.pending_files_path(task_id));
    }

    /// The document of an earlier attempt was not saved: delete the noted files,
    /// unless a document refers to them (original/archive by file name; thumbnails carry the
    /// id of the unsaved document).
    async fn resolve_pending_files(&self, task_id: &str) {
        let p = self.pending_files_path(task_id);
        let Some(files) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()) else {
            return;
        };
        let mut entries = Vec::with_capacity(files.len());
        for f in &files {
            let path = PathBuf::from(f);
            let rel = path
                .strip_prefix(&self.settings.originals_dir)
                .or_else(|_| path.strip_prefix(&self.settings.archive_dir))
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"));
            let check = match rel {
                Some(r) => {
                    match sqlx::query_scalar::<_, i32>(
                        "SELECT 1 FROM documents_document WHERE filename = $1 OR archive_filename = $1 LIMIT 1",
                    )
                    .bind(&r)
                    .fetch_optional(&self.pool)
                    .await
                    {
                        Ok(Some(_)) => PendingCheck::Referenced,
                        Ok(None) => PendingCheck::Unreferenced,
                        Err(e) => {
                            tracing::warn!(target: "paperless.consumer", "Task {task_id}: could not check noted file {f}: {e}");
                            PendingCheck::Unknown
                        }
                    }
                }
                None => PendingCheck::Unreferenced,
            };
            entries.push((f.clone(), check));
        }
        let (removed, remaining) = blocking_io(|| remove_unreferenced(&entries));
        tracing::info!(
            target: "paperless.consumer",
            "Task {task_id}: previous attempt was not stored; removed {removed} of {} noted file(s), {} kept for a later retry",
            files.len(),
            remaining.len()
        );
        if remaining.is_empty() {
            let _ = std::fs::remove_file(&p);
        } else {
            match serde_json::to_vec(&remaining) {
                Ok(b) => {
                    if let Err(e) = std::fs::write(&p, b) {
                        tracing::warn!(target: "paperless.consumer", "Task {task_id}: could not update the note of pending files: {e}");
                    }
                }
                Err(e) => tracing::warn!(target: "paperless.consumer", "Task {task_id}: could not update the note of pending files: {e}"),
            }
        }
    }

    /// FINDINGS B7: completion of a redelivered task whose document is already saved:
    /// catch up on what can still be missing after the commit (delete original, search index), report success.
    /// The post-consumption script does not run again (whether it already ran is unknown).
    async fn finish_redelivered(&self, st: &State, task_id: &str, doc_id: i64) -> ConsumeOutcome {
        tracing::warn!(
            target: "paperless.tasks",
            "Task {task_id} was delivered again, but document {doc_id} from it is already stored; not consuming {} again",
            st.filename()
        );
        // The path may meanwhile hold a NEW file with the same name (consume folder): delete only
        // when its bytes are those of the stored document, otherwise leave it to the watcher.
        let path = st.input.original_file.clone();
        if path.is_file() {
            let stored = match stored_checksums(&self.pool, doc_id).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(target: "paperless.consumer", "Could not read the checksum of document {doc_id}: {e}");
                    Vec::new()
                }
            };
            let p = path.clone();
            match blocking_io(move || redelivered_original_action(&p, &stored)) {
                RedeliveredOriginal::Delete => {
                    tracing::debug!(target: "paperless.consumer", "Deleting original file {}", path.display());
                    if let Err(e) = std::fs::remove_file(&path)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(target: "paperless.consumer", "Could not delete original file {}: {e}", path.display());
                    }
                }
                RedeliveredOriginal::Missing => {}
                RedeliveredOriginal::Keep(why) => tracing::info!(
                    target: "paperless.consumer",
                    "Not deleting {}: {why}; it is consumed as a new document",
                    path.display()
                ),
            }
        }
        self.add_to_index(doc_id).await;
        self.send_progress(st, 100, 100, ProgressStatus::Success, msg::FINISHED, Some(doc_id));
        ConsumeOutcome::Success { document_id: doc_id }
    }

    fn able_to_run(&self, p: Plugin, st: &State) -> bool {
        match p {
            Plugin::Collate => {
                self.settings.collate_double_sided
                    && st
                        .input
                        .original_file
                        .components()
                        .any(|c| c.as_os_str().to_string_lossy() == self.settings.collate_double_sided_subdir_name)
            }
            Plugin::Barcode => st.barcodes.able_to_run(&st.input.mime_type) && self.barcodes.available(),
            _ => true,
        }
    }

    // ------------------------------------------------------------------------------------------
    // Hilfen (ConsumerPluginMixin)
    // ------------------------------------------------------------------------------------------

    fn send_progress(&self, st: &State, current: i64, max: i64, status: ProgressStatus, message: &str, document_id: Option<i64>) {
        self.progress.send(ProgressUpdate {
            filename: Some(st.status_filename.clone()),
            task_id: st.task_id.clone(),
            status,
            message: message.to_string(),
            current_progress: current,
            max_progress: max,
            document_id,
            perms: ViewPermissions {
                owner_id: st.metadata.owner_id.filter(|o| *o != 0),
                users_can_view: st.metadata.users_can_view(),
                groups_can_view: st.metadata.groups_can_view(),
            },
        });
    }

    /// `_fail(message, log_message)`.
    fn fail(&self, st: &State, message: &str, log_message: Option<&str>) -> ConsumeError {
        self.send_progress(st, 100, 100, ProgressStatus::Failed, message, None);
        let lm = log_message.unwrap_or(message);
        tracing::error!(target: "paperless.consumer", "{lm}");
        ConsumeError::Consumer(format!("{}: {lm}", st.filename()))
    }

    // ------------------------------------------------------------------------------------------
    // ConsumerPreflightPlugin
    // ------------------------------------------------------------------------------------------

    async fn preflight(&self, st: &mut State) -> Result<(), ConsumeError> {
        self.send_progress(st, 0, 100, ProgressStatus::Started, msg::NEW_FILE, None);
        // pre_check_file_exists
        if !st.input.original_file.is_file() {
            return Err(self.fail(
                st,
                msg::FILE_NOT_FOUND,
                Some(&format!("Cannot consume {}: File not found.", st.input.original_file.display())),
            ));
        }
        // pre_check_duplicate
        let original = st.input.original_file.clone();
        let checksum = blocking_io(|| compute_checksum(&original)).map_err(|e| ConsumeError::Other(e.to_string()))?;
        // PaperRust side table of the duplicate check (shrunk files): create once per process.
        crate::source_checksum::ensure_pool(&self.pool).await?;
        let mut conn = self.pool.acquire().await?;
        let dups = db::find_duplicates(&mut conn, &checksum).await?;
        drop(conn);
        st.checksum = Some(checksum);
        if !dups.is_empty() {
            Self::warn_duplicate(st, &dups);
            if self.settings.consumer_delete_duplicates {
                return Err(self.reject_duplicate(st, &dups));
            }
        }
        // pre_check_directories
        self.settings.ensure_directories().map_err(|e| ConsumeError::Other(e.to_string()))?;
        Ok(())
    }

    /// Warning of `pre_check_duplicate` ("Consuming duplicate ...").
    fn warn_duplicate(st: &State, dups: &[db::DuplicateRow]) {
        let any_trash = dups.iter().any(|d| d.in_trash);
        let mut log_msg = format!("Consuming duplicate {}: {} existing document(s) share the same content.", st.filename(), dups.len());
        if any_trash {
            log_msg.push_str(" Note: at least one existing document is in the trash.");
        }
        tracing::warn!(target: "paperless.consumer", "{log_msg}");
    }

    /// `pre_check_duplicate` with `CONSUMER_DELETE_DUPLICATES`: delete the original, report
    /// the duplicate. Also used when the re-check while storing finds a duplicate that a
    /// concurrent task stored after this task's preflight (see `claim_unique`).
    fn reject_duplicate(&self, st: &State, dups: &[db::DuplicateRow]) -> ConsumeError {
        let any_trash = dups.iter().any(|d| d.in_trash);
        let Some(first) = dups.first() else {
            return ConsumeError::Other("duplicate check without duplicates".into());
        };
        let label = Some(first.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| first.original_filename.clone().filter(|t| !t.is_empty()))
            .or_else(|| {
                first
                    .filename
                    .as_deref()
                    .filter(|f| !f.is_empty())
                    .and_then(|f| Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()))
                    .filter(|n| !n.is_empty())
            })
            .unwrap_or_else(|| first.id.to_string());
        let _ = std::fs::remove_file(&st.input.original_file);
        let mut failure = format!("Not consuming {}: It is a duplicate of {label} (#{})", st.filename(), first.id);
        let status = if any_trash {
            failure.push_str(" Note: existing document is in the trash.");
            msg::DOCUMENT_ALREADY_EXISTS_IN_TRASH
        } else {
            msg::DOCUMENT_ALREADY_EXISTS
        };
        self.send_progress(st, 100, 100, ProgressStatus::Failed, status, None);
        tracing::error!(target: "paperless.consumer", "{failure}");
        ConsumeError::Duplicate { message: format!("{}: {failure}", st.filename()), duplicate_id: first.id, in_trash: any_trash }
    }

    // ------------------------------------------------------------------------------------------
    // AsnCheckPlugin
    // ------------------------------------------------------------------------------------------

    async fn asn_check(&self, st: &mut State) -> Result<(), ConsumeError> {
        let Some(asn) = st.metadata.asn else { return Ok(()) };
        if !(ASN_MIN..=ASN_MAX).contains(&asn) {
            return Err(self.fail(
                st,
                msg::ASN_RANGE,
                Some(&format!(
                    "Not consuming {}: Given ASN {asn} is out of range [{}, {}]",
                    st.filename(),
                    thousands(ASN_MIN),
                    thousands(ASN_MAX)
                )),
            ));
        }
        let mut conn = self.pool.acquire().await?;
        if let Some((_, in_trash)) = db::find_asn(&mut conn, asn).await? {
            return Err(self.asn_exists(st, asn, in_trash));
        }
        Ok(())
    }

    /// Failure of `AsnCheckPlugin` ("Given ASN ... already exists!"); also used by the re-check
    /// while storing (`claim_unique`).
    fn asn_exists(&self, st: &State, asn: i64, in_trash: bool) -> ConsumeError {
        let mut log_msg = format!("Not consuming {}: Given ASN {asn} already exists!", st.filename());
        let m = if in_trash {
            log_msg.push_str(" Note: existing document is in the trash.");
            msg::ASN_ALREADY_EXISTS_IN_TRASH
        } else {
            msg::ASN_ALREADY_EXISTS
        };
        self.fail(st, m, Some(&log_msg))
    }

    // ------------------------------------------------------------------------------------------
    // CollatePlugin
    // ------------------------------------------------------------------------------------------

    async fn collate(&self, st: &mut State) -> Result<Option<String>, ConsumeError> {
        let s = &self.settings;
        let pdf_file = if st.input.mime_type == "application/pdf" {
            st.input.original_file.clone()
        } else if st.input.mime_type == "image/tiff" && s.collate_double_sided_tiff_support {
            let p = self.pages.tiff_to_pdf(&st.input.original_file, &st.tmp_dir).map_err(|e| ConsumeError::Other(e.to_string()))?;
            let _ = std::fs::remove_file(&st.input.original_file);
            p
        } else {
            return Err(ConsumeError::Consumer("Unsupported file type for collation of double-sided scans".into()));
        };
        let staging = s.scratch_dir.join(STAGING_FILE_NAME);
        let mut valid = false;
        if let Ok(meta) = std::fs::metadata(&staging) {
            let age = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).unwrap_or_default();
            if age > Duration::from_secs(COLLATE_TIMEOUT_SECONDS) {
                tracing::warn!(target: "paperless.double_sided", "Outdated double sided staging file exists, deleting it");
                let _ = std::fs::remove_file(&staging);
            } else {
                valid = true;
            }
        }
        if valid {
            let old = &st.input.original_file;
            let stem = old.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let renamed = old.with_file_name(format!("{stem}-collated.pdf"));
            let new_file: PathBuf =
                renamed.components().filter(|c| c.as_os_str().to_string_lossy() != s.collate_double_sided_subdir_name).collect();
            let r = (|| {
                if let Some(parent) = new_file.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| StageError::Other(e.to_string()))?;
                }
                self.pages.collate(&staging, &pdf_file, &new_file)
            })();
            let _ = std::fs::remove_file(&pdf_file);
            let _ = std::fs::remove_file(&staging);
            return match r {
                Ok(()) => {
                    tracing::info!(target: "paperless.double_sided", "Collated documents into new file {}", new_file.display());
                    Err(ConsumeError::Stopped("Success. Even numbered pages of double sided scan collated with odd pages".into()))
                }
                Err(e) => Err(ConsumeError::Consumer(e.to_string())),
            };
        }
        move_file(&pdf_file, &staging).map_err(|e| ConsumeError::Other(e.to_string()))?;
        if let Ok(f) = std::fs::File::options().write(true).open(&staging) {
            let _ = f.set_modified(SystemTime::now());
        }
        tracing::info!(
            target: "paperless.double_sided",
            "Got scan with odd numbered pages of double-sided scan, moved it to {}",
            staging.display()
        );
        Err(ConsumeError::Stopped(format!(
            "Received odd numbered pages of double sided scan, waiting up to {} minutes for even numbered pages",
            COLLATE_TIMEOUT_SECONDS / 60
        )))
    }

    // ------------------------------------------------------------------------------------------
    // BarcodePlugin (scanner via `BarcodeScanner`)
    // ------------------------------------------------------------------------------------------

    async fn barcode(&self, st: &mut State) -> Result<Option<String>, ConsumeError> {
        let b: BarcodeSettings = st.barcodes.clone();
        // setup(): own working directory (TemporaryDirectory(dir=base_tmp_dir, prefix="barcode"))
        let work = TempDir::new(&st.tmp_dir, "barcode").map_err(|e| ConsumeError::Other(e.to_string()))?;
        // maybe_override_pixel_limit + convert_from_tiff_to_pdf (errors abort as upstream)
        let pdf = if st.input.mime_type == "image/tiff" {
            self.pages.tiff_to_pdf(&st.input.original_file, &work.path).map_err(|e| ConsumeError::Other(e.to_string()))?
        } else {
            st.input.original_file.clone()
        };
        let codes = {
            let scanner = self.barcodes.clone();
            let (pdf, dir, scan) = (pdf.clone(), work.path.clone(), b.scan_settings());
            tokio::task::spawn_blocking(move || scanner.detect(&pdf, &dir, &scan)).await.map_err(|e| ConsumeError::Other(e.to_string()))?
        };
        let is_asn = |c: &Barcode| c.value.starts_with(&b.barcode_asn_prefix);
        let is_tag = |c: &Barcode| {
            b.barcode_tag_mapping.iter().any(|(p, _)| pl_matching::regex_compat::safe_regex_match(p, &c.value, true).is_some())
        };
        let mut separators: BTreeMap<i64, bool> = BTreeMap::new();
        if b.barcodes_enabled {
            let retain = b.barcode_retain_split_pages;
            for c in &codes {
                if c.value == b.barcode_string && (!retain || c.page > 0) {
                    separators.insert(c.page, retain);
                }
            }
            if b.barcode_enable_asn {
                for c in codes.iter().filter(|c| is_asn(c) && c.page != 0) {
                    separators.insert(c.page, true);
                }
            }
            if b.barcode_tag_split && b.barcode_enable_tag {
                for c in codes.iter().filter(|c| is_tag(c) && c.page != 0) {
                    separators.insert(c.page, true);
                }
            }
        }
        if b.barcode_enable_tag && (!b.barcode_tag_split || st.input.original_path.is_some() || separators.is_empty()) {
            let mut tags = Vec::new();
            let mut conn = self.pool.acquire().await?;
            for c in &codes {
                for raw in c.value.split(',') {
                    let mut tag_str: Option<String> = None;
                    for (pattern, sub) in &b.barcode_tag_mapping {
                        if pl_matching::regex_compat::safe_regex_match(pattern, raw, true).is_some() {
                            // safe_regex_sub returns None on error/timeout → no tag
                            tag_str = if sub.is_empty() {
                                Some(raw.to_string())
                            } else {
                                pl_matching::regex_compat::safe_regex_sub(pattern, sub, raw, true)
                            };
                            break;
                        }
                    }
                    if let Some(t) = tag_str.filter(|t| !t.is_empty()) {
                        match db::get_or_create_tag(&mut conn, &t).await {
                            Ok(id) => tags.push(id),
                            Err(e) => tracing::error!(target: "paperless.barcodes", "Failed to find or create TAG '{raw}' because: {e}"),
                        }
                    }
                }
            }
            if !tags.is_empty() {
                tracing::info!(target: "paperless.barcodes", "Found tags in barcode: {tags:?}");
                match st.metadata.tag_ids.as_mut() {
                    Some(t) if !t.is_empty() => t.extend(tags),
                    _ => st.metadata.tag_ids = Some(tags),
                }
            }
        }
        if !separators.is_empty() {
            let Some(queue) = &self.queue else {
                return Err(ConsumeError::Other("Barcode splitting requires a task queue".into()));
            };
            let pages: Vec<(i64, bool)> = separators.into_iter().collect();
            // fname = input_doc.original_file.stem; file times from the original
            let stem = st.input.original_file.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let parts = self
                .barcodes
                .split(&pdf, &pages, &work.path, &stem, &st.input.original_file)
                .map_err(|e| ConsumeError::Other(e.to_string()))?;
            let split_dir = TempDir::new(&self.settings.scratch_dir, "paperless-barcode-split-")
                .map_err(|e| ConsumeError::Other(e.to_string()))?
                .keep();
            for part in parts {
                let name = part.file_name().map(|n| n.to_owned()).unwrap_or_default();
                let target = split_dir.join(&name);
                // copy_file_with_basic_stats
                copy_with_stats(&part, &target).map_err(|e| ConsumeError::Other(e.to_string()))?;
                let mut doc = ConsumableDocument::new(st.input.source(), &target).map_err(|e| ConsumeError::Other(e.to_string()))?;
                doc.mailrule_id = st.input.mailrule_id;
                doc.original_path = Some(st.input.original_file.clone());
                crate::task::enqueue_consume(queue.as_ref(), &doc, &st.metadata).await.map_err(ConsumeError::Other)?;
            }
            let _ = std::fs::remove_file(&st.input.original_file);
            let m = "Barcode splitting complete!";
            self.progress.send(ProgressUpdate {
                filename: Some(st.status_filename.clone()),
                task_id: st.task_id.clone(),
                status: ProgressStatus::Success,
                message: m.into(),
                current_progress: 100,
                max_progress: 100,
                document_id: None,
                perms: ViewPermissions::default(),
            });
            return Err(ConsumeError::Stopped(m.into()));
        }
        if b.barcode_enable_asn
            && let Some(text) = codes.iter().find(|c| is_asn(c)).map(|c| c.value.clone())
        {
            let digits: String = text[b.barcode_asn_prefix.len()..].trim().chars().filter(|c| c.is_ascii_digit()).collect();
            match digits.parse::<i64>() {
                Ok(asn) => {
                    let mut conn = self.pool.acquire().await?;
                    if st.metadata.skip_asn_if_exists && db::find_asn(&mut conn, asn).await?.is_some() {
                        tracing::info!(target: "paperless.barcodes", "Found ASN in barcode {asn} but skipping because it already exists.");
                    } else {
                        tracing::info!(target: "paperless.barcodes", "Found ASN in barcode: {asn}");
                        st.metadata.asn = Some(asn);
                    }
                }
                Err(e) => tracing::warn!(target: "paperless.barcodes", "Failed to parse ASN number because: {e}"),
            }
        }
        if b.barcode_store_values {
            let v: Vec<StoredBarcode> =
                codes.iter().map(|c| StoredBarcode { page: c.page + 1, value: c.value.clone(), format: c.format.clone() }).collect();
            st.metadata.barcodes = if v.is_empty() { None } else { Some(v) };
        }
        drop(work);
        Ok(None)
    }

    // ------------------------------------------------------------------------------------------
    // WorkflowTriggerPlugin
    // ------------------------------------------------------------------------------------------

    async fn workflow_trigger(&self, st: &mut State) -> Result<Option<String>, ConsumeError> {
        let mut conn = self.pool.acquire().await?;
        let r = workflows::run_consumption(&mut conn, &st.input, self.effects.as_ref()).await?;
        st.metadata.update(&r.overrides);
        if let Some(reason) = r.stop {
            return Err(ConsumeError::Stopped(reason));
        }
        Ok(Some(r.message))
    }

    // ------------------------------------------------------------------------------------------
    // ConsumerPlugin
    // ------------------------------------------------------------------------------------------

    async fn run_consumer(&self, st: &mut State) -> Result<i64, ConsumeError> {
        let filename = st.filename();
        tracing::info!(target: "paperless.consumer", "Consuming {filename}");
        let work = TempDir::new(&self.settings.scratch_dir, "paperless-ngx").map_err(|e| ConsumeError::Other(e.to_string()))?;
        // F4 (security review 2026-11-19): name of the working copy sanitized for the file system –
        // on Windows `C:start.bat` otherwise left the working folder (drive-relative path).
        let work_name = working_file_name(&filename);
        let working_copy = work.path.join(&work_name);
        blocking_io(|| copy_with_stats(&st.input.original_file, &working_copy)).map_err(|e| ConsumeError::Other(e.to_string()))?;
        let mut mime_type = pl_parsers::mime::detect_file(&working_copy).map_err(|e| ConsumeError::Other(e.to_string()))?;
        tracing::debug!(target: "paperless.consumer", "Detected mime type: {mime_type}");
        // Unchanged original if qpdf repaired the working copy (it is saved and
        // used for the checksum; the repaired working copy is parsed).
        let mut unmodified_original: Option<PathBuf> = None;
        if Path::new(&filename).extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            && PDF_RECOVERABLE_MIME_TYPES.contains(&mime_type.as_str())
        {
            tracing::debug!(target: "paperless.consumer", "Detected possible PDF with wrong mime type, trying to clean with qpdf");
            match clean_with_qpdf(&working_copy) {
                Ok(()) => {
                    mime_type = pl_parsers::mime::detect_file(&working_copy).map_err(|e| ConsumeError::Other(e.to_string()))?;
                    tracing::debug!(target: "paperless.consumer", "Detected mime type after qpdf: {mime_type}");
                    let uo = work.path.join("uo").join(&work_name);
                    let r = std::fs::create_dir_all(work.path.join("uo")).and_then(|_| copy_with_stats(&st.input.original_file, &uo));
                    match r {
                        Ok(()) => unmodified_original = Some(uo),
                        Err(e) => tracing::error!(target: "paperless.consumer", "Error attempting to clean PDF: {e}"),
                    }
                }
                Err(e) => tracing::error!(target: "paperless.consumer", "Error attempting to clean PDF: {e}"),
            }
        }
        // PaperRust: repair broken PDFs, shrink large scans (pl-pdftools, child process;
        // docs/contract/pl-pdftools.md §5). Repair: the original stays the stored original.
        let is_pdf_name = Path::new(&filename).extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if mime_type == "application/pdf" || (is_pdf_name && PDF_RECOVERABLE_MIME_TYPES.contains(&mime_type.as_str())) {
            let o = crate::pdftools::prepare(&working_copy, &work.path).await;
            if o.repaired {
                mime_type = pl_parsers::mime::detect_file(&working_copy).map_err(|e| ConsumeError::Other(e.to_string()))?;
                if unmodified_original.is_none() {
                    let uo = work.path.join("uo").join(&work_name);
                    match std::fs::create_dir_all(work.path.join("uo")).and_then(|_| copy_with_stats(&st.input.original_file, &uo)) {
                        Ok(()) => unmodified_original = Some(uo),
                        Err(e) => tracing::error!(target: "paperless.consumer", "Could not keep the original of the repaired PDF: {e}"),
                    }
                }
            }
            if o.shrunk.is_some() {
                // the shrunk file is stored (purpose of the setting); the original's checksum is
                // recorded for the duplicate check (source_checksum.rs)
                unmodified_original = None;
                st.shrunk = true;
            }
        }
        let allow_remote = st.metadata.remote_ocr || self.remote_ocr_by_default;
        let Some(parser) = self.parsing.parser_for(&mime_type, &filename, &working_copy, allow_remote) else {
            return Err(self.fail(st, msg::UNSUPPORTED_TYPE, Some(&format!("Unsupported mime type {mime_type}"))));
        };
        if st.metadata.remote_ocr && !parser.uses_remote_service {
            tracing::warn!(
                target: "paperless.consumer",
                "Remote OCR was requested for this document but no remote parser is available for it, processing locally instead."
            );
        }
        self.run_pre_consume_script(st, &working_copy)?;
        tracing::debug!(target: "paperless.consumer", "Parser: {}", parser.name);

        // --- parse ---------------------------------------------------------------------------
        self.send_progress(st, 20, 100, ProgressStatus::Working, msg::PARSING_DOCUMENT, None);
        tracing::debug!(target: "paperless.consumer", "Parsing {filename}...");
        let parser_tmp = work.path.join("parser");
        std::fs::create_dir_all(&parser_tmp).map_err(|e| ConsumeError::Other(e.to_string()))?;
        let parsed = {
            let parsing = self.parsing.clone();
            let parser = parser.clone();
            let wc = working_copy.clone();
            let mt = mime_type.clone();
            let fname = filename.clone();
            let ptmp = parser_tmp.clone();
            let mailrule = st.input.mailrule_id;
            let progress = self.progress.clone();
            let base = ProgressUpdate {
                filename: Some(st.status_filename.clone()),
                task_id: st.task_id.clone(),
                status: ProgressStatus::Working,
                message: String::new(),
                current_progress: 0,
                max_progress: 100,
                document_id: None,
                perms: ViewPermissions {
                    owner_id: st.metadata.owner_id.filter(|o| *o != 0),
                    users_can_view: st.metadata.users_can_view(),
                    groups_can_view: st.metadata.groups_can_view(),
                },
            };
            let dates = self.dates.clone();
            tokio::task::spawn_blocking(move || -> Result<(ParsedOutput, PathBuf, Option<i64>, bool), StageError> {
                let req = ParseRequest { path: &wc, mime_type: &mt, filename: &fname, mailrule_id: mailrule, tempdir: &ptmp };
                let produce_archive = parsing.should_produce_archive(&parser, &mt, &wc);
                let mut out = parsing.parse(&parser, &req, produce_archive)?;
                tracing::debug!(target: "paperless.consumer", "Generating thumbnail for {fname}...");
                progress.send(ProgressUpdate {
                    current_progress: 70,
                    message: crate::consumer::msg::GENERATING_THUMBNAIL.into(),
                    ..base.clone()
                });
                let thumb = parsing.thumbnail_with_text(&parser, &req, out.archive_path.as_deref(), out.text.as_deref())?;
                let date_from_text = out.date.is_none();
                if date_from_text {
                    progress.send(ProgressUpdate {
                        current_progress: 90,
                        message: crate::consumer::msg::PARSE_DATE.into(),
                        ..base.clone()
                    });
                    out.date = dates.detect(&fname, out.text.as_deref().unwrap_or(""));
                }
                let pages = parsing.page_count(&parser, &req, out.archive_path.as_deref());
                Ok((out, thumb, pages, date_from_text))
            })
            .await
        };
        let (mut parsed, thumbnail, page_count, date_from_text) = match parsed {
            Ok(Ok(v)) => v,
            Ok(Err(StageError::Parse(e))) => {
                return Err(self.fail(st, &e, Some(&format!("Error occurred while consuming document {filename}: {e}"))));
            }
            Ok(Err(StageError::Other(e))) => {
                return Err(self.fail(st, &e, Some(&format!("Unexpected error while consuming document {filename}: {e}"))));
            }
            Err(join) => {
                let e = join.to_string();
                return Err(self.fail(st, &e, Some(&format!("Unexpected error while consuming document {filename}: {e}"))));
            }
        };

        // PaperRust: AI text recognition after the normal recognition (chosen, "always" or little text)
        let archive = parsed.archive_path.clone();
        let transcript = self
            .apply_vision(
                st.metadata.ai_ocr,
                &working_copy,
                &mime_type,
                archive.as_deref(),
                Some(&parser_tmp),
                &mut parsed.text,
                page_count,
            )
            .await;
        if transcript.as_ref().is_some_and(|t| t.accepted() > 0) && date_from_text {
            parsed.date = self.dates.detect(&filename, parsed.text.as_deref().unwrap_or(""));
        }

        let classifier = self.classifier.load();
        self.send_progress(st, 95, 100, ProgressStatus::Working, msg::SAVE_DOCUMENT, None);

        // --- store (one transaction) -------------------------------------------------------
        let mut written: Vec<PathBuf> = Vec::new();
        let source_file = unmodified_original.clone().unwrap_or_else(|| working_copy.clone());
        // Deadlock/serialization failure (40P01/40001): PostgreSQL rolled back the whole ingestion transaction
        // -> remove stored files and retry completely (s3 follow-up to B1).
        let mut attempt = 0u32;
        let stored = loop {
            attempt += 1;
            let r =
                self.store_all(st, &source_file, &mime_type, &parsed, &thumbnail, page_count, classifier.as_deref(), &mut written).await;
            match r {
                Err(StoreFail::Error(e)) if e.starts_with(RETRYABLE_MARK) && attempt < db::MAX_ATTEMPTS => {
                    tracing::warn!(target: "paperless.consumer", "Storing {filename} hit a database conflict ({}), retrying", &e[RETRYABLE_MARK.len()..]);
                    for p in written.drain(..) {
                        let _ = std::fs::remove_file(p);
                    }
                    tokio::time::sleep(db::retry_backoff(attempt)).await;
                }
                Err(StoreFail::Error(e)) if e.starts_with(STORAGE_MARK) => {
                    break Err(StoreFail::Error(format!("{MSG_STORAGE_FAILED}: {}", &e[STORAGE_MARK.len()..])));
                }
                Err(StoreFail::Error(e)) => break Err(StoreFail::Error(e.strip_prefix(RETRYABLE_MARK).map(str::to_string).unwrap_or(e))),
                Err(f) => break Err(f),
                Ok(v) => break Ok(v),
            }
        };
        let (doc_id, workflow_outcome) = match stored {
            Ok(v) => v,
            Err(f) => {
                // tests/chaos C1-3: if the connection broke at/before the COMMIT, it is open whether the document
                // was saved. Then do NOT delete the stored files (otherwise: document without
                // original/thumbnail), but note them; the next delivery decides
                // ([`Consumer::resolve_pending_files`]).
                let ambiguous = matches!(&f, StoreFail::Error(e) if is_db_unavailable_text(e)) && st.task_id.is_some();
                if ambiguous && let Some(task_id) = st.task_id.as_deref() {
                    if let Err(je) = self.write_pending_files(task_id, &written) {
                        tracing::warn!(target: "paperless.consumer", "Could not note files of {filename} for later cleanup: {je}");
                    }
                } else {
                    for p in &written {
                        let _ = std::fs::remove_file(p);
                    }
                }
                return Err(match f {
                    StoreFail::Error(e) => self.fail(
                        st,
                        &e,
                        Some(&format!("The following error occurred while storing document {filename} after parsing: {e}")),
                    ),
                    StoreFail::Duplicate(dups) => {
                        tracing::info!(target: "paperless.consumer", "{filename}: a concurrent task stored the same content meanwhile");
                        Self::warn_duplicate(st, &dups);
                        self.reject_duplicate(st, &dups)
                    }
                    StoreFail::AsnExists { asn, in_trash } => {
                        tracing::info!(target: "paperless.consumer", "{filename}: a concurrent task took ASN {asn} meanwhile");
                        self.asn_exists(st, asn, in_trash)
                    }
                });
            }
        };

        // document_consumption_finished receivers that registered at the ingestion trigger
        // (W1 "remove password"); the document is committed.
        self.effects.consumption_finished(doc_id, &st.input.original_file);
        // PaperRust: AI transcript (original OCR lines, "KI-Abschrift" label) for the committed document.
        self.record_vision(doc_id, transcript).await;

        // After the commit: delete original (upstream still in the transaction), index, script.
        tracing::debug!(target: "paperless.consumer", "Deleting original file {}", st.input.original_file.display());
        let _ = std::fs::remove_file(&st.input.original_file);
        let _ = std::fs::remove_file(&working_copy);
        if let Some(uo) = &unmodified_original {
            tracing::debug!(target: "paperless.consumer", "Deleting unmodified original file {}", uo.display());
            let _ = std::fs::remove_file(uo);
        }
        if let (Some(parent), Some(name)) = (st.input.original_file.parent(), st.input.original_file.file_name()) {
            let shadow = parent.join(format!("._{}", name.to_string_lossy()));
            if shadow.is_file() {
                tracing::debug!(target: "paperless.consumer", "Deleting shadow file {}", shadow.display());
                let _ = std::fs::remove_file(&shadow);
            }
        }
        self.add_to_index(doc_id).await;
        // apply_ai_suggestions.delay_on_commit(...) from the "document added" workflows
        workflows::enqueue_ai_suggestions(self.queue.as_deref(), &workflow_outcome).await;
        drop(work);

        self.run_post_consume_script(st, doc_id).await?;
        tracing::info!(target: "paperless.consumer", "Document {doc_id} consumption finished");
        self.send_progress(st, 100, 100, ProgressStatus::Success, msg::FINISHED, Some(doc_id));
        Ok(doc_id)
    }

    /// `add_to_index` (version → root document). Lock taken → `index_document` with countdown 60.
    async fn add_to_index(&self, doc_id: i64) {
        let target = match self.pool.acquire().await {
            Ok(mut c) => match db::load_document(&mut c, doc_id).await {
                Ok(Some(d)) => d.root_document_id.unwrap_or(doc_id),
                _ => doc_id,
            },
            Err(_) => doc_id,
        };
        let r = self.index.add_or_update(&self.pool, target).await;
        if let Err(e) = &r {
            // FINDINGS B8: do not lose silently – retry later as with a taken lock
            tracing::error!(target: "paperless.consumer", "Error while updating search index for {target}: {e}; retrying later");
        }
        match r {
            Ok(IndexOutcome::Deferred) | Err(_) => {
                if let Some(q) = &self.queue {
                    // taken lock: like upstream after 60 s; error (e.g. DB briefly gone): after 5 s
                    let countdown = if r.is_err() { 5.0 } else { 60.0 };
                    if let Err(e) =
                        q.enqueue("documents.tasks.index_document", Default::default(), index_retry_opts(target, countdown)).await
                    {
                        // tests/chaos c1 (overall check 2026-11-18): database gone → index *and* enqueue
                        // fail; without a retry the document remained unfindable until the next restart.
                        tracing::error!(target: "paperless.consumer", "Could not schedule index update for {target}: {e}; retrying in the background");
                        tokio::spawn(retry_schedule_index(q.clone(), target, INDEX_SCHEDULE_RETRY));
                    }
                } else {
                    tracing::warn!(target: "paperless.consumer", "Search index locked, document {target} not indexed");
                }
            }
            Ok(_) => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn store_all(
        &self,
        st: &State,
        working_copy: &Path,
        mime_type: &str,
        parsed: &ParsedOutput,
        thumbnail: &Path,
        page_count: Option<i64>,
        classifier: Option<&(dyn pl_matching::Classifier + Send + Sync)>,
        written: &mut Vec<PathBuf>,
    ) -> Result<(i64, workflows::RunOutcome), StoreFail> {
        let audit = AuditLog { enabled: self.settings.audit_log_enabled, actor_id: None };
        let mut outcome = workflows::RunOutcome::default();
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        self.claim_unique(&mut tx, st).await?;
        let text = parsed.text.clone().unwrap_or_default();
        let mut doc = if let Some(root_id) = st.input.root_document_id {
            self.create_version(&mut tx, st, root_id, &text, page_count, mime_type, working_copy).await?
        } else {
            self.store(&mut tx, st, &audit, &text, parsed.date, page_count, mime_type, working_copy).await?
        };
        // PaperRust: stored file is a shrunk copy -> keep the original's checksum for the duplicate
        // check (same transaction, visible together with the row; claim_unique holds the lock on it).
        if let Some(orig) = crate::source_checksum::needs_record(st.shrunk, st.checksum.as_deref(), &doc.row.checksum) {
            crate::source_checksum::record(&mut tx, doc.row.id, &orig).await.map_err(store_err)?;
        }
        // refetch (Document.objects.get(pk=...))
        let fresh = db::load_document(&mut tx, doc.row.id).await.map_err(store_err)?.ok_or("document vanished")?;
        doc = DocInstance::from_db(fresh);

        // document_consumption_finished
        if doc.row.root_document_id.is_none() {
            outcome = self.consumption_finished(&mut tx, &audit, &mut doc, classifier, working_copy).await.map_err(store_err)?;
        }

        // Store files (under MEDIA_LOCK)
        {
            let _lock = MediaLock::acquire(&self.settings.media_lock()).await.map_err(store_err)?;
            let fresh = db::load_document(&mut tx, doc.row.id).await.map_err(store_err)?.ok_or("document vanished")?;
            let created_attr = doc.created_attr.clone();
            doc = DocInstance { row: fresh, created_attr };
            let ctx = crate::templating::document_context(&mut tx, &doc.row).await.map_err(store_err)?;
            let originals = self.settings.originals_dir.clone();
            let opts = &self.settings.templating;
            let mut generated =
                pl_templating::generate_unique_filename(&ctx, false, opts, |rel| originals.join(rel).exists()).map_err(store_err)?;
            if generated.len() > MAX_STORED_FILENAME_LENGTH {
                tracing::warn!(target: "paperless.consumer", "Generated source filename exceeds db path limit, falling back to default naming");
                generated = pl_templating::generate_filename(
                    &ctx,
                    pl_templating::GenerateOptions { use_format: false, ..Default::default() },
                    opts,
                )
                .map_err(store_err)?;
            }
            let source_path = originals.join(&generated);
            pl_templating::validate_path_in_root(&source_path, &originals).map_err(|e| e.0)?;
            pl_templating::create_source_path_directory(&source_path).map_err(store_err)?;
            blocking_io(|| write_atomic(working_copy, &source_path, written)).map_err(store_err)?;
            doc.row.filename = Some(generated);
            let thumb_path = self.settings.thumbnail_path(doc.row.id);
            blocking_io(|| write_atomic(thumbnail, &thumb_path, written)).map_err(store_err)?;

            if let Some(archive) = parsed.archive_path.as_deref().filter(|p| p.is_file()) {
                let mut actx = ctx.clone();
                actx.filename.clone_from(&doc.row.filename);
                let archive_dir = self.settings.archive_dir.clone();
                let mut gen_a =
                    pl_templating::generate_unique_filename(&actx, true, opts, |rel| archive_dir.join(rel).exists()).map_err(store_err)?;
                if gen_a.len() > MAX_STORED_FILENAME_LENGTH {
                    tracing::warn!(target: "paperless.consumer", "Generated archive filename exceeds db path limit, falling back to default naming");
                    gen_a = pl_templating::generate_filename(
                        &actx,
                        pl_templating::GenerateOptions { archive_filename: true, use_format: false, ..Default::default() },
                        opts,
                    )
                    .map_err(store_err)?;
                }
                let archive_path = archive_dir.join(&gen_a);
                pl_templating::validate_path_in_root(&archive_path, &archive_dir).map_err(|e| e.0)?;
                pl_templating::create_source_path_directory(&archive_path).map_err(store_err)?;
                blocking_io(|| write_atomic(archive, &archive_path, written)).map_err(store_err)?;
                doc.row.archive_filename = Some(gen_a);
                doc.row.archive_checksum = Some(blocking_io(|| compute_checksum(&archive_path)).map_err(store_err)?);
            }
        }
        // document.save() → post_save update_filename_and_move_files
        docops::save(&mut tx, &audit, &mut doc, None).await.map_err(store_err)?;
        self.update_filename_and_move_files(&mut tx, &mut doc, written).await.map_err(store_err)?;
        if let Some(root) = doc.row.root_document_id {
            // W2: `document_updated.send(document=document.root_document, skip_ai_index=True)`
            // (consumer.py ~line 768) still in the ingestion transaction: "document
            // updated" workflows, WebSocket `document_updated`, then `post_save` of the root document
            // (file name). Errors abort ingestion as upstream. `apply_ai_suggestions`
            // from these workflows is enqueued by the caller after the commit.
            tracing::debug!(target: "paperless.consumer", "document_updated for root {root}");
            outcome = self.document_updated(&mut tx, root).await.map_err(store_err)?;
            crate::files::update_filename_and_move_files(&mut tx, &self.settings, root).await.map_err(store_err)?;
        }
        // FINDINGS B7: note on the task, in the same transaction, which document it created
        // – if the process aborts after the commit but before the task completes, the
        // redelivery recognizes the document (instead of "File not found" or a duplicate).
        if let Some(task_id) = st.task_id.as_deref() {
            record_task_document(&mut tx, task_id, doc.row.id).await.map_err(store_err)?;
        }
        // PaperRust: a parser limit cut the text short → system note (same transaction, indexed with it)
        if let Some(m) = parsed.truncation.as_deref() {
            crate::system_note::set_truncation(&mut tx, doc.row.id, m).await.map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok((doc.row.id, outcome))
    }

    /// Uniqueness re-check under transaction-scoped advisory locks, first thing in the store
    /// transaction. With several task slots, two tasks can both pass `AsnCheckPlugin` /
    /// `pre_check_duplicate` before either has stored (upstream with `PAPERLESS_TASK_WORKERS` > 1
    /// has the same window): the second would then fail on the unique ASN constraint with a raw
    /// database error, or - with `CONSUMER_DELETE_DUPLICATES` - store a duplicate. The lock is held
    /// until this transaction ends, so a waiting task sees the other's committed document and is
    /// rejected with the same message as the plugin. Fixed order (ASN, then checksum) and no other
    /// lock held yet in this transaction, so no lock cycles.
    /// The checksum key is that of the delivered file (`st.checksum`); a document whose stored file
    /// is a shrunk copy records exactly this checksum in `paperrust_source_checksum` in the same
    /// transaction, and `find_duplicates` matches it, so one lock key per task is enough.
    async fn claim_unique(&self, tx: &mut PgConnection, st: &State) -> Result<(), StoreFail> {
        if st.input.root_document_id.is_none()
            && let Some(asn) = st.metadata.asn
        {
            db::advisory_xact_lock(tx, &format!("paperrust:asn:{asn}")).await.map_err(store_err)?;
            if let Some((_, in_trash)) = db::find_asn(tx, asn).await.map_err(store_err)? {
                return Err(StoreFail::AsnExists { asn, in_trash });
            }
        }
        if self.settings.consumer_delete_duplicates
            && let Some(sum) = st.checksum.as_deref()
        {
            db::advisory_xact_lock(tx, &format!("paperrust:checksum:{sum}")).await.map_err(store_err)?;
            let dups = db::find_duplicates(tx, sum).await.map_err(store_err)?;
            if !dups.is_empty() {
                return Err(StoreFail::Duplicate(dups));
            }
        }
        Ok(())
    }

    /// `_store`.
    #[allow(clippy::too_many_arguments)]
    async fn store(
        &self,
        conn: &mut PgConnection,
        st: &State,
        audit: &AuditLog,
        text: &str,
        date: Option<DateTime<FixedOffset>>,
        page_count: Option<i64>,
        mime_type: &str,
        working_copy: &Path,
    ) -> Result<DocInstance, String> {
        tracing::debug!(target: "paperless.consumer", "Saving record to database");
        let tz = self.settings.time_zone;
        let (created_attr, created_date) = if let Some(c) = st.metadata.created {
            tracing::debug!(target: "paperless.consumer", "Creation date from post_documents parameter: {c}");
            (CreatedAttr::Date(c), c)
        } else if let Some(d) = date {
            tracing::debug!(target: "paperless.consumer", "Creation date from parse_date: {d}");
            (datetime_attr(&d), d.with_timezone(&tz).date_naive())
        } else {
            let mtime = std::fs::metadata(&st.input.original_file).and_then(|m| m.modified()).map_err(|e| e.to_string())?;
            let dt = mtime_datetime(mtime, tz);
            tracing::debug!(target: "paperless.consumer", "Creation date from st_mtime: {dt}");
            (datetime_attr(&dt), dt.with_timezone(&tz).date_naive())
        };

        let mut title = match &st.metadata.filename {
            Some(f) => py_stem(f),
            None => st.input.original_file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        if let Some(t) = &st.metadata.title {
            match self.parse_title_placeholders(conn, st, t).await {
                Ok(Some(r)) => title = r,
                Ok(None) => {}
                Err(e) => tracing::error!(
                    target: "paperless.consumer",
                    "Error occurred parsing title override '{t}', falling back to original. Exception: {e}"
                ),
            }
        }
        let checksum = blocking_io(|| compute_checksum(working_copy)).map_err(|e| e.to_string())?;
        let now = now_micros();
        let mut row = DocRow {
            id: 0,
            title: title.chars().take(127).collect(),
            content: text.to_string(),
            mime_type: mime_type.to_string(),
            checksum,
            archive_checksum: None,
            page_count,
            created: created_date,
            modified: now,
            added: now,
            filename: None,
            archive_filename: None,
            original_filename: Some(st.filename()),
            archive_serial_number: None,
            correspondent_id: None,
            document_type_id: None,
            storage_path_id: None,
            owner_id: None,
            root_document_id: None,
            version_label: None,
            version_index: None,
            deleted_at: None,
            restored_at: None,
            transaction_id: None,
        };
        row.id = db::insert_document(conn, &row).await.map_err(|e| e.to_string())?;
        audit.document_created(conn, &row, &created_attr, None).await.map_err(|e| e.to_string())?;
        let mut doc = DocInstance { row, created_attr };
        self.apply_overrides(conn, st, audit, &mut doc).await?;
        docops::save(conn, audit, &mut doc, None).await.map_err(|e| e.to_string())?;
        Ok(doc)
    }

    /// `apply_overrides`.
    async fn apply_overrides(&self, conn: &mut PgConnection, st: &State, audit: &AuditLog, doc: &mut DocInstance) -> Result<(), String> {
        let m = &st.metadata;
        if let Some(c) = m.correspondent_id.filter(|c| *c != 0) {
            db::require_ids(conn, "documents_correspondent", "Correspondent", &[c]).await?;
            doc.row.correspondent_id = Some(c);
        }
        if let Some(t) = m.document_type_id.filter(|c| *c != 0) {
            db::require_ids(conn, "documents_documenttype", "DocumentType", &[t]).await?;
            doc.row.document_type_id = Some(t);
        }
        if let Some(tags) = m.tag_ids.as_ref().filter(|t| !t.is_empty()) {
            for t in tags {
                db::require_ids(conn, "documents_tag", "Tag", &[*t]).await?;
                docops::add_nested_tags(conn, audit, doc, &[*t]).await.map_err(|e| e.to_string())?;
            }
        }
        if let Some(s) = m.storage_path_id.filter(|c| *c != 0) {
            db::require_ids(conn, "documents_storagepath", "StoragePath", &[s]).await?;
            doc.row.storage_path_id = Some(s);
        }
        if let Some(asn) = m.asn {
            doc.row.archive_serial_number = Some(asn);
        }
        if let Some(l) = &m.version_label {
            doc.row.version_label = Some(l.clone());
        }
        if let Some(o) = m.owner_id.filter(|c| *c != 0) {
            db::require_ids(conn, "auth_user", "User", &[o]).await?;
            doc.row.owner_id = Some(o);
        }
        if m.view_users.is_some() || m.view_groups.is_some() || m.change_users.is_some() || m.change_groups.is_some() {
            let spec = pl_authz::owned::PermissionsSpec {
                view: Some(pl_authz::owned::GranteesUpdate {
                    users: Some(m.view_users.clone().unwrap_or_default()),
                    groups: Some(m.view_groups.clone().unwrap_or_default()),
                }),
                change: Some(pl_authz::owned::GranteesUpdate {
                    users: Some(m.change_users.clone().unwrap_or_default()),
                    groups: Some(m.change_groups.clone().unwrap_or_default()),
                }),
            };
            pl_authz::owned::set_permissions_for_objects(conn, &spec, &pl_authz::models::DOCUMENT, &[doc.row.id], false)
                .await
                .map_err(|e| format!("{e:?}"))?;
        }
        if let Some(cf) = m.custom_fields.as_ref().filter(|c| !c.is_empty()) {
            let ids: Vec<i64> = cf.keys().copied().collect();
            for f in db::custom_fields(conn, &ids).await.map_err(|e| e.to_string())? {
                let v = cf.get(&f.id).cloned().unwrap_or(Value::Null);
                create_custom_field_instance(conn, audit, doc.row.id, &f, &v).await.map_err(|e| e.to_string())?;
            }
        }
        if let Some(codes) = &m.barcodes {
            store_barcodes(conn, doc.row.id, codes).await.map_err(|e| e.to_string())?;
        }
        // PaperRust: storage location from the number range of the ASN (only with the "multiple number ranges" switch)
        if let Some(asn) = doc.row.archive_serial_number {
            crate::asn_location::apply_on_consume(conn, audit, doc.row.id, asn).await;
        }
        // PaperRust folder view (stage 2): upload into a folder (field `folder`, checked by the upload)
        if let Some(fid) = m.folder_id {
            assign_folder(conn, doc.row.id, fid).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// `_parse_title_placeholders`.
    async fn parse_title_placeholders(&self, conn: &mut PgConnection, st: &State, title: &str) -> Result<Option<String>, String> {
        let m = &st.metadata;
        let get = |o: Option<String>, model: &str| o.ok_or_else(|| format!("{model} matching query does not exist."));
        let corr = match m.correspondent_id {
            Some(id) => Some(get(db::object_name(conn, "documents_correspondent", id).await.map_err(|e| e.to_string())?, "Correspondent")?),
            None => None,
        };
        let dtype = match m.document_type_id {
            Some(id) => Some(get(db::object_name(conn, "documents_documenttype", id).await.map_err(|e| e.to_string())?, "DocumentType")?),
            None => None,
        };
        let owner = match m.owner_id {
            Some(id) => Some(get(db::username(conn, id).await.map_err(|e| e.to_string())?, "User")?),
            None => None,
        };
        let custom_fields = match &m.custom_fields {
            Some(cf) => crate::templating::override_custom_fields(conn, cf).await.map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        let filename = st.filename();
        let p = pl_templating::WorkflowPlaceholders {
            correspondent_name: corr,
            doc_type_name: dtype,
            owner_username: owner,
            local_added: Utc::now().with_timezone(&self.settings.time_zone).fixed_offset(),
            original_filename: filename.clone(),
            filename,
            created: None,
            doc_title: None,
            doc_url: None,
            doc_id: None,
            custom_fields,
        };
        pl_templating::parse_w_workflow_placeholders(title, &p).map_err(|e| e.to_string())
    }

    /// `_create_version_from_root` + saving + audit "Version Added".
    #[allow(clippy::too_many_arguments)]
    async fn create_version(
        &self,
        conn: &mut PgConnection,
        st: &State,
        root_id: i64,
        text: &str,
        page_count: Option<i64>,
        mime_type: &str,
        working_copy: &Path,
    ) -> Result<DocInstance, String> {
        tracing::debug!(target: "paperless.consumer", "Saving record for updated version to database");
        // select_for_update – on the table, not on the view: since data model stage 1
        // `documents_document` is a view with outer joins, `FOR UPDATE` fails there
        // (same rule as `pl_store::datamodel::document_table`).
        let lock_table = document_lock_table(conn).await.map_err(|e| e.to_string())?;
        let lock = Query::select()
            .column(a("id"))
            .from(a(lock_table))
            .and_where(Expr::col(a("id")).eq(root_id))
            .and_where(Expr::col(a("deleted_at")).is_null())
            .lock(sea_query::LockType::Update)
            .to_owned();
        if db::fetch_optional(conn, &lock).await.map_err(|e| e.to_string())?.is_none() {
            return Err("Document matching query does not exist.".into());
        }
        let root = db::load_document(conn, root_id).await.map_err(|e| e.to_string())?.ok_or("Document matching query does not exist.")?;
        let max_q = Query::select()
            .expr(Expr::col(a("version_index")).max())
            .from(a("documents_document"))
            .and_where(Expr::col(a("root_document_id")).eq(root_id))
            .to_owned();
        let max: Option<i32> = match db::fetch_optional(conn, &max_q).await.map_err(|e| e.to_string())? {
            Some(r) => sqlx::Row::try_get(&r, 0).map_err(|e| e.to_string())?,
            None => None,
        };
        let now = now_micros();
        let mut row = DocRow {
            id: 0,
            title: root.title.clone(),
            content: text.to_string(),
            mime_type: mime_type.to_string(),
            checksum: compute_checksum(working_copy).map_err(|e| e.to_string())?,
            archive_checksum: None,
            page_count,
            created: root.created,
            modified: now,
            added: now,
            filename: None,
            archive_filename: None,
            original_filename: Some(st.filename()),
            archive_serial_number: None,
            correspondent_id: None,
            document_type_id: None,
            storage_path_id: None,
            owner_id: root.owner_id,
            root_document_id: Some(root_id),
            version_label: st.metadata.version_label.clone(),
            version_index: Some(i64::from(max.unwrap_or(0)) + 1),
            deleted_at: None,
            restored_at: None,
            transaction_id: None,
        };
        let actor = if self.settings.audit_log_enabled { st.metadata.actor_id } else { None };
        let audit = AuditLog { enabled: self.settings.audit_log_enabled, actor_id: actor };
        row.id = db::insert_document(conn, &row).await.map_err(|e| e.to_string())?;
        audit.document_created(conn, &row, &CreatedAttr::Date(row.created), None).await.map_err(|e| e.to_string())?;
        if let Some(codes) = &st.metadata.barcodes {
            store_barcodes(conn, row.id, codes).await.map_err(|e| e.to_string())?;
        }
        let upd = Query::update()
            .table(a("documents_document"))
            .value(a("modified"), now_micros())
            .and_where(Expr::col(a("id")).eq(root_id))
            .to_owned();
        crate::lookup::forget_docs();
        let (sql, v) = upd.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, v).execute(&mut *conn).await.map_err(|e| e.to_string())?;
        let corr = match root.correspondent_id {
            Some(id) => db::object_name(conn, "documents_correspondent", id).await.map_err(|e| e.to_string())?,
            None => None,
        };
        audit::version_added(&audit, conn, &root, corr.as_deref(), row.id).await.map_err(|e| e.to_string())?;
        Ok(DocInstance::from_db(row))
    }

    /// Receivers of `document_consumption_finished` in connection order (documents/apps.py).
    async fn consumption_finished(
        &self,
        conn: &mut PgConnection,
        audit: &AuditLog,
        doc: &mut DocInstance,
        classifier: Option<&(dyn pl_matching::Classifier + Send + Sync)>,
        original_file: &Path,
    ) -> Result<workflows::RunOutcome, sqlx::Error> {
        let owner = match doc.row.owner_id {
            // FINDINGS B2: via the held transaction, no second pool connection
            Some(id) => pl_authz::user::load_user_by_id(&mut *conn, id).await?,
            None => None,
        };
        // add_inbox_tags
        let inbox = db::inbox_tags(conn, owner.as_ref()).await?;
        docops::add_nested_tags(conn, audit, doc, &inbox).await?;

        let content = workflows::effective_content(conn, &doc.row).await?;
        let md = pl_matching::MatchDocument { effective_content: Some(content), display: doc.repr(conn).await? };

        // set_correspondent
        if doc.row.correspondent_id.is_none() {
            let cands = db::matchables(conn, &pl_authz::models::CORRESPONDENT, "Correspondent", owner.as_ref()).await?;
            let found = pl_matching::matching::match_correspondents(&md, &cands, classifier.map(|c| c as &dyn pl_matching::Classifier))
                .unwrap_or_default();
            if let Some(sel) = found.first() {
                if found.len() > 1 {
                    tracing::debug!(target: "paperless.handlers", "Detected {} potential correspondents, so we've opted for {}", found.len(), sel.name);
                }
                tracing::info!(target: "paperless.handlers", "Assigning correspondent {} to {}", sel.name, md.display);
                doc.row.correspondent_id = Some(sel.id);
                docops::save(conn, audit, doc, Some(&["correspondent"])).await?;
            }
        }
        // set_document_type
        if doc.row.document_type_id.is_none() {
            let cands = db::matchables(conn, &pl_authz::models::DOCUMENT_TYPE, "DocumentType", owner.as_ref()).await?;
            let found = pl_matching::matching::match_document_types(&md, &cands, classifier.map(|c| c as &dyn pl_matching::Classifier))
                .unwrap_or_default();
            if let Some(sel) = found.first() {
                if found.len() > 1 {
                    tracing::info!(target: "paperless.handlers", "Detected {} potential document types, so we've opted for {}", found.len(), sel.name);
                }
                tracing::info!(target: "paperless.handlers", "Assigning document type {} to {}", sel.name, md.display);
                doc.row.document_type_id = Some(sel.id);
                docops::save(conn, audit, doc, Some(&["document_type"])).await?;
            }
        }
        // set_tags
        {
            let current: Vec<i64> = db::document_tags(conn, doc.row.id).await?.into_iter().map(|(i, _)| i).collect();
            let cands = db::matchables(conn, &pl_authz::models::TAG, "Tag", owner.as_ref()).await?;
            let found =
                pl_matching::matching::match_tags(&md, &cands, classifier.map(|c| c as &dyn pl_matching::Classifier)).unwrap_or_default();
            let add: Vec<i64> = found.iter().map(|m| m.id).filter(|i| !current.contains(i)).collect();
            if !add.is_empty() {
                let names: Vec<&str> = found.iter().filter(|m| add.contains(&m.id)).map(|m| m.name.as_str()).collect();
                tracing::info!(target: "paperless.handlers", "Tagging \"{}\" with \"{}\"", md.display, names.join(", "));
                docops::add_nested_tags(conn, audit, doc, &add).await?;
            }
        }
        // set_storage_path
        if doc.row.storage_path_id.is_none() {
            let cands = db::matchables(conn, &pl_authz::models::STORAGE_PATH, "StoragePath", owner.as_ref()).await?;
            let found = pl_matching::matching::match_storage_paths(&md, &cands, classifier.map(|c| c as &dyn pl_matching::Classifier))
                .unwrap_or_default();
            if let Some(sel) = found.first() {
                if found.len() > 1 {
                    tracing::info!(target: "paperless.handlers", "Detected {} potential storage paths, so we've opted for {}", found.len(), sel.name);
                }
                tracing::info!(target: "paperless.handlers", "Assigning storage path {} to {}", sel.name, md.display);
                doc.row.storage_path_id = Some(sel.id);
                docops::save(conn, audit, doc, Some(&["storage_path"])).await?;
            }
        }
        // run_workflows_added
        let outcome = workflows::run_added(conn, audit, doc, original_file, self.effects.as_ref(), self.settings.time_zone).await?;
        // add_to_index: after the commit (see store_all/run_consumer)
        Ok(outcome)
    }

    /// `update_filename_and_move_files` after the last `save()`: recompute the file name; if it is
    /// the same, only set `modified`, otherwise move files.
    pub async fn update_filename_and_move_files(
        &self,
        conn: &mut PgConnection,
        doc: &mut DocInstance,
        written: &mut Vec<PathBuf>,
    ) -> Result<(), sqlx::Error> {
        let Some(fresh) = db::load_document(conn, doc.row.id).await? else { return Ok(()) };
        doc.row = fresh;
        let Some(old_filename) = doc.row.filename.clone() else { return Ok(()) };
        let ctx = crate::templating::document_context(conn, &doc.row).await?;
        let opts = &self.settings.templating;
        let originals = &self.settings.originals_dir;
        let candidate = match pl_templating::generate_filename(&ctx, Default::default(), opts) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(target: "paperless.handlers", "Exception during file handling: {e}");
                return Ok(());
            }
        };
        let mut updates: Vec<&str> = vec!["modified"];
        doc.row.modified = now_micros();
        if candidate != old_filename {
            // Security review 2026-11-19 F9: join names with the root only via safe_join
            // (`files::join` returns a path that never exists for unsafe names).
            let cand_path = crate::files::join(originals, &candidate);
            let new_name = if cand_path.exists() {
                pl_templating::generate_unique_filename(&ctx, false, opts, |rel| crate::files::join(originals, rel).exists())
                    .unwrap_or(old_filename.clone())
            } else {
                candidate
            };
            if new_name != old_filename
                && let Some((from, to)) = rename_paths(originals, &old_filename, &new_name)
                && pl_templating::validate_path_in_root(&to, originals).is_ok()
                && !to.is_file()
                && from.is_file()
            {
                let _ = pl_templating::create_source_path_directory(&to);
                if move_file(&from, &to).is_ok() {
                    written.retain(|p| p != &from);
                    written.push(to);
                    doc.row.filename = Some(new_name);
                    updates.push("filename");
                    pl_templating::delete_empty_directories(from.parent().unwrap_or(originals), originals);
                }
            }
        }
        db::update_document_fields(conn, &doc.row, &updates).await
    }

    fn run_pre_consume_script(&self, st: &State, working_copy: &Path) -> Result<(), ConsumeError> {
        let Some(script) = &self.settings.pre_consume_script else { return Ok(()) };
        if !script.is_file() {
            return Err(self.fail(
                st,
                msg::PRE_CONSUME_SCRIPT_NOT_FOUND,
                Some(&format!("Configured pre-consume script {} does not exist.", script.display())),
            ));
        }
        tracing::info!(target: "paperless.consumer", "Executing pre-consume script {}", script.display());
        let env = vec![
            ("DOCUMENT_SOURCE_PATH".to_string(), st.input.original_file.to_string_lossy().into_owned()),
            ("DOCUMENT_WORKING_PATH".to_string(), working_copy.to_string_lossy().into_owned()),
            ("TASK_ID".to_string(), st.task_id.clone().unwrap_or_default()),
        ];
        self.scripts
            .run(script, &env)
            .map_err(|e| self.fail(st, msg::PRE_CONSUME_SCRIPT_ERROR, Some(&format!("Error while executing pre-consume script: {e}"))))
    }

    async fn run_post_consume_script(&self, st: &State, doc_id: i64) -> Result<(), ConsumeError> {
        let Some(script) = self.settings.post_consume_script.clone() else { return Ok(()) };
        if !script.is_file() {
            return Err(self.fail(
                st,
                msg::POST_CONSUME_SCRIPT_NOT_FOUND,
                Some(&format!("Configured post-consume script {} does not exist.", script.display())),
            ));
        }
        tracing::info!(target: "paperless.consumer", "Executing post-consume script {}", script.display());
        let mut conn = self.pool.acquire().await?;
        let d = db::load_document(&mut conn, doc_id).await?.ok_or_else(|| ConsumeError::Other("document vanished".into()))?;
        let name = |t: &'static str, id: Option<i64>| (t, id);
        let mut names = Vec::new();
        for (t, id) in [name("documents_documenttype", d.document_type_id), name("documents_correspondent", d.correspondent_id)] {
            names.push(match id {
                Some(id) => db::object_name(&mut conn, t, id).await?.unwrap_or_else(|| "None".into()),
                None => "None".into(),
            });
        }
        let owner = match d.owner_id {
            Some(o) => db::username(&mut conn, o).await?.unwrap_or_default(),
            None => String::new(),
        };
        let tags: Vec<String> = db::document_tags(&mut conn, doc_id).await?.into_iter().map(|(_, n)| n).collect();
        let s = &self.settings;
        let src = s.originals_dir.join(d.filename.clone().unwrap_or_default());
        let arch = d.archive_filename.as_ref().map_or_else(|| "None".to_string(), |f| s.archive_dir.join(f).to_string_lossy().into_owned());
        let ext = pl_templating::default_file_extension(&d.mime_type);
        let env = vec![
            ("DOCUMENT_ID".to_string(), doc_id.to_string()),
            ("DOCUMENT_TYPE".to_string(), names[0].clone()),
            ("DOCUMENT_CREATED".to_string(), d.created.format("%Y-%m-%d").to_string()),
            ("DOCUMENT_MODIFIED".to_string(), py_aware_utc(&d.modified)),
            ("DOCUMENT_ADDED".to_string(), py_aware_utc(&d.added)),
            (
                "DOCUMENT_FILE_NAME".to_string(),
                pl_templating::sanitize_filename(
                    &format!(
                        "{}{ext}",
                        audit::document_repr(&CreatedAttr::Date(d.created), d.correspondent_id.map(|_| names[1].as_str()), &d.title)
                    ),
                    "-",
                ),
            ),
            ("DOCUMENT_SOURCE_PATH".to_string(), src.to_string_lossy().into_owned()),
            ("DOCUMENT_ARCHIVE_PATH".to_string(), arch),
            ("DOCUMENT_THUMBNAIL_PATH".to_string(), s.thumbnail_path(doc_id).to_string_lossy().into_owned()),
            ("DOCUMENT_DOWNLOAD_URL".to_string(), format!("/api/documents/{doc_id}/download/")),
            ("DOCUMENT_THUMBNAIL_URL".to_string(), format!("/api/documents/{doc_id}/thumb/")),
            ("DOCUMENT_OWNER".to_string(), owner),
            ("DOCUMENT_CORRESPONDENT".to_string(), names[1].clone()),
            ("DOCUMENT_TAGS".to_string(), tags.join(",")),
            ("DOCUMENT_ORIGINAL_FILENAME".to_string(), d.original_filename.clone().unwrap_or_else(|| "None".into())),
            ("TASK_ID".to_string(), st.task_id.clone().unwrap_or_default()),
        ];
        drop(conn);
        self.scripts
            .run(&script, &env)
            .map_err(|e| self.fail(st, msg::POST_CONSUME_SCRIPT_ERROR, Some(&format!("Error while executing post-consume script: {e}"))))
    }
}

/// `ConsumerPlugin` order (tasks.consume_file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plugin {
    Preflight,
    AsnCheck,
    Collate,
    Barcode,
    WorkflowTrigger,
    Consumer,
}

impl Plugin {
    pub fn name(self) -> &'static str {
        match self {
            Plugin::Preflight => "ConsumerPreflightPlugin",
            Plugin::AsnCheck => "AsnCheckPlugin",
            Plugin::Collate => "CollatePlugin",
            Plugin::Barcode => "BarcodePlugin",
            Plugin::WorkflowTrigger => "WorkflowTriggerPlugin",
            Plugin::Consumer => "ConsumerPlugin",
        }
    }
}

struct State {
    input: ConsumableDocument,
    metadata: DocumentMetadataOverrides,
    #[allow(dead_code)]
    tmp_dir: PathBuf,
    task_id: Option<String>,
    status_filename: String,
    /// `BarcodeConfig()` of this run (environment + ApplicationConfiguration).
    barcodes: BarcodeSettings,
    /// Checksum of the original from the preflight (duplicate re-check when storing).
    checksum: Option<String>,
    /// PaperRust: the stored file is a shrunk copy of the original (pdftools.rs); its checksum then
    /// differs from `checksum` and the original's checksum goes to `paperrust_source_checksum`.
    shrunk: bool,
}

impl State {
    /// `self.filename = self.metadata.filename or self.input_doc.original_file.name`.
    fn filename(&self) -> String {
        self.metadata.filename.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| self.input.file_name())
    }
}

/// `CustomFieldInstance.objects.create(field=, document=, value_<typ>=)` + Audit.
pub async fn create_custom_field_instance(
    conn: &mut PgConnection,
    audit: &AuditLog,
    doc_id: i64,
    f: &db::CustomFieldDef,
    value: &Value,
) -> Result<(), sqlx::Error> {
    let Some(col_name) = db::value_field_name(&f.data_type) else {
        return Err(sqlx::Error::Protocol(format!("unknown custom field type {}", f.data_type)));
    };
    let expr = db::custom_value_expr(&f.data_type, value).map_err(sqlx::Error::Protocol)?;
    let created = now_micros();
    let q = Query::insert()
        .into_table(a("documents_customfieldinstance"))
        .columns([a("created"), a("field_id"), a("document_id"), a(col_name)])
        .values_panic([created.into(), (f.id as i32).into(), (doc_id as i32).into(), expr])
        .returning(Query::returning().exprs([Expr::col(a("id")).into(), Expr::col(a("value_monetary_amount")).cast_as(a("text"))]))
        .to_owned();
    let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, v).fetch_one(&mut *conn).await?;
    let id = i64::from(sqlx::Row::try_get::<i32, _>(&row, 0)?);
    // GeneratedField: Django reads the value back after the INSERT (auditlog sees it)
    let amount: Option<String> = sqlx::Row::try_get(&row, 1)?;
    let repr = format!("{} : {}", f.name, audit::custom_value_for_search(&f.data_type, value, f.extra_data.as_ref()));
    audit
        .custom_field_instance_created(
            conn,
            id,
            f.id,
            doc_id,
            &created,
            col_name,
            audit::custom_value_smart_str(&f.data_type, value),
            amount,
            &repr,
        )
        .await
}

/// `DocumentBarcode.objects.bulk_create(...)`.
/// Folder view (stage 2, pl-api-v2 `folders.rs`): put the new document into folder `folder_id`
/// (`paperrust_document_folder`, at most one folder per document). A folder deleted since the upload (or
/// missing folder tables) leaves the document in "Unsorted" with a warning instead of failing the ingestion.
async fn assign_folder(conn: &mut PgConnection, doc_id: i64, folder_id: i64) -> Result<(), sqlx::Error> {
    let tables: Option<String> = sqlx::query_scalar("SELECT to_regclass('paperrust_document_folder')::text").fetch_one(&mut *conn).await?;
    let done = if tables.is_some() {
        sqlx::query(
            "INSERT INTO paperrust_document_folder (document_id, folder_id) SELECT $1::int4, f.id FROM paperrust_folder f WHERE f.id = $2 FOR KEY SHARE ON CONFLICT (document_id) DO UPDATE SET folder_id = EXCLUDED.folder_id, moved = now()",
        )
        .bind(doc_id)
        .bind(folder_id)
        .execute(&mut *conn)
        .await?
        .rows_affected()
            > 0
    } else {
        false
    };
    if !done {
        tracing::warn!(target: "paperless.consumer", "Folder {folder_id} no longer exists, document {doc_id} stays unsorted");
    }
    Ok(())
}

async fn store_barcodes(conn: &mut PgConnection, doc_id: i64, codes: &[StoredBarcode]) -> Result<(), sqlx::Error> {
    for c in codes {
        let q = Query::insert()
            .into_table(a("documents_documentbarcode"))
            .columns([a("document_id"), a("page"), a("value"), a("format")])
            .values_panic([(doc_id as i32).into(), (c.page as i32).into(), c.value.clone().into(), c.format.clone().into()])
            .to_owned();
        let (sql, v) = q.build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, v).execute(&mut *conn).await?;
    }
    Ok(())
}

pub(crate) fn cf_template_value(f: &db::CustomFieldDef, v: &Value) -> pl_templating::CustomFieldInstance {
    use pl_templating::CustomFieldValue as V;
    let value = match (f.data_type.as_str(), v) {
        (_, Value::Null) => V::Null,
        ("boolean", Value::Bool(b)) => V::Bool(*b),
        ("integer", Value::Number(n)) => n.as_i64().map_or(V::Null, V::Int),
        ("float", Value::Number(n)) => n.as_f64().map_or(V::Null, V::Float),
        ("date", Value::String(s)) => NaiveDate::parse_from_str(s, "%Y-%m-%d").map_or(V::Str(s.clone()), V::Date),
        ("documentlink", Value::Array(a)) => V::IntList(a.iter().filter_map(Value::as_i64).collect()),
        (_, Value::String(s)) => V::Str(s.clone()),
        (_, other) => V::Str(other.to_string()),
    };
    let select_options = f.extra_data.as_ref().and_then(|e| e.get("select_options")).and_then(Value::as_array).map(|o| {
        o.iter()
            .map(|x| pl_templating::SelectOption {
                id: x.get("id").and_then(Value::as_str).unwrap_or_default().into(),
                label: x.get("label").and_then(Value::as_str).unwrap_or_default().into(),
            })
            .collect()
    });
    pl_templating::CustomFieldInstance { field_name: f.name.clone(), data_type: f.data_type.clone(), select_options, value }
}

/// `str(datetime)` / `isoformat()` of an aware datetime.
fn datetime_attr(d: &DateTime<FixedOffset>) -> CreatedAttr {
    let off = d.offset().local_minus_utc();
    let sign = if off < 0 { '-' } else { '+' };
    let o = off.abs();
    let mut tz = format!("{sign}{:02}:{:02}", o / 3600, (o % 3600) / 60);
    if o % 60 != 0 {
        tz.push_str(&format!(":{:02}", o % 60));
    }
    let micros = d.timestamp_subsec_micros();
    let base = d.naive_local();
    let (s, i) = if micros == 0 {
        (base.format("%Y-%m-%d %H:%M:%S").to_string(), base.format("%Y-%m-%dT%H:%M:%S").to_string())
    } else {
        (base.format("%Y-%m-%d %H:%M:%S%.6f").to_string(), base.format("%Y-%m-%dT%H:%M:%S%.6f").to_string())
    };
    CreatedAttr::DateTime { str_: format!("{s}{tz}"), iso: format!("{i}{tz}") }
}

/// `str(aware datetime)` in UTC as Django returns them from the DB (`+00:00`).
fn py_aware_utc(t: &DateTime<Utc>) -> String {
    format!("{}+00:00", audit::py_naive_utc(t))
}

/// `datetime.fromtimestamp(st_mtime, tz=current_timezone)` (microseconds rounded like Python).
fn mtime_datetime(t: SystemTime, tz: chrono_tz::Tz) -> DateTime<FixedOffset> {
    let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    };
    let whole = secs.floor();
    let mut micros = ((secs - whole) * 1e6).round_ties_even() as i64;
    let mut s = whole as i64;
    if micros >= 1_000_000 {
        s += 1;
        micros -= 1_000_000;
    }
    let utc = Utc.timestamp_opt(s, (micros * 1000) as u32).single().unwrap_or_else(Utc::now);
    utc.with_timezone(&tz).fixed_offset()
}

/// `Path(name).stem`.
fn py_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base.to_string(),
    }
}

/// `f"{n:,}"`.
fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Source/target path for `update_filename_and_move_files`: both names only via `safe_join`
/// (security review 2026-11-19 F9). Unsafe name (from the DB or the template) → `None`, the
/// file is then treated like a missing one and not moved (warning without path to clients).
fn rename_paths(originals: &Path, old: &str, new: &str) -> Option<(PathBuf, PathBuf)> {
    let from = crate::files::safe_join(originals, old);
    let to = crate::files::safe_join(originals, new);
    if from.is_none() || to.is_none() {
        tracing::warn!(target: "paperless.handlers", "Refusing to move document file: stored or generated filename is unsafe");
    }
    Some((from?, to?))
}

/// File name of the working copy in the working folder (F4): only the last part, without drive/stream separator
/// `:` and characters forbidden on Windows, without trailing dots/spaces, no device names
/// (`CON`, `NUL`, `COM1` …). The extension stays (type detection, qpdf/pdftools depend on it).
pub fn working_file_name(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or("");
    // shared sanitizing in pl-core (security review 2026-11-19 F12)
    pl_core::safepath::sanitize_component(base, "document")
}

/// `compute_checksum` (SHA-256 hex).
pub fn compute_checksum(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `copy_file_with_basic_stats` (content + mtime/atime).
fn copy_with_stats(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::copy(src, dst)?;
    copy_times(src, dst);
    Ok(())
}

fn copy_times(src: &Path, dst: &Path) {
    if let Ok(m) = std::fs::metadata(src)
        && let Ok(mt) = m.modified()
        && let Ok(f) = std::fs::File::options().write(true).open(dst)
    {
        let times = std::fs::FileTimes::new().set_modified(mt).set_accessed(m.accessed().unwrap_or(mt));
        let _ = f.set_times(times);
    }
}

/// `_write(source, target)` atomic: copy to a temp file next to the target, then rename.
/// The target is noted in `written` (cleanup on error).
fn write_atomic(src: &Path, target: &Path, written: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let parent = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = parent.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().simple()));
    std::fs::copy(src, &tmp)?;
    copy_times(src, &tmp);
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    written.push(target.to_path_buf());
    Ok(())
}

/// `shutil.move`.
pub(crate) fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            copy_times(from, to);
            std::fs::remove_file(from)
        }
    }
}

/// New directory for the own user only (final security review W-N2, like
/// `tempfile.mkdtemp`: Unix `0700`). Windows: no own ACL – the directory inherits the
/// permissions of `SCRATCH_DIR` (default `%TEMP%` of the service account or the desktop data directory
/// restricted to the user via ACL); `mkdtemp` sets no ACL on Windows either.
fn create_private_dir(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(p)
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(p)
    }
}

/// `tempfile.TemporaryDirectory(prefix=..., dir=...)`.
pub(crate) struct TempDir {
    pub path: PathBuf,
    keep: bool,
}

/// Cleanup after processing (D-102): if the input is in a spool directory
/// `SCRATCH_DIR/tmp…/` (upload via API/UI) or `SCRATCH_DIR/paperless-mail-…/`
/// (mail attachment), the file – even after
/// failure or duplicate – and the then empty directory are deleted. upstream deletes only the
/// original after success and leaves the `mkdtemp` directory (and after failure the file)
/// in place. Files outside (input folder, versions from the media folder) stay untouched.
/// tests/chaos C5: remove working folders that an aborted process (hard abort, power failure) left in
/// `SCRATCH_DIR`. Only folders that the consumer creates itself
/// ([`TempDir`]: `paperless-ngx<32 hex>` for the working copy, `tmp<32 hex>` for parser intermediate files)
/// and that are older than `older_than` (no running ingestion can still use them: the
/// task has a hard time limit). Upload spool (`tmp` + 8 characters) and mail spool
/// (`paperless-mail-…`) belong to enqueued tasks and stay. Returns the number of deleted folders.
pub fn sweep_stale_scratch(scratch_dir: &Path, older_than: Duration) -> usize {
    let Ok(rd) = std::fs::read_dir(scratch_dir) else { return 0 };
    let now = SystemTime::now();
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_consumer_workdir(&name) || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let old =
            e.metadata().and_then(|m| m.modified()).ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age >= older_than);
        if old && std::fs::remove_dir_all(e.path()).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        tracing::info!(target: "paperless.consumer", "Removed {n} stale working folder(s) from {}", scratch_dir.display());
    }
    n
}

/// Name of a working folder created by the consumer (`paperless-ngx`/`tmp` + 32 hex characters).
fn is_consumer_workdir(name: &str) -> bool {
    let rest = name.strip_prefix("paperless-ngx").or_else(|| name.strip_prefix("tmp"));
    rest.is_some_and(|r| r.len() == 32 && r.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Background loop for [`sweep_stale_scratch`]: at startup and then hourly.
pub async fn scratch_sweeper(scratch_dir: PathBuf, older_than: Duration) {
    loop {
        let dir = scratch_dir.clone();
        let _ = tokio::task::spawn_blocking(move || sweep_stale_scratch(&dir, older_than)).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

pub fn cleanup_upload_spool(scratch_dir: &Path, original: &Path) {
    let Some(dir) = original.parent() else { return };
    let is_spool = dir.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.starts_with("tmp") || n.starts_with("paperless-mail-")
    }) && dir.parent().is_some_and(|p| same_dir(p, scratch_dir));
    if !is_spool {
        return;
    }
    if original.is_file() {
        tracing::debug!(target: "paperless.consumer", "Deleting leftover upload {}", original.display());
        let _ = std::fs::remove_file(original);
    }
    if let Some(name) = original.file_name() {
        let _ = std::fs::remove_file(dir.join(format!("._{}", name.to_string_lossy())));
    }
    // only if empty (remove_dir fails otherwise) – foreign files stay
    let _ = std::fs::remove_dir(dir);
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => absolute(a) == absolute(b),
    }
}

impl TempDir {
    pub fn new(base: &Path, prefix: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(base)?;
        let path = absolute(base).join(format!("{prefix}{}", uuid::Uuid::new_v4().simple()));
        create_private_dir(&path)?;
        Ok(Self { path, keep: false })
    }
    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// `FileLock(settings.MEDIA_LOCK)`: see [`crate::medialock`] (lock order, time limit).
pub(crate) use crate::medialock::MediaLock;

/// `CONSUMER_PDF_RECOVERABLE_MIME_TYPES`.
pub const PDF_RECOVERABLE_MIME_TYPES: &[&str] = &["application/octet-stream"];

/// `run_subprocess(["qpdf", "--replace-input", working_copy])` with pl-pdfops instead of qpdf.
/// Exit 0 → `Ok`; exit 3 (warnings, file replaced anyway) or 2 (not readable, file
/// unchanged) → `check_returncode()` → `CalledProcessError` as upstream (MIME is then not
/// determined again).
fn clean_with_qpdf(working_copy: &Path) -> Result<(), String> {
    let code = match pl_pdfops::repair::replace_input(working_copy) {
        Ok(outcome) => outcome.exit_code(),
        Err(e) => {
            tracing::warn!(target: "paperless.consumer", "{e}");
            2
        }
    };
    tracing::info!(target: "paperless.consumer", "qpdf exited {code}");
    if code == 0 {
        return Ok(());
    }
    Err(format!("Command '['qpdf', '--replace-input', {}]' returned non-zero exit status {code}.", py_path_repr(working_copy)))
}

/// `repr(Path)`: `PosixPath('…')` or `WindowsPath('…')` with `as_posix()` and Python `repr` of the text.
fn py_path_repr(p: &Path) -> String {
    let posix = p.to_string_lossy().replace('\\', "/");
    let quoted =
        if posix.contains('\'') && !posix.contains('"') { format!("\"{posix}\"") } else { format!("'{}'", posix.replace('\'', "\\'")) };
    if cfg!(windows) { format!("WindowsPath({quoted})") } else { format!("PosixPath({quoted})") }
}

/// Barcode columns of `paperless_applicationconfiguration` (first row, `BarcodeConfig`).
pub async fn load_barcode_app_config(pool: &PgPool) -> Result<BarcodeAppConfig, sqlx::Error> {
    use sqlx::Row as _;
    let q = Query::select()
        .columns([
            a("barcodes_enabled"),
            a("barcode_enable_tiff_support"),
            a("barcode_string"),
            a("barcode_retain_split_pages"),
            a("barcode_enable_asn"),
            a("barcode_asn_prefix"),
            a("barcode_upscale"),
            a("barcode_dpi"),
            a("barcode_max_pages"),
            a("barcode_enable_tag"),
            a("barcode_tag_mapping"),
            a("barcode_tag_split"),
            a("barcode_store_values"),
        ])
        .from(a("paperless_applicationconfiguration"))
        .order_by(a("id"), Order::Asc)
        .limit(1)
        .to_owned();
    let mut conn = pool.acquire().await?;
    let Some(r) = db::fetch_optional(&mut conn, &q).await? else { return Ok(Default::default()) };
    Ok(BarcodeAppConfig {
        barcodes_enabled: r.try_get("barcodes_enabled")?,
        barcode_enable_tiff_support: r.try_get("barcode_enable_tiff_support")?,
        barcode_string: r.try_get("barcode_string")?,
        barcode_retain_split_pages: r.try_get("barcode_retain_split_pages")?,
        barcode_enable_asn: r.try_get("barcode_enable_asn")?,
        barcode_asn_prefix: r.try_get("barcode_asn_prefix")?,
        barcode_upscale: r.try_get("barcode_upscale")?,
        barcode_dpi: r.try_get::<Option<i16>, _>("barcode_dpi")?.map(i64::from),
        barcode_max_pages: r.try_get::<Option<i16>, _>("barcode_max_pages")?.map(i64::from),
        barcode_enable_tag: r.try_get("barcode_enable_tag")?,
        barcode_tag_mapping: r.try_get("barcode_tag_mapping")?,
        barcode_tag_split: r.try_get("barcode_tag_split")?,
        barcode_store_values: r.try_get("barcode_store_values")?,
    })
}

/// `paperless_applicationconfiguration` (first row) as OCR overrides.
pub async fn load_app_config(pool: &PgPool) -> Result<pl_ocr::config::AppConfigOverrides, sqlx::Error> {
    use sqlx::Row as _;
    let q = Query::select()
        .columns([
            a("output_type"),
            a("pages"),
            a("language"),
            a("mode"),
            a("archive_file_generation"),
            a("image_dpi"),
            a("unpaper_clean"),
            a("deskew"),
            a("rotate_pages"),
            a("rotate_pages_threshold"),
            a("max_image_pixels"),
            a("color_conversion_strategy"),
            a("user_args"),
            a("remote_ocr_engine"),
            a("remote_ocr_api_key"),
            a("remote_ocr_endpoint"),
            a("remote_ocr_mode"),
        ])
        .from(a("paperless_applicationconfiguration"))
        .order_by(a("id"), Order::Asc)
        .limit(1)
        .to_owned();
    let mut conn = pool.acquire().await?;
    let Some(r) = db::fetch_optional(&mut conn, &q).await? else { return Ok(Default::default()) };
    Ok(pl_ocr::config::AppConfigOverrides {
        output_type: r.try_get("output_type")?,
        pages: r.try_get::<Option<i16>, _>("pages")?.map(i64::from),
        language: r.try_get("language")?,
        mode: r.try_get("mode")?,
        archive_file_generation: r.try_get("archive_file_generation")?,
        image_dpi: r.try_get::<Option<i16>, _>("image_dpi")?.map(i64::from),
        unpaper_clean: r.try_get("unpaper_clean")?,
        deskew: r.try_get("deskew")?,
        rotate_pages: r.try_get("rotate_pages")?,
        rotate_pages_threshold: r.try_get("rotate_pages_threshold")?,
        max_image_pixels: r.try_get("max_image_pixels")?,
        color_conversion_strategy: r.try_get("color_conversion_strategy")?,
        user_args: r.try_get("user_args")?,
        // RemoteOCRConfig: UI values before the environment (`app_config.x or settings.X`)
        remote_ocr_engine: r.try_get("remote_ocr_engine")?,
        remote_ocr_api_key: r.try_get("remote_ocr_api_key")?,
        remote_ocr_endpoint: r.try_get("remote_ocr_endpoint")?,
        remote_ocr_mode: r.try_get("remote_ocr_mode")?,
    })
}

use crate::medialock::document_lock_table;

/// Prefix on error texts of `store_all` when the database aborted the transaction because of a deadlock
/// or serialization failure (retry permitted). Removed before the message.
const RETRYABLE_MARK: &str = "[retryable] ";

/// Failure of `store_all`: an error text (40P01/40001 marked with [`RETRYABLE_MARK`]) or a
/// uniqueness conflict found by `claim_unique` (reported like the preflight/ASN plugins).
#[derive(Debug)]
enum StoreFail {
    Error(String),
    Duplicate(Vec<db::DuplicateRow>),
    AsnExists { asn: i64, in_trash: bool },
}

impl From<String> for StoreFail {
    fn from(e: String) -> Self {
        Self::Error(e)
    }
}

impl From<&str> for StoreFail {
    fn from(e: &str) -> Self {
        Self::Error(e.to_string())
    }
}

/// Synchronous file work (copying, checksums of whole files) inside the async consume path.
/// On the multi-threaded runtime it runs via `block_in_place`, so the runtime moves its other
/// tasks (web requests when the worker runs in the server process) to another thread instead of
/// letting them wait behind a slow disk; elsewhere (current-thread runtime in tests) directly.
pub(crate) fn blocking_io<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// Error in `store_all` as text; 40P01/40001 with [`RETRYABLE_MARK`] (see `run_consumer`),
/// write errors in the media folder (disk full, no write permission) with [`STORAGE_MARK`].
fn store_err<E: std::fmt::Display + 'static>(e: E) -> String {
    let any: &dyn std::any::Any = &e;
    if let Some(se) = any.downcast_ref::<sqlx::Error>()
        && db::is_retryable(se)
    {
        return format!("{RETRYABLE_MARK}{e}");
    }
    if let Some(io) = any.downcast_ref::<std::io::Error>()
        && is_storage_error(io)
    {
        return format!("{STORAGE_MARK}{e}");
    }
    e.to_string()
}

/// Prefix on error texts of `store_all` for write errors in the media folder (tests/chaos C3).
const STORAGE_MARK: &str = "[storage] ";

/// Message for write errors in the media folder (part of the error message; recognized by
/// [`Consumer::is_transient_failure`]).
pub const MSG_STORAGE_FAILED: &str = "Could not write the document files (disk full or no write permission on the media folder)";

/// Disk full / quota / read-only / no write permission (tests/chaos C3). Windows:
/// ERROR_DISK_FULL 112, ERROR_HANDLE_DISK_FULL 39; Unix: ENOSPC 28, EDQUOT 122 (Linux) or 69 (BSD/macOS).
pub fn is_storage_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    if matches!(e.kind(), K::StorageFull | K::PermissionDenied | K::ReadOnlyFilesystem | K::QuotaExceeded) {
        return true;
    }
    let code = e.raw_os_error().unwrap_or(0);
    if cfg!(windows) { matches!(code, 112 | 39) } else { matches!(code, 28 | 122 | 69) }
}

/// Key in `paperrust_task_queue.headers`: id of the document that the ingestion task created
/// (FINDINGS B7). Set in the ingestion transaction.
pub const TASK_DOCUMENT_HEADER: &str = "paperrust_document_id";

async fn task_queue_exists(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT to_regclass('paperrust_task_queue') IS NOT NULL").fetch_one(&mut *conn).await
}

/// Notes `doc_id` on the running task `task_id` (without a queue table: nothing).
pub async fn record_task_document(conn: &mut PgConnection, task_id: &str, doc_id: i64) -> Result<(), sqlx::Error> {
    if !task_queue_exists(conn).await? {
        return Ok(());
    }
    sqlx::query("UPDATE paperrust_task_queue SET headers = headers || jsonb_build_object($2::text, $3::bigint) WHERE task_id = $1")
        .bind(task_id)
        .bind(TASK_DOCUMENT_HEADER)
        .bind(doc_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Document that an earlier delivery of the task `task_id` has already saved (and that still
/// exists, also in the trash).
pub async fn task_document(pool: &PgPool, task_id: &str) -> Result<Option<i64>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    if !task_queue_exists(&mut conn).await? {
        return Ok(None);
    }
    let id: Option<i32> = sqlx::query_scalar(
        "SELECT d.id FROM paperrust_task_queue q          JOIN documents_document d ON d.id = (q.headers ->> $2)::int4          WHERE q.task_id = $1 AND (q.headers ->> $2) ~ '^[0-9]{1,9}$'",
    )
    .bind(task_id)
    .bind(TASK_DOCUMENT_HEADER)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(id.map(i64::from))
}

/// Checksums under which document `doc_id` counts as "this file": the stored `checksum` and, for a
/// shrunk original, the delivered file's checksum (`paperrust_source_checksum`).
async fn stored_checksums(pool: &PgPool, doc_id: i64) -> Result<Vec<String>, sqlx::Error> {
    crate::source_checksum::ensure_pool(pool).await?;
    let id = i32::try_from(doc_id).map_err(|e| sqlx::Error::Protocol(format!("document id {doc_id}: {e}")))?;
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT d.checksum, s.checksum FROM documents_document d \
         LEFT JOIN paperrust_source_checksum s ON s.document_id = d.id WHERE d.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(a, b)| a.into_iter().chain(b).filter(|c| !c.is_empty()).collect()).unwrap_or_default())
}

/// What to do with the input path of a redelivered task whose document is already stored.
#[derive(Debug, PartialEq, Eq)]
enum RedeliveredOriginal {
    /// Same bytes as the stored document: the leftover original, delete it.
    Delete,
    /// No file at the path (any more).
    Missing,
    /// Different or unknown content: keep it (reason for the log).
    Keep(String),
}

/// Compares the file at `path` (SHA-256, as [`compute_checksum`] / `documents_document.checksum`)
/// with the stored checksums. Without a known checksum nothing is deleted.
fn redelivered_original_action(path: &Path, stored: &[String]) -> RedeliveredOriginal {
    if !path.is_file() {
        return RedeliveredOriginal::Missing;
    }
    if stored.is_empty() {
        return RedeliveredOriginal::Keep("checksum of the stored document unknown".into());
    }
    match compute_checksum(path) {
        Ok(c) if stored.iter().any(|s| s.eq_ignore_ascii_case(&c)) => RedeliveredOriginal::Delete,
        Ok(_) => RedeliveredOriginal::Keep("content differs from the stored document".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RedeliveredOriginal::Missing,
        Err(e) => RedeliveredOriginal::Keep(format!("could not read it ({e})")),
    }
}

/// Outcome of one noted file of an unsaved earlier attempt (see [`Consumer::resolve_pending_files`]).
#[derive(Debug)]
enum PendingCheck {
    /// A stored document refers to the file: keep the file, drop it from the note.
    Referenced,
    /// No document refers to it: delete it.
    Unreferenced,
    /// The reference check failed: keep file and note entry for a later delivery.
    Unknown,
}

/// Deletes the unreferenced noted files. Returns (number really deleted, entries that must stay in
/// the note for a later retry: failed reference checks and failed deletions).
fn remove_unreferenced(entries: &[(String, PendingCheck)]) -> (usize, Vec<String>) {
    let mut removed = 0;
    let mut remaining = Vec::new();
    for (f, check) in entries {
        match check {
            PendingCheck::Referenced => {}
            PendingCheck::Unknown => remaining.push(f.clone()),
            PendingCheck::Unreferenced => match std::fs::remove_file(f) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(target: "paperless.consumer", "Could not remove noted file {f}: {e}");
                    remaining.push(f.clone());
                }
            },
        }
    }
    (removed, remaining)
}

/// Options of the deferred index task (`index_document` with countdown).
fn index_retry_opts(target: i64, countdown: f64) -> pl_queue::EnqueueOptions {
    pl_queue::EnqueueOptions { args: vec![Value::from(target)], countdown: Some(countdown), ..Default::default() }
}

/// Wait times between the attempts to enqueue the index task afterwards (together a good 30 min).
const INDEX_SCHEDULE_RETRY: &[std::time::Duration] = &[
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(10),
    std::time::Duration::from_secs(20),
    std::time::Duration::from_secs(40),
    std::time::Duration::from_secs(60),
    std::time::Duration::from_secs(120),
    std::time::Duration::from_secs(300),
    std::time::Duration::from_secs(600),
    std::time::Duration::from_secs(900),
];

/// Enqueues `index_document` for `target` as soon as the queue (DB) is reachable again.
/// `true` = enqueued.
pub(crate) async fn retry_schedule_index(q: Arc<dyn pl_queue::TaskQueue>, target: i64, delays: &[std::time::Duration]) -> bool {
    for d in delays {
        tokio::time::sleep(*d).await;
        // DB is back: run immediately (the countdown 60 only applies to a taken index lock)
        let opts = pl_queue::EnqueueOptions { args: vec![Value::from(target)], ..Default::default() };
        match q.enqueue("documents.tasks.index_document", Default::default(), opts).await {
            Ok(_) => {
                tracing::info!(target: "paperless.consumer", "Index update for {target} scheduled after retry");
                return true;
            }
            Err(e) => tracing::debug!(target: "paperless.consumer", "Index update for {target} still not schedulable: {e}"),
        }
    }
    tracing::error!(target: "paperless.consumer", "Document {target} could not be added to the search index; run `document_index reindex`");
    false
}

/// PaperRust: merges checked AI results into the content (see [`Consumer::apply_vision`]). Returns the
/// transcript entries (statuses adjusted: line texts not found in the page text and whole-page texts
/// that could not be placed become `Skipped`) and the new content if any AI text went into it.
fn merge_vision_result(
    lines: Option<&pl_ocr::uncertain::LinesFile>,
    items: &[pl_core::vision::ItemResult],
    pages: usize,
) -> (Vec<pl_core::vision::TranscriptEntry>, Option<String>) {
    use pl_core::vision::{ItemStatus, TranscriptEntry};
    use std::collections::BTreeMap;
    let mut new_text: Option<String> = None;
    let page_lines = |p: usize| lines.and_then(|f| f.pages.get(p.wrapping_sub(1))).and_then(Option::as_ref);
    let mut entries: Vec<TranscriptEntry> = items
        .iter()
        .map(|it| {
            let pl = page_lines(it.page);
            let line = it.line.and_then(|j| pl.and_then(|p| p.lines.get(j)));
            TranscriptEntry {
                page: it.page,
                line: it.line,
                bbox: line.map(|l| l.bbox),
                ocr_text: match (it.line, line) {
                    (Some(_), Some(l)) => l.text.clone(),
                    (Some(_), None) => String::new(),
                    (None, _) => pl.map(|p| p.text.clone()).unwrap_or_default(),
                },
                ai_text: it.text.clone(),
                status: it.status,
            }
        })
        .collect();
    let mut page_ai: BTreeMap<usize, String> = BTreeMap::new();
    let mut line_ai: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut line_idx: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut order: Vec<usize> = (0..entries.len()).filter(|&i| entries[i].status == ItemStatus::Accepted).collect();
    order.sort_by_key(|&i| (entries[i].page, entries[i].line));
    for i in order {
        let e = &entries[i];
        match e.line {
            None => {
                page_ai.insert(e.page, e.ai_text.clone());
            }
            Some(_) => {
                line_ai.entry(e.page).or_default().push((e.ocr_text.clone(), e.ai_text.clone()));
                line_idx.entry(e.page).or_default().push(i);
            }
        }
    }
    let merged = lines.and_then(|f| pl_ocr::uncertain::merge_content(f, &page_ai, &line_ai));
    match merged {
        Some((content, applied)) => {
            // line replacements whose OCR text was not found in the page text did not go in
            let applied: std::collections::BTreeSet<usize> =
                applied.into_iter().filter_map(|(p, j)| line_idx.get(&p).and_then(|v| v.get(j)).copied()).collect();
            for v in line_idx.values() {
                for &i in v {
                    if !applied.contains(&i) {
                        entries[i].status = ItemStatus::Skipped;
                    }
                }
            }
            if entries.iter().any(|e| e.status == ItemStatus::Accepted) {
                new_text = Some(content);
            }
        }
        None => {
            // no per-page classic text: whole-page texts only if every page was read
            let complete = (1..=pages).all(|p| page_ai.contains_key(&p)) && line_ai.is_empty();
            if complete {
                new_text = Some(page_ai.values().cloned().collect::<Vec<_>>().join("\n\n"));
            } else {
                for e in entries.iter_mut().filter(|e| e.status == ItemStatus::Accepted) {
                    e.status = ItemStatus::Skipped;
                }
            }
        }
    }
    if !entries.iter().any(|e| e.status == ItemStatus::Accepted) {
        new_text = None;
    }
    (entries, new_text)
}

#[cfg(test)]
mod tests {
    // --- PaperRust: KI-Abschrift merge (merge_vision_result) ---------------------------------------
    fn vision_lines() -> pl_ocr::uncertain::LinesFile {
        use pl_ocr::uncertain::{LinesFile, OcrLine, PageLines};
        let line = |t: &str, conf: f32, y: f32| OcrLine { text: t.into(), bbox: [10.0, y, 300.0, y + 30.0], conf, chars: t.len() };
        let p1 = PageLines {
            width: 1000,
            height: 1400,
            text: "Sehr geehrte Damen und Herren
Bifte anrufen!
"
            .into(),
            lines: vec![line("Sehr geehrte Damen und Herren", 95.0, 100.0), line("Bifte anrufen!", 20.0, 200.0)],
            has_confidence: true,
        };
        let p2 = PageLines {
            width: 1000,
            height: 1400,
            text: "Eirkaufe
"
            .into(),
            lines: vec![line("Eirkaufe", 15.0, 100.0)],
            has_confidence: true,
        };
        LinesFile { version: 1, engine: "tesseract".into(), pages: vec![Some(p1), Some(p2)] }
    }

    fn item(page: usize, line: Option<usize>, status: pl_core::vision::ItemStatus, text: &str) -> pl_core::vision::ItemResult {
        pl_core::vision::ItemResult { page, line, status, text: text.into() }
    }

    #[test]
    fn vision_merge_lines_and_pages() {
        use pl_core::vision::ItemStatus::*;
        let f = vision_lines();
        let items = [item(1, Some(1), Accepted, "Bitte anrufen!"), item(2, None, Accepted, "Einkaufen")];
        let (entries, text) = merge_vision_result(Some(&f), &items, 2);
        assert_eq!(
            text.as_deref(),
            Some(
                "Sehr geehrte Damen und Herren
Bitte anrufen!

Einkaufen"
            )
        );
        // the original OCR line is kept in the transcript
        assert_eq!(entries[0].ocr_text, "Bifte anrufen!");
        assert_eq!(entries[0].bbox, Some([10.0, 200.0, 300.0, 230.0]));
        assert_eq!(
            entries[1].ocr_text,
            "Eirkaufe
"
        );
    }

    #[test]
    fn vision_merge_keeps_ocr_on_loop() {
        use pl_core::vision::ItemStatus::*;
        let f = vision_lines();
        // page 2 looped → classic text of page 2 stays; nothing accepted → content unchanged
        let (entries, text) = merge_vision_result(Some(&f), &[item(2, None, Loop, "Eink")], 2);
        assert_eq!(text, None);
        assert_eq!(entries[0].status, Loop);
        // a line whose OCR text is not in the page text is not applied (status skipped)
        let mut g = vision_lines();
        if let Some(Some(p)) = g.pages.get_mut(0) {
            p.text = "anderer Text".into();
        }
        let (entries, text) = merge_vision_result(Some(&g), &[item(1, Some(1), Accepted, "Bitte anrufen!")], 2);
        assert_eq!(text, None);
        assert_eq!(entries[0].status, Skipped);
    }

    #[test]
    fn vision_merge_without_line_data() {
        use pl_core::vision::ItemStatus::*;
        // all pages read → AI text replaces the content (previous behavior)
        let items = [item(1, None, Accepted, "Seite eins"), item(2, None, Accepted, "Seite zwei")];
        let (_, text) = merge_vision_result(None, &items, 2);
        assert_eq!(
            text.as_deref(),
            Some(
                "Seite eins

Seite zwei"
            )
        );
        // one page missing (limit/loop) → classic text stays for the whole document
        let items = [item(1, None, Accepted, "Seite eins"), item(2, None, Skipped, "")];
        let (entries, text) = merge_vision_result(None, &items, 2);
        assert_eq!(text, None);
        assert_eq!(entries[0].status, Skipped);
    }

    use super::*;

    /// Queue that rejects the first `fail` calls like a dropped DB.
    struct FlakyQueue {
        fail: std::sync::atomic::AtomicUsize,
        enqueued: std::sync::Mutex<Vec<(String, Vec<Value>)>>,
    }

    impl pl_queue::TaskQueue for FlakyQueue {
        fn enqueue<'a>(
            &'a self,
            task_name: &'a str,
            _kwargs: serde_json::Map<String, Value>,
            opts: pl_queue::EnqueueOptions,
        ) -> futures::future::BoxFuture<'a, Result<uuid::Uuid, pl_queue::QueueError>> {
            Box::pin(async move {
                use std::sync::atomic::Ordering;
                if self.fail.load(Ordering::SeqCst) > 0 {
                    self.fail.fetch_sub(1, Ordering::SeqCst);
                    return Err(pl_queue::QueueError::Invalid("database unavailable".into()));
                }
                self.enqueued.lock().unwrap().push((task_name.to_string(), opts.args));
                Ok(uuid::Uuid::nil())
            })
        }
        fn status(&self, _id: uuid::Uuid) -> futures::future::BoxFuture<'_, Result<Option<pl_queue::TaskInfo>, pl_queue::QueueError>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// tests/chaos c1 (2026-11-18): index writing and enqueueing fail during a DB outage;
    /// the index task must still be enqueued after the DB returns (before: lost until
    /// restart).
    #[tokio::test]
    async fn index_update_scheduled_after_queue_recovers() {
        let q = Arc::new(FlakyQueue { fail: 3.into(), enqueued: Default::default() });
        let delays = [std::time::Duration::from_millis(1); 5];
        assert!(retry_schedule_index(q.clone(), 250, &delays).await);
        let got = q.enqueued.lock().unwrap().clone();
        assert_eq!(got, vec![("documents.tasks.index_document".to_string(), vec![Value::from(250)])]);
        // permanently gone: gives up after the attempts (with an error message) instead of running endlessly
        let q = Arc::new(FlakyQueue { fail: 100.into(), enqueued: Default::default() });
        assert!(!retry_schedule_index(q.clone(), 7, &delays[..2]).await);
        assert!(q.enqueued.lock().unwrap().is_empty());
    }

    /// D-102: upload spool cleaned up after success (file already gone) and failure (file there);
    /// files outside `SCRATCH_DIR/tmp…/` stay.
    #[test]
    fn upload_spool_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("paperless");
        // Success: original already moved, empty directory remains
        let ok_dir = scratch.join("tmpabc");
        std::fs::create_dir_all(&ok_dir).unwrap();
        cleanup_upload_spool(&scratch, &ok_dir.join("a.pdf"));
        assert!(!ok_dir.exists());
        // Failure: file is still there
        let bad_dir = scratch.join("tmpdef");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("big.pdf"), b"x").unwrap();
        cleanup_upload_spool(&scratch, &bad_dir.join("big.pdf"));
        assert!(!bad_dir.exists());
        // Mail attachment
        let mail_dir = scratch.join("paperless-mail-1234abcd");
        std::fs::create_dir_all(&mail_dir).unwrap();
        std::fs::write(mail_dir.join("att.pdf"), b"x").unwrap();
        cleanup_upload_spool(&scratch, &mail_dir.join("att.pdf"));
        assert!(!mail_dir.exists());
        // foreign file in the spool directory → directory stays
        let other = scratch.join("tmpghi");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("x.pdf"), b"x").unwrap();
        std::fs::write(other.join("keep.txt"), b"x").unwrap();
        cleanup_upload_spool(&scratch, &other.join("x.pdf"));
        assert!(other.join("keep.txt").exists() && !other.join("x.pdf").exists());
        // Input folder: delete nothing
        let consume = root.path().join("consume").join("tmpsub");
        std::fs::create_dir_all(&consume).unwrap();
        std::fs::write(consume.join("c.pdf"), b"x").unwrap();
        cleanup_upload_spool(&scratch, &consume.join("c.pdf"));
        assert!(consume.join("c.pdf").exists());
        // directly in the scratch folder (no tmp subfolder): delete nothing
        std::fs::write(scratch.join("d.pdf"), b"x").unwrap();
        cleanup_upload_spool(&scratch, &scratch.join("d.pdf"));
        assert!(scratch.join("d.pdf").exists());
    }

    /// tests/chaos C5: working folders left behind after a crash, spool stays.
    #[test]
    fn stale_scratch_sweep() {
        let d = tempfile::tempdir().unwrap();
        let s = d.path();
        let work = s.join(format!("paperless-ngx{}", uuid::Uuid::new_v4().simple()));
        let parse = s.join(format!("tmp{}", uuid::Uuid::new_v4().simple()));
        let spool = s.join("tmpab12cd34");
        let mail = s.join("paperless-mail-x1y2z3");
        for p in [&work, &parse, &spool, &mail] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("f.pdf"), b"%PDF").unwrap();
        }
        // fresh: delete nothing (may still be running)
        assert_eq!(sweep_stale_scratch(s, Duration::from_secs(3600)), 0);
        // old enough: only the consumer's working folders
        assert_eq!(sweep_stale_scratch(s, Duration::ZERO), 2);
        assert!(!work.exists() && !parse.exists());
        assert!(spool.join("f.pdf").exists() && mail.join("f.pdf").exists());
        assert!(is_consumer_workdir(&format!("tmp{}", "a".repeat(32))));
        assert!(!is_consumer_workdir("tmp3fou57zi"));
    }

    /// tests/chaos C1/C4: which errors count as transient (keep file, retry).
    #[test]
    fn transient_failure_texts() {
        let io = sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset by peer"));
        assert!(is_db_unavailable_text(&format!("x.pdf: The following error occurred while storing document x.pdf after parsing: {io}")));
        assert!(is_db_unavailable_text(&sqlx::Error::PoolTimedOut.to_string()));
        assert!(is_db_unavailable_text(&sqlx::Error::PoolClosed.to_string()));
        assert!(is_db_unavailable_text("error returned from database: the database system is starting up"));
        assert!(!is_db_unavailable_text("x.pdf: Cannot consume x.pdf: File not found."));
        assert!(!is_db_unavailable_text("duplicate key value violates unique constraint"));
        assert!(is_service_unreachable_text("Azure AI Vision parsing failed: error sending request"));
        assert!(!is_service_unreachable_text("Azure AI Vision parsing failed: (InvalidRequest) bad document"));
        assert!(!is_service_unreachable_text("error sending request"));
        assert_eq!(ConsumeError::Transient("x".into()).error_type(), "OperationalError");
        // C3: disk full / no write permission
        assert!(is_storage_error(&std::io::Error::from(std::io::ErrorKind::StorageFull)));
        assert!(is_storage_error(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(is_storage_error(&std::io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 })));
        assert!(!is_storage_error(&std::io::Error::from(std::io::ErrorKind::NotFound)));
        let m = store_err(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(m.starts_with(STORAGE_MARK));
        assert!(store_err(std::io::Error::from(std::io::ErrorKind::NotFound)).find(STORAGE_MARK).is_none());
    }

    #[test]
    fn helpers() {
        assert_eq!(thousands(4_294_967_295), "4,294,967,295");
        assert_eq!(thousands(0), "0");
        assert_eq!(py_stem("dir/a.b.pdf"), "a.b");
        assert_eq!(py_stem(".hidden"), ".hidden");
        let d = DateTime::parse_from_rfc3339("2024-01-15T10:00:00+01:00").unwrap();
        assert_eq!(
            datetime_attr(&d),
            CreatedAttr::DateTime { str_: "2024-01-15 10:00:00+01:00".into(), iso: "2024-01-15T10:00:00+01:00".into() }
        );
        let t = SystemTime::UNIX_EPOCH + Duration::from_micros(1_700_000_000_123_456);
        let m = mtime_datetime(t, chrono_tz::Europe::Zurich);
        assert_eq!(m.to_rfc3339(), "2023-11-14T23:13:20.123456+01:00");
    }

    #[test]
    fn working_file_name_stays_inside() {
        // F4: drive-relative, stream, paths, device names, trailing dots
        assert_eq!(working_file_name("C:start.bat"), "C_start.bat");
        assert_eq!(working_file_name("x.pdf:strom"), "x.pdf_strom");
        assert_eq!(working_file_name("..\\..\\evil.pdf"), "evil.pdf");
        assert_eq!(working_file_name("a/b/c.pdf"), "c.pdf");
        assert_eq!(working_file_name("CON.pdf"), "_CON.pdf");
        assert_eq!(working_file_name("com1.txt"), "_com1.txt");
        assert_eq!(working_file_name("ok. "), "ok");
        assert_eq!(working_file_name(".."), "document");
        assert_eq!(working_file_name("Rechnung März.pdf"), "Rechnung März.pdf");
        let dir = std::path::Path::new("work");
        for n in ["C:start.bat", "D:..\\x", "\\\\server\\share\\f.pdf", "/etc/passwd"] {
            let p = dir.join(working_file_name(n));
            assert_eq!(p.parent(), Some(dir), "{n} -> {}", p.display());
        }
    }

    /// Security review 2026-11-19 F9: a DB file name with `..`/absolute path is never moved.
    #[test]
    fn rename_paths_rejects_unsafe_names() {
        let o = std::path::Path::new("/m/originals");
        assert_eq!(rename_paths(o, "a.pdf", "b/c.pdf"), Some((o.join("a.pdf"), o.join("b").join("c.pdf"))));
        for bad in ["../../etc/passwd", "/etc/passwd", "a/../../x.pdf"] {
            assert_eq!(rename_paths(o, bad, "x.pdf"), None, "{bad:?}");
            assert_eq!(rename_paths(o, "x.pdf", bad), None, "{bad:?}");
        }
        // Windows leftovers (F12): rejected on Windows, ordinary names on Linux
        assert_eq!(rename_paths(o, "x.pdf:strom", "y.pdf").is_none(), cfg!(windows));
        assert_eq!(rename_paths(o, "..\\x.pdf", "y.pdf").is_none(), cfg!(windows));
    }

    #[test]
    fn checksum_and_atomic_write() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a.txt");
        std::fs::write(&a, b"abc").unwrap();
        assert_eq!(compute_checksum(&a).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let mut w = Vec::new();
        let target = t.path().join("x/y/b.txt");
        write_atomic(&a, &target, &mut w).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"abc");
        assert_eq!(w, vec![target.clone()]);
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    /// Regression: a redelivered task must not delete a NEW file that meanwhile sits at the same
    /// inbox path; only the stored document's own bytes may be removed.
    #[test]
    fn redelivered_original_deleted_only_on_matching_checksum() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("scan.pdf");
        std::fs::write(&f, b"abc").unwrap();
        let same = compute_checksum(&f).unwrap();
        let other = "0".repeat(64);
        assert_eq!(redelivered_original_action(&f, std::slice::from_ref(&same)), RedeliveredOriginal::Delete);
        assert_eq!(redelivered_original_action(&f, &[other.clone(), same.to_uppercase()]), RedeliveredOriginal::Delete);
        assert!(matches!(redelivered_original_action(&f, std::slice::from_ref(&other)), RedeliveredOriginal::Keep(_)));
        assert!(matches!(redelivered_original_action(&f, &[]), RedeliveredOriginal::Keep(_)), "unknown checksum: keep");
        assert_eq!(redelivered_original_action(&t.path().join("gone.pdf"), &[same]), RedeliveredOriginal::Missing);
        assert!(f.is_file(), "the decision itself never deletes");
    }

    /// Regression: failed deletions and failed reference checks stay in the note; only real
    /// deletions are counted.
    #[test]
    fn pending_files_only_real_deletions_counted() {
        let t = tempfile::tempdir().unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let del = t.path().join("del.pdf");
        std::fs::write(&del, b"x").unwrap();
        let referenced = t.path().join("ref.pdf");
        std::fs::write(&referenced, b"x").unwrap();
        let unknown = t.path().join("unknown.pdf");
        std::fs::write(&unknown, b"x").unwrap();
        let missing = t.path().join("missing.pdf");
        // a non-empty directory cannot be removed with remove_file: stands for a failed delete
        let undeletable = t.path().join("dir");
        std::fs::create_dir_all(undeletable.join("inner")).unwrap();
        let entries = vec![
            (s(&del), PendingCheck::Unreferenced),
            (s(&referenced), PendingCheck::Referenced),
            (s(&unknown), PendingCheck::Unknown),
            (s(&missing), PendingCheck::Unreferenced),
            (s(&undeletable), PendingCheck::Unreferenced),
        ];
        let (removed, remaining) = remove_unreferenced(&entries);
        assert_eq!(removed, 1);
        assert_eq!(remaining, vec![s(&unknown), s(&undeletable)]);
        assert!(!del.exists());
        assert!(referenced.exists() && unknown.exists() && undeletable.exists());
    }

    #[test]
    fn qpdf_replacement_exit_codes() {
        let t = tempfile::tempdir().unwrap();
        // no PDF → exit 2, file unchanged
        let p = t.path().join("x.pdf");
        std::fs::write(&p, b"not a pdf at all").unwrap();
        let e = clean_with_qpdf(&p).unwrap_err();
        assert!(e.starts_with("Command '['qpdf', '--replace-input', "), "{e}");
        assert!(e.ends_with("]' returned non-zero exit status 2."), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), b"not a pdf at all");
        // clean PDF with garbage before it → exit 0, header at the front
        let mut doc = lopdf::Document::with_version("1.4");
        let pages = doc.new_object_id();
        let page = doc.add_object(
            lopdf::dictionary! {"Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 10.into(), 10.into()]},
        );
        doc.objects
            .insert(pages, lopdf::Object::Dictionary(lopdf::dictionary! {"Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1}));
        let cat = doc.add_object(lopdf::dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", cat);
        let mut bytes = b"JUNK".to_vec();
        doc.save_to(&mut bytes).unwrap();
        // garbage before the header (offsets count from the header, like qpdf) + wrong startxref → qpdf
        // rebuilds the cross-references and ends with exit 3
        let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
        bytes.truncate(at);
        bytes.extend_from_slice(
            b"startxref
999999
%%EOF
",
        );
        std::fs::write(&p, &bytes).unwrap();
        let e = clean_with_qpdf(&p).unwrap_err();
        assert!(e.ends_with("exit status 3."), "{e}");
        assert!(std::fs::read(&p).unwrap().starts_with(b"%PDF-"));
        // now clean → exit 0
        assert_eq!(clean_with_qpdf(&p), Ok(()));
        assert!(py_path_repr(Path::new("a'b")).ends_with("Path(\"a'b\")"));
        assert!(py_path_repr(Path::new("x/y.pdf")).ends_with("Path('x/y.pdf')"));
    }

    #[test]
    fn plugin_names() {
        assert_eq!(Plugin::Preflight.name(), "ConsumerPreflightPlugin");
        assert_eq!(Plugin::WorkflowTrigger.name(), "WorkflowTriggerPlugin");
        assert_eq!(ConsumeError::Consumer("x".into()).error_type(), "ConsumerError");
    }

    /// `blocking_io` on both runtime flavors (block_in_place only on the multi-threaded one,
    /// which would panic on a current-thread runtime) and outside a runtime.
    #[test]
    fn blocking_io_all_runtimes() {
        assert_eq!(blocking_io(|| 1), 1);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(rt.block_on(async { blocking_io(|| 2) }), 2);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap();
        let v = rt.block_on(async { tokio::spawn(async { blocking_io(|| 3) }).await.unwrap() });
        assert_eq!(v, 3);
    }

    #[test]
    fn store_fail_from_text() {
        assert!(matches!(StoreFail::from("x"), StoreFail::Error(e) if e == "x"));
        assert!(matches!(StoreFail::from(String::from("y")), StoreFail::Error(e) if e == "y"));
    }
}
