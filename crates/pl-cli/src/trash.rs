//! `empty_trash`: empty the trash like the task `documents.tasks.empty_trash`
//! (upstream without its own manage command; beat "Empty trash" daily).
//!
//! upstream: src/documents/tasks.py (`empty_trash`: `Document.deleted_objects` older than
//! `EMPTY_TRASH_DELAY` days or `doc_ids`, hard delete with collector cascade, then
//! remove LogEntries of the deleted documents), src/documents/signals/handlers.py
//! (`cleanup_document_deletion`: move original to `EMPTY_TRASH_DIR` or delete, delete archive
//! and thumbnail, remove empty folders) – v3.4.2.

use std::path::{Path, PathBuf};

use crate::args::{self, value};
use crate::ctx::Ctx;
use crate::export::descriptor;
use crate::export::medialock::MediaLock;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[args::Opt { names: &["--doc-ids"], dest: "doc_ids", nargs: 1 }, value(&["--delay-days"], "delay")];

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let ids = match a.value("doc_ids") {
        Some(s) => Some(
            s.split(',')
                .filter(|x| !x.trim().is_empty())
                .map(|x| x.trim().parse::<i64>().map_err(|_| CliError::Usage(format!("invalid id '{x}'"))))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let c = Ctx::connect().await?;
    let n = empty_trash(&c, ids.as_deref()).await?;
    println!("Deleted {n} documents from trash");
    Ok(())
}

fn stem_suffix(name: &str) -> (String, String) {
    let s = crate::export::pypath::suffix(name).to_string();
    (name[..name.len() - s.len()].to_string(), s)
}

fn cleanup_files(c: &Ctx, d: &pl_consumer::db::DocRow, file_type: &str) {
    let fname = match d.filename.as_deref() {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => format!("{:07}{file_type}", d.id),
    };
    let source = c.source_path(&fname);
    let archive: Option<PathBuf> = d.archive_filename.as_deref().map(|a| c.archive_path(a));
    let thumb = c.thumbnail_path(d.id);
    if let Some(trash) = &c.cfg.empty_trash_dir {
        let name = source.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let (base, ext) = stem_suffix(&name);
        let mut counter = 0;
        let target = loop {
            let t = trash.join(format!("{base}{}{ext}", if counter > 0 { format!("_{counter:02}") } else { String::new() }));
            if t.exists() {
                counter += 1;
            } else {
                break t;
            }
        };
        let moved = std::fs::rename(&source, &target)
            .or_else(|_| crate::export::copy_file_with_basic_stats(&source, &target).and_then(|_| std::fs::remove_file(&source)));
        if let Err(e) = moved {
            tracing::error!(target: "paperless.handlers", "Failed to move {} to trash at {}: {e}. Skipping cleanup!", source.display(), target.display());
            return;
        }
    }
    let mut files: Vec<PathBuf> = archive.iter().cloned().collect();
    files.push(thumb);
    if c.cfg.empty_trash_dir.is_none() {
        files.push(source.clone());
    }
    for f in files {
        if f.is_file() {
            if let Err(e) = std::fs::remove_file(&f) {
                tracing::warn!(target: "paperless.handlers", "While deleting document {}, the file {} could not be deleted: {e}", d.id, f.display());
            }
        } else {
            tracing::warn!(target: "paperless.handlers", "Expected {} to exist, but it did not", f.display());
        }
    }
    if let Some(p) = source.parent() {
        pl_templating::delete_empty_directories(p, &c.cfg.originals_dir);
    }
    if let Some(a) = &archive
        && let Some(p) = Path::new(a).parent()
    {
        pl_templating::delete_empty_directories(p, &c.cfg.archive_dir);
    }
}

/// `empty_trash(doc_ids)`; returns the number of selected (deleted) documents.
pub async fn empty_trash(c: &Ctx, doc_ids: Option<&[i64]>) -> CliResult<usize> {
    let d = descriptor::get();
    let mut tx = c.pool.begin().await?;
    let selected: Vec<i32> = match doc_ids {
        Some(ids) => {
            let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
            sqlx::query_scalar("SELECT id FROM documents_document WHERE deleted_at IS NOT NULL AND id = ANY($1) ORDER BY id")
                .bind(&ids32)
                .fetch_all(&mut *tx)
                .await?
        }
        None => {
            tracing::info!(target: "paperless.tasks", "Emptying trash of all expired documents");
            sqlx::query_scalar(
                "SELECT id FROM documents_document WHERE deleted_at IS NOT NULL AND deleted_at < now() - make_interval(days => $1) ORDER BY id",
            )
            .bind(c.cfg.empty_trash_delay as i32)
            .fetch_all(&mut *tx)
            .await?
        }
    };
    // cascade root_document → versions (preload for the files)
    let mut all: Vec<i32> = selected.clone();
    let mut frontier = selected.clone();
    while !frontier.is_empty() {
        let next: Vec<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE root_document_id = ANY($1)")
            .bind(&frontier)
            .fetch_all(&mut *tx)
            .await?;
        frontier = next.into_iter().filter(|i| !all.contains(i)).collect();
        all.extend(frontier.iter().copied());
    }
    let mut rows = Vec::new();
    for id in &all {
        if let Some(r) = pl_consumer::db::load_document(&mut tx, i64::from(*id)).await? {
            rows.push(r);
        }
    }
    let ids64: Vec<i64> = all.iter().map(|i| i64::from(*i)).collect();
    crate::export::importer::cascade_delete(&mut tx, d, "documents.document", ids64).await?;
    if c.cfg.audit_log_enabled && !selected.is_empty() {
        sqlx::query(
            "DELETE FROM auditlog_logentry WHERE content_type_id = (SELECT id FROM django_content_type WHERE app_label='documents' AND model='document')
             AND object_id = ANY($1)",
        )
        .bind(selected.iter().map(|i| i64::from(*i)).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let app_cfg = pl_files::db::app_config(&c.pool).await?;
    let remote_tiff = pl_files::db::remote_ocr_active(app_cfg.as_ref(), &c.cfg);
    {
        let _lock = MediaLock::acquire(&c.cfg.media_lock).await?;
        for r in &rows {
            let ft = pl_files::naming::default_file_extension(&r.mime_type, remote_tiff && r.mime_type == "image/tiff");
            cleanup_files(c, r, ft);
        }
    }
    tracing::info!(target: "paperless.tasks", "Deleted {} documents from trash", selected.len());
    Ok(selected.len())
}
