// Re-ingestion of an existing document (bulk_edit "reprocess", `document_archiver`).
//
// upstream: src/documents/tasks.py@v3.4.2 (update_document_content_maybe_archive_file,
//           _read_barcodes_for_reprocess), src/documents/barcodes.py@v3.4.2 (read_barcode_values,
//           scannable_mime_types), src/documents/caching.py@v3.4.2 (clear_document_caches),
//           django-auditlog 3.4.1 (LogEntry.objects.log_create).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::audit::{AuditLog, document_repr};
use crate::consumer::{Consumer, MediaLock, TempDir, compute_checksum, load_barcode_app_config, move_file};
use crate::data::StoredBarcode;
use crate::db::{self, DocRow, now_micros};
use crate::files;
use crate::stages::{IndexOutcome, ParseRequest, ParsedOutput, StageError};

/// Result of [`Consumer::update_document_content_maybe_archive_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReprocessOutcome {
    /// Content (and, if applicable, archive/thumbnail) updated.
    Updated,
    /// No parser for the MIME type (upstream: error logged, task successful).
    NoParser,
    /// Error while parsing/saving – logged like upstream (`logger.exception`), the
    /// task is still successful; database rolled back.
    Failed(String),
}

/// Errors that make the task fail upstream (`Document.objects.get` outside the try).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReprocessError {
    /// `Document.DoesNotExist`
    #[error("Document matching query does not exist.")]
    DoesNotExist,
    #[error("{0}")]
    Db(String),
}

impl ReprocessError {
    /// Python class name for `PaperlessTask.result`.
    pub fn error_type(&self) -> &'static str {
        match self {
            ReprocessError::DoesNotExist => "DoesNotExist",
            ReprocessError::Db(_) => "OperationalError",
        }
    }
}

impl From<sqlx::Error> for ReprocessError {
    fn from(e: sqlx::Error) -> Self {
        ReprocessError::Db(e.to_string())
    }
}

/// Values of a `documents_documentbarcode` row (`StoredBarcode`), read from the original.
type Barcodes = Option<Vec<StoredBarcode>>;

/// `str(e)` of an exception in the try block.
struct Fail(String);

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail(e.to_string())
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail(e.to_string())
    }
}

impl Consumer {
    /// `update_document_content_maybe_archive_file(document_id, *, remote_ocr=False)`:
    /// parser again on `source_path`, content (+ archive file with checksum and new
    /// `archive_filename`) via `UPDATE` without `save()` (no file renaming), audit entry
    /// "Update document content", barcodes anew (with `CONSUMER_STORE_BARCODE_VALUES`), move files under
    /// `MEDIA_LOCK`, search index, caches. `page_count` and `checksum` stay unchanged.
    pub async fn update_document_content_maybe_archive_file(
        &self,
        document_id: i64,
        remote_ocr: bool,
    ) -> Result<ReprocessOutcome, ReprocessError> {
        self.update_document_content_maybe_archive_file_with(document_id, remote_ocr, false).await
    }

    /// Like [`Self::update_document_content_maybe_archive_file`]; PaperRust `ai_ocr` = AI text recognition
    /// chosen for this document (pl-core `vision`; otherwise the AI rules "always"/"little text" apply).
    pub async fn update_document_content_maybe_archive_file_with(
        &self,
        document_id: i64,
        remote_ocr: bool,
        ai_ocr: bool,
    ) -> Result<ReprocessOutcome, ReprocessError> {
        let document = {
            let mut conn = self.pool.acquire().await?;
            // Document.objects.get(id=document_id) (without trash)
            match db::load_document(&mut conn, document_id).await? {
                Some(d) if d.deleted_at.is_none() => d,
                _ => return Err(ReprocessError::DoesNotExist),
            }
        };
        let source = files::source_path(&self.settings, &document);
        let mime_type = document.mime_type.clone();
        let original_filename = document.original_filename.clone().unwrap_or_default();
        let allow_remote = remote_ocr || self.remote_ocr_by_default;
        let Some(parser) = self.parsing.parser_for(&mime_type, &original_filename, &source, allow_remote) else {
            let repr = self.document_str(&document).await;
            tracing::error!(
                target: "paperless.tasks",
                "No parser found for mime type {mime_type}, cannot archive document {repr} (ID: {document_id})"
            );
            return Ok(ReprocessOutcome::NoParser);
        };
        match self.reprocess_inner(&document, &source, &parser, &original_filename, ai_ocr).await {
            Ok(()) => Ok(ReprocessOutcome::Updated),
            Err(Fail(e)) => {
                let repr = self.document_str(&document).await;
                tracing::error!(target: "paperless.tasks", "Error while parsing document {repr} (ID: {document_id}): {e}");
                Ok(ReprocessOutcome::Failed(e))
            }
        }
    }

    async fn document_str(&self, d: &DocRow) -> String {
        let corr = match (self.pool.acquire().await, d.correspondent_id) {
            (Ok(mut c), Some(id)) => db::object_name(&mut c, "documents_correspondent", id).await.ok().flatten(),
            _ => None,
        };
        document_repr(&crate::audit::CreatedAttr::Date(d.created), corr.as_deref(), &d.title)
    }

    async fn reprocess_inner(
        &self,
        document: &DocRow,
        source: &Path,
        parser: &crate::stages::ParserInfo,
        filename: &str,
        ai_ocr: bool,
    ) -> Result<(), Fail> {
        let work = TempDir::new(&self.settings.scratch_dir, "paperless-reprocess-")?;
        let mime_type = document.mime_type.clone();
        let barcode_settings = match load_barcode_app_config(&self.pool).await {
            Ok(c) => self.settings.barcodes.with_app_config(&c),
            Err(e) => {
                tracing::warn!(target: "paperless.barcodes", "Could not read application configuration, using environment: {e}");
                self.settings.barcodes.clone()
            }
        };
        // parse + barcodes + thumbnail (blockierend)
        let (parsed, barcodes, thumbnail) = {
            let parsing = self.parsing.clone();
            let barcodes_scanner = self.barcodes.clone();
            let pages = self.pages.clone();
            let parser = parser.clone();
            let src = source.to_path_buf();
            let mt = mime_type.clone();
            let fname = filename.to_string();
            let tmp = work.path.clone();
            let bs = barcode_settings.clone();
            let repr = self.document_str(document).await;
            tokio::task::spawn_blocking(move || -> Result<(ParsedOutput, Barcodes, PathBuf), StageError> {
                let ptmp = tmp.join("parser");
                std::fs::create_dir_all(&ptmp).map_err(|e| StageError::Other(e.to_string()))?;
                let req = ParseRequest { path: &src, mime_type: &mt, filename: &fname, mailrule_id: None, tempdir: &ptmp };
                let produce_archive = parsing.should_produce_archive(&parser, &mt, &src);
                let out = parsing.parse(&parser, &req, produce_archive)?;
                let barcodes = read_barcodes_for_reprocess(barcodes_scanner.as_ref(), pages.as_ref(), &bs, &src, &mt, &tmp, &repr);
                let thumb = parsing.thumbnail_with_text(&parser, &req, out.archive_path.as_deref(), out.text.as_deref())?;
                Ok((out, barcodes, thumb))
            })
            .await
            .map_err(|e| Fail(e.to_string()))?
            .map_err(|e| Fail(e.to_string()))?
        };
        let mut text_opt = parsed.text.clone();
        // PaperRust: AI text recognition (chosen, "always" or little text); page count as stored
        self.apply_vision(ai_ocr, source, &mime_type, parsed.archive_path.as_deref(), &mut text_opt, document.page_count).await;
        let text = text_opt.unwrap_or_default();
        let archive = parsed.archive_path.clone().filter(|p| p.is_file());
        let audit = AuditLog { enabled: self.settings.audit_log_enabled, actor_id: None };

        // with transaction.atomic():
        let mut tx = self.pool.begin().await?;
        let old = db::load_document(&mut tx, document.id).await?.ok_or_else(|| Fail("Document matching query does not exist.".into()))?;
        let old_repr = {
            let corr = match old.correspondent_id {
                Some(id) => db::object_name(&mut tx, "documents_correspondent", id).await?,
                None => None,
            };
            document_repr(&crate::audit::CreatedAttr::Date(old.created), corr.as_deref(), &old.title)
        };
        let mut new_archive_filename: Option<String> = None;
        if let Some(a) = &archive {
            let checksum = compute_checksum(a)?;
            // generate_unique_filename(document, archive_filename=True) with the instance from the start
            let ctx = crate::templating::document_context(&mut tx, document).await?;
            let archive_dir = self.settings.archive_dir.clone();
            let name = pl_templating::generate_unique_filename(&ctx, true, &self.settings.templating, |rel| {
                files::join(&archive_dir, rel).exists()
            })
            .map_err(|e| Fail(format!("{e:?}")))?;
            sqlx::query("UPDATE documents_document SET archive_checksum = $2, content = $3, archive_filename = $4 WHERE id = $1")
                .bind(document.id as i32)
                .bind(&checksum)
                .bind(&text)
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            let mut changes = Map::new();
            changes.insert("content".into(), json!([old.content, text]));
            changes.insert("archive_checksum".into(), json!([old.archive_checksum, checksum]));
            changes.insert("archive_filename".into(), json!([old.archive_filename, name]));
            log_update_content(&mut tx, &audit, document.id, &old_repr, changes).await?;
            new_archive_filename = Some(name);
        } else {
            sqlx::query("UPDATE documents_document SET content = $2 WHERE id = $1")
                .bind(document.id as i32)
                .bind(&text)
                .execute(&mut *tx)
                .await?;
            let mut changes = Map::new();
            changes.insert("content".into(), json!([old.content, text]));
            log_update_content(&mut tx, &audit, document.id, &old_repr, changes).await?;
        }
        if let Some(codes) = &barcodes {
            sqlx::query("DELETE FROM documents_documentbarcode WHERE document_id = $1").bind(document.id as i32).execute(&mut *tx).await?;
            for c in codes {
                sqlx::query("INSERT INTO documents_documentbarcode (document_id, page, value, format) VALUES ($1, $2, $3, $4)")
                    .bind(document.id as i32)
                    .bind(c.page as i32)
                    .bind(&c.value)
                    .bind(&c.format)
                    .execute(&mut *tx)
                    .await?;
            }
            // metadata_etag includes modified
            sqlx::query("UPDATE documents_document SET modified = $2 WHERE id = $1")
                .bind(document.id as i32)
                .bind(now_micros())
                .execute(&mut *tx)
                .await?;
        }
        {
            let _lock = MediaLock::acquire(&self.settings.media_lock()).await?;
            if let (Some(a), Some(name)) = (&archive, &new_archive_filename) {
                // document.archive_path (archive_filename of the instance was set)
                let target = files::join(&self.settings.archive_dir, name);
                pl_templating::create_source_path_directory(&target)?;
                move_file(a, &target)?;
            }
            let thumb_target = self.settings.thumbnail_path(document.id);
            if let Some(p) = thumb_target.parent() {
                std::fs::create_dir_all(p)?;
            }
            move_file(&thumbnail, &thumb_target)?;
        }
        tx.commit().await?;

        // document.refresh_from_db()
        let mut conn = self.pool.acquire().await?;
        let fresh = db::load_document(&mut conn, document.id).await?;
        // FINDINGS B2: index and caches fetch their own connections – return this one beforehand
        drop(conn);
        let checksum = fresh.as_ref().and_then(|d| d.archive_checksum.clone());
        tracing::info!(
            target: "paperless.tasks",
            "Updating index for document {} ({})",
            document.id,
            checksum.as_deref().unwrap_or("None")
        );
        match self.index.add_or_update(&self.pool, document.id).await {
            Ok(IndexOutcome::Deferred) => {
                tracing::warn!(target: "paperless.tasks", "Search index locked, document {} not re-indexed", document.id)
            }
            Ok(_) => {}
            Err(e) => return Err(Fail(e)),
        }
        // llm_index_add_or_update_document: worker (pl-ai) after success
        self.caches.clear_document_caches(document.id).await;
        if let Some(root) = fresh.and_then(|d| d.root_document_id) {
            self.caches.clear_document_caches(root).await;
        }
        drop(work);
        Ok(())
    }
}

/// `LogEntry.objects.log_create(instance=oldDocument, changes=…, additional_data={"reason": …},
/// action=UPDATE)` (only with `AUDIT_LOG_ENABLED`).
async fn log_update_content(
    conn: &mut sqlx::PgConnection,
    audit: &AuditLog,
    doc_id: i64,
    repr: &str,
    changes: Map<String, Value>,
) -> Result<(), sqlx::Error> {
    audit
        .write(conn, "document", doc_id, repr, crate::audit::ACTION_UPDATE, changes, Some(json!({"reason": "Update document content"})))
        .await
}

/// `_read_barcodes_for_reprocess`: `None` without `CONSUMER_STORE_BARCODE_VALUES`, for a
/// non-scannable type (`scannable_mime_types`) and on scan errors (warning).
#[allow(clippy::too_many_arguments)]
fn read_barcodes_for_reprocess(
    scanner: &dyn crate::stages::BarcodeScanner,
    pages: &dyn crate::stages::PdfPageOps,
    settings: &crate::settings::BarcodeSettings,
    source: &Path,
    mime_type: &str,
    tmp: &Path,
    repr: &str,
) -> Barcodes {
    if !settings.barcode_store_values {
        return None;
    }
    let scannable = mime_type == "application/pdf" || (settings.barcode_enable_tiff_support && mime_type == "image/tiff");
    if !scannable {
        return None;
    }
    let work = tmp.join("barcodes");
    let r: Result<Vec<StoredBarcode>, String> = (|| {
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        if !scanner.available() {
            return Err("no barcode scanner available".into());
        }
        let pdf =
            if mime_type == "image/tiff" { pages.tiff_to_pdf(source, &work).map_err(|e| e.to_string())? } else { source.to_path_buf() };
        let codes = scanner.scan(&pdf, &work, &settings.scan_settings()).map_err(|e| e.to_string())?;
        Ok(codes.into_iter().map(|c| StoredBarcode { page: c.page + 1, value: c.value, format: c.format }).collect())
    })();
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(target: "paperless.tasks", "Could not read barcodes of document {repr}: {e}");
            None
        }
    }
}
