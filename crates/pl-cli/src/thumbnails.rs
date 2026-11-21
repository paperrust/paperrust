//! `document_thumbnails [-d ID] [--processes N]`: regenerate thumbnails.
//!
//! upstream: src/documents/management/commands/document_thumbnails.py (v3.4.2):
//! `get_parser_registry().get_parser_for_file(mime, original_filename, source_path)` →
//! `parser.get_thumbnail(source_path, mime)` → `shutil.move(thumb, thumbnail_path)`; documents
//! via `process_parallel` (B7: truly parallel here, see [`crate::par`]), errors as
//! "Failed document {id}: {error}".
//! Parser/rendering: pl-consumer `NativeParsing` (pl-parsers, pl-ocr/PDFium). Without PDFium the default
//! image is produced on a render error, as upstream (`documents/resources/document.webp`).

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::Row;

use crate::CliResult;
use crate::args::{self, value};
use crate::ctx::Ctx;
use pl_consumer::stages::{DocumentParsing, ParseRequest};

const OPTS: &[args::Opt] = &[args::PROGRESS, args::PROCESSES, value(&["-d", "--document"], "document")];

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let processes = crate::par::processes(&a)?;
    let c = Ctx::connect().await?;
    let doc = a.int("document")?;
    let (_ok, failed) = regenerate_parallel(&c, doc.filter(|d| *d != 0), processes, true).await?;
    let _ = failed;
    Ok(())
}

/// Returns: (successful, [(id, error)]), sequentially.
pub async fn regenerate(c: &Ctx, only: Option<i64>) -> CliResult<(usize, Vec<(i64, String)>)> {
    regenerate_parallel(c, only, 1, false).await
}

/// One document (row from `documents_document`).
#[derive(Debug, Clone)]
struct Item {
    id: i64,
    mime: String,
    original: Option<String>,
    filename: Option<String>,
}

/// Shared state of the tasks.
struct Job {
    parsing: pl_consumer::native::NativeParsing,
    originals_dir: PathBuf,
    thumbnail_dir: PathBuf,
    scratch_dir: PathBuf,
    remote_tiff: bool,
}

impl Job {
    /// `_process_document(doc_id)`; `Ok(false)` = no parser (upstream: warning, no error).
    fn one(&self, it: &Item) -> Result<bool, String> {
        let id = it.id;
        let fname = match it.filename.as_deref() {
            Some(f) if !f.is_empty() => f.to_string(),
            _ => format!("{id:07}{}", pl_files::naming::default_file_extension(&it.mime, self.remote_tiff && it.mime == "image/tiff")),
        };
        // Security review 2026-11-19 F9: DB name only via safe_join; unsafe → parser finds nothing
        let source = pl_consumer::files::join(&self.originals_dir, &fname);
        if source.ends_with(pl_consumer::files::UNSAFE_MARKER) {
            return Err("original file name is unsafe, treated as missing".into());
        }
        let original = it.original.as_deref().unwrap_or("");
        let Some(parser) = self.parsing.parser_for(&it.mime, original, &source, true) else {
            tracing::warn!(target: "paperless.management.thumbnails", "Document {id}: No parser for mime type {}", it.mime);
            return Ok(false);
        };
        let tmp = self.scratch_dir.join(format!("paperless-thumb-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let req = ParseRequest { path: &source, mime_type: &it.mime, filename: original, mailrule_id: None, tempdir: &tmp };
        let res = match self.parsing.thumbnail(&parser, &req, None) {
            Ok(thumb) => {
                let dst = self.thumbnail_dir.join(format!("{id:07}.webp"));
                if std::fs::rename(&thumb, &dst).is_err() {
                    crate::export::copy_file_with_basic_stats(&thumb, &dst).map(|_| true).map_err(|e| e.to_string())
                } else {
                    Ok(true)
                }
            }
            Err(e) => Err(format!("{e:?}")),
        };
        let _ = std::fs::remove_dir_all(&tmp);
        res
    }
}

/// All (or one) document(s) with at most `processes` concurrent tasks.
/// `print_failures`: print "Failed document {id}: {error}" immediately (like upstream).
pub async fn regenerate_parallel(
    c: &Ctx,
    only: Option<i64>,
    processes: usize,
    print_failures: bool,
) -> CliResult<(usize, Vec<(i64, String)>)> {
    let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "SELECT id, mime_type, original_filename, filename FROM documents_document WHERE deleted_at IS NULL",
    );
    if let Some(d) = only {
        q.push(" AND id = ").push_bind(i32::try_from(d).unwrap_or(-1));
    }
    q.push(" ORDER BY created DESC");
    let rows = q.build().fetch_all(&c.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(Item { id: i64::from(r.try_get::<i32, _>(0)?), mime: r.try_get(1)?, original: r.try_get(2)?, filename: r.try_get(3)? });
    }
    let ocr_settings = pl_ocr::OcrSettings::from_process_env().map_err(|e| crate::CliError::Other(e.to_string()))?;
    let app = pl_consumer::consumer::load_app_config(&c.pool).await?;
    let ocr = pl_ocr::OcrConfig::new(&ocr_settings, &app).map_err(|e| crate::CliError::Other(e.to_string()))?;
    let app_cfg = pl_files::db::app_config(&c.pool).await?;
    std::fs::create_dir_all(&c.cfg.scratch_dir)?;
    std::fs::create_dir_all(&c.cfg.thumbnail_dir)?;
    let job = Arc::new(Job {
        parsing: pl_consumer::native::NativeParsing::new(ocr, c.cfg.tz),
        originals_dir: c.cfg.originals_dir.clone(),
        thumbnail_dir: c.cfg.thumbnail_dir.clone(),
        scratch_dir: c.cfg.scratch_dir.clone(),
        remote_tiff: pl_files::db::remote_ocr_active(app_cfg.as_ref(), &c.cfg),
    });
    let mut ok = 0;
    let mut failed = Vec::new();
    crate::par::process_parallel(
        items,
        processes,
        |it: Item| {
            let job = job.clone();
            async move { tokio::task::spawn_blocking(move || job.one(&it)).await.map_err(|e| e.to_string())? }
        },
        |it, r| match r {
            Ok(true) => ok += 1,
            Ok(false) => {}
            Err(e) => {
                if print_failures {
                    println!("Failed document {}: {e}", it.id);
                }
                failed.push((it.id, e));
            }
        },
    )
    .await;
    Ok((ok, failed))
}
