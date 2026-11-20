// File names after changes to an existing document (post_save receiver).
//
// upstream: src/documents/signals/handlers.py@v3.4.2 (update_filename_and_move_files, validate_move,
//           _path_matches_checksum, CannotMoveFilesException), src/documents/file_handling.py@v3.4.2
//           (generate_filename, generate_unique_filename, create_source_path_directory,
//           delete_empty_directories), src/documents/models.py@v3.4.2 (source_path, archive_path),
//           filelock (FileLock(settings.MEDIA_LOCK)).
//
// Same logic as pl-documents-write `files::update_filename_and_move_files` (there for the
// API write paths, here for the worker: scheduled workflows, bulk_update_documents, AI actions).
// pl-documents-write depends on pl-consumer, which is why the worker version lives here.

use std::path::{Path, PathBuf};

use sqlx::PgConnection;

use crate::consumer::{MAX_STORED_FILENAME_LENGTH, MediaLock, compute_checksum, move_file};
use crate::db::{self, DocRow, now_micros};
use crate::settings::ConsumerSettings;
use pl_templating::{DocumentContext, GenerateOptions, generate_filename, generate_unique_filename};

/// Path separator like `pathlib.Path` of the platform (`\` only on Windows); shared with pl-files and
/// pl-documents-write in [`pl_core::safepath`] (final security review C-N1).
pub use pl_core::safepath::PATH_SEPS;

/// Last component for unsafe names (contains NUL → every file operation fails).
pub const UNSAFE_MARKER: &str = "\u{0}unsafe-path";

/// `(root / rel)` confined to `root` ([`pl_core::safepath::safe_join`]).
pub use pl_core::safepath::safe_join;

/// [`safe_join`]; unsafe names → `root/<NUL>unsafe-path` (and a warning).
pub fn join(root: &Path, rel: &str) -> PathBuf {
    safe_join(root, rel).unwrap_or_else(|| {
        tracing::warn!(target: "paperless.handlers", "Refusing path outside of root {}: {rel:?}", root.display());
        root.join(UNSAFE_MARKER)
    })
}

/// `Document.source_path`.
pub fn source_path(s: &ConsumerSettings, d: &DocRow) -> PathBuf {
    match d.filename.as_deref().filter(|f| !f.is_empty()) {
        Some(f) => join(&s.originals_dir, f),
        None => s.originals_dir.join(format!("{:07}{}", d.id, pl_templating::default_file_extension(&d.mime_type))),
    }
}

/// `Document.archive_path` (`None` without an archive version).
pub fn archive_path(s: &ConsumerSettings, d: &DocRow) -> Option<PathBuf> {
    d.archive_filename.as_deref().filter(|f| !f.is_empty()).map(|a| join(&s.archive_dir, a))
}

fn outside_root(p: &Path, root: &Path) -> bool {
    p.as_os_str().to_string_lossy().contains('\0') || pl_templating::validate_path_in_root(p, root).is_err()
}

/// `_path_matches_checksum`.
fn checksum_matches(path: &Path, checksum: Option<&str>) -> bool {
    match checksum {
        Some(c) if path.is_file() => compute_checksum(path).is_ok_and(|h| h == c),
        _ => false,
    }
}

/// `CannotMoveFilesException` or IO/DB error in the try block.
#[derive(Debug)]
struct MoveFailed(String);

fn validate_move(old: &Path, new: &Path, root: &Path) -> Result<(), MoveFailed> {
    if outside_root(old, root) {
        let m = format!("Refusing to move file from outside root {}: {}.", root.display(), old.display());
        tracing::warn!(target: "paperless.handlers", "{m}");
        return Err(MoveFailed(m));
    }
    if outside_root(new, root) {
        let m = format!("Refusing to move file outside root {}: {}.", root.display(), new.display());
        tracing::warn!(target: "paperless.handlers", "{m}");
        return Err(MoveFailed(m));
    }
    if !old.is_file() {
        let m = format!("File {} doesn't exist.", old.display());
        tracing::error!(target: "paperless.handlers", "{m}");
        return Err(MoveFailed(m));
    }
    if new.is_file() {
        let m = format!("Cannot rename file since target path {} already exists.", new.display());
        tracing::warn!(target: "paperless.handlers", "{m}");
        return Err(MoveFailed(m));
    }
    Ok(())
}

use pl_core::safepath::same_rel;

/// `update_filename_and_move_files(sender, instance)` for a document and – for a root
/// document with a move – recursively its versions. Errors while moving are logged and rolled back
/// like upstream (no exception); DB errors on loading are passed back.
pub async fn update_filename_and_move_files(conn: &mut PgConnection, s: &ConsumerSettings, doc_id: i64) -> Result<(), sqlx::Error> {
    Box::pin(update_one(conn, s, doc_id)).await
}

async fn update_one(conn: &mut PgConnection, s: &ConsumerSettings, doc_id: i64) -> Result<(), sqlx::Error> {
    let Some(first) = db::load_document(conn, doc_id).await? else { return Ok(()) };
    if first.filename.as_deref().is_none_or(str::is_empty) {
        return Ok(());
    }
    {
        // FINDINGS B1: first lock the document rows (own transaction or savepoint in
        // the caller's), then MEDIA_LOCK – never wait for a row under MEDIA_LOCK.
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        // Root document before the version (order as when ingesting a version)
        crate::medialock::lock_document_rows(&mut tx, &[first.root_document_id.unwrap_or(doc_id)]).await?;
        crate::medialock::lock_document_rows(&mut tx, &[doc_id]).await?;
        let lock = MediaLock::acquire(&s.media_lock()).await.map_err(|e| sqlx::Error::Protocol(format!("media lock: {e}")))?;
        // instance.refresh_from_db()
        let Some(inst) = db::load_document(&mut tx, doc_id).await? else { return Ok(()) };
        let ctx = crate::templating::document_context(&mut tx, &inst).await?;
        let moved = move_under_lock(&mut tx, s, &inst, ctx).await?;
        finish_move_tx(tx).await;
        drop(lock);
        if !moved {
            return Ok(());
        }
    }
    // Keep version files in sync with root
    if first.root_document_id.is_none() {
        let versions: Vec<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY created DESC, id",
        )
        .bind(doc_id as i32)
        .fetch_all(&mut *conn)
        .await?;
        for v in versions {
            Box::pin(update_one(conn, s, i64::from(v))).await?;
        }
    }
    Ok(())
}

/// Bundled follow-up: `true` if `update_filename_and_move_files` would move nothing
/// and change no name for this state (template yields the same original and, if applicable,
/// archive name) – then only `UPDATE … SET modified = now()` remains, which the caller executes for many
/// documents in one statement. Pure calculation without the file system; in all other cases
/// (`false`) the caller takes the single path. `ctx` must belong to `inst` (without versions).
pub fn filename_unchanged(s: &ConsumerSettings, inst: &DocRow, ctx: &DocumentContext) -> bool {
    let Some(old) = inst.filename.as_deref().filter(|f| !f.is_empty()) else { return false };
    let opts = &s.templating;
    let Ok(candidate) = generate_filename(ctx, GenerateOptions::default(), opts) else { return false };
    if candidate.len() > MAX_STORED_FILENAME_LENGTH || !same_rel(&candidate, old) {
        return false;
    }
    let Some(old_af) = inst.archive_filename.as_deref() else { return true };
    let mut actx = ctx.clone();
    actx.filename = Some(old.to_string());
    match generate_filename(&actx, GenerateOptions { archive_filename: true, ..Default::default() }, opts) {
        Ok(ac) => ac.len() <= MAX_STORED_FILENAME_LENGTH && same_rel(&ac, old_af),
        Err(_) => false,
    }
}

/// `update_filename_and_move_files` with a state loaded beforehand (bundled): like
/// [`update_filename_and_move_files`], except that the template context is reused if the
/// document re-read under the lock is unchanged (otherwise it is reloaded as before),
/// and the version query is omitted if the caller knows there are no versions.
pub async fn update_filename_with_context(
    conn: &mut PgConnection,
    s: &ConsumerSettings,
    pre: &DocRow,
    pre_ctx: &DocumentContext,
    has_versions: bool,
) -> Result<(), sqlx::Error> {
    if pre.root_document_id.is_some() || has_versions {
        return update_filename_and_move_files(conn, s, pre.id).await;
    }
    if pre.filename.as_deref().is_none_or(str::is_empty) {
        return Ok(());
    }
    // FINDINGS B1: lock the row before MEDIA_LOCK (see `update_one`)
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    crate::medialock::lock_document_rows(&mut tx, &[pre.id]).await?;
    let lock = MediaLock::acquire(&s.media_lock()).await.map_err(|e| sqlx::Error::Protocol(format!("media lock: {e}")))?;
    // instance.refresh_from_db()
    let Some(inst) = db::load_document(&mut tx, pre.id).await? else { return Ok(()) };
    let ctx = if inst == *pre { pre_ctx.clone() } else { crate::templating::document_context(&mut tx, &inst).await? };
    move_under_lock(&mut tx, s, &inst, ctx).await?;
    finish_move_tx(tx).await;
    drop(lock);
    Ok(())
}

/// Complete the transaction or savepoint of `update_filename_and_move_files` (still under
/// MEDIA_LOCK). An error while moving/writing has already been logged and rolled back by
/// `move_under_lock` (upstream: no exception); if committing then fails, the
/// savepoint is rolled back (the surrounding transaction stays usable) and only logged.
async fn finish_move_tx(tx: sqlx::Transaction<'_, sqlx::Postgres>) {
    if let Err(e) = tx.commit().await {
        tracing::warn!(target: "paperless.handlers", "Exception during file handling: {e}");
    }
}

/// `Ok(false)` = early `return` (nothing moved), `Ok(true)` = end of the function reached.
async fn move_under_lock(conn: &mut PgConnection, s: &ConsumerSettings, inst: &DocRow, ctx: DocumentContext) -> Result<bool, sqlx::Error> {
    let opts = &s.templating;
    let old_filename = inst.filename.clone().unwrap_or_default();
    let old_source_path = source_path(s, inst);
    let old_archive_filename = inst.archive_filename.clone();
    let old_archive_path = archive_path(s, inst);
    let has_archive = inst.archive_filename.is_some();

    let mut move_original = false;
    let mut move_archive = false;
    let mut new_filename = old_filename.clone();
    let mut new_archive_filename = old_archive_filename.clone();

    let attempt: Result<(), MoveFailed> = (|| {
        let candidate = generate_filename(&ctx, GenerateOptions::default(), opts).map_err(|e| MoveFailed(format!("{e:?}")))?;
        if candidate.len() > MAX_STORED_FILENAME_LENGTH {
            let m = format!("Generated filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH})", candidate.len());
            tracing::warn!(target: "paperless.handlers", "{m}");
            return Err(MoveFailed(m));
        }
        let candidate_path = join(&s.originals_dir, &candidate);
        let mut original_already_moved = false;
        new_filename = if same_rel(&candidate, &old_filename) {
            old_filename.clone()
        } else if candidate_path.exists() && candidate_path != old_source_path {
            if !old_source_path.is_file() && checksum_matches(&candidate_path, Some(&inst.checksum)) {
                original_already_moved = true;
                candidate.clone()
            } else {
                generate_unique_filename(&ctx, false, opts, |rel| join(&s.originals_dir, rel).exists())
                    .map_err(|e| MoveFailed(format!("{e:?}")))?
            }
        } else {
            candidate.clone()
        };
        move_original = old_filename != new_filename && !original_already_moved;

        if has_archive {
            let mut actx = ctx.clone();
            actx.filename = Some(new_filename.clone());
            let archive_candidate = generate_filename(&actx, GenerateOptions { archive_filename: true, ..Default::default() }, opts)
                .map_err(|e| MoveFailed(format!("{e:?}")))?;
            if archive_candidate.len() > MAX_STORED_FILENAME_LENGTH {
                let m = format!(
                    "Generated archive filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH})",
                    archive_candidate.len()
                );
                tracing::warn!(target: "paperless.handlers", "{m}");
                return Err(MoveFailed(m));
            }
            let archive_candidate_path = join(&s.archive_dir, &archive_candidate);
            let old_af = old_archive_filename.clone().unwrap_or_default();
            let mut archive_already_moved = false;
            let naf = if same_rel(&archive_candidate, &old_af) {
                old_af.clone()
            } else if archive_candidate_path.exists() && Some(&archive_candidate_path) != old_archive_path.as_ref() {
                let old_exists = old_archive_path.as_ref().is_some_and(|p| p.is_file());
                if !old_exists && checksum_matches(&archive_candidate_path, inst.archive_checksum.as_deref()) {
                    archive_already_moved = true;
                    archive_candidate.clone()
                } else {
                    generate_unique_filename(&actx, true, opts, |rel| join(&s.archive_dir, rel).exists())
                        .map_err(|e| MoveFailed(format!("{e:?}")))?
                }
            } else {
                archive_candidate.clone()
            };
            move_archive = old_af != naf && !archive_already_moved;
            new_archive_filename = Some(naf);
        }
        Ok(())
    })();

    let result: Result<(), MoveFailed> = match attempt {
        Err(e) => Err(e),
        Ok(()) if !move_original && !move_archive => {
            // Document.objects.filter(pk=…).update(modified=now[, filename][, archive_filename])
            let fname = (old_filename != new_filename).then(|| new_filename.clone());
            let aname = (old_archive_filename != new_archive_filename).then(|| new_archive_filename.clone());
            crate::lookup::forget_docs();
            let r = sqlx::query(
                "UPDATE documents_document SET modified = $2, \
                 filename = CASE WHEN $3 THEN $4 ELSE filename END, \
                 archive_filename = CASE WHEN $5 THEN $6 ELSE archive_filename END \
                 WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(inst.id as i32)
            .bind(now_micros())
            .bind(fname.is_some())
            .bind(fname)
            .bind(aname.is_some())
            .bind(aname.flatten())
            .execute(&mut *conn)
            .await;
            match r {
                Ok(_) => return Ok(false),
                Err(e) => Err(MoveFailed(e.to_string())),
            }
        }
        Ok(()) => {
            let new_source_path = join(&s.originals_dir, &new_filename);
            let new_archive_path = new_archive_filename.as_ref().map(|a| join(&s.archive_dir, a));
            let r: Result<(), MoveFailed> = async {
                if move_original {
                    validate_move(&old_source_path, &new_source_path, &s.originals_dir)?;
                    pl_templating::create_source_path_directory(&new_source_path).map_err(|e| MoveFailed(e.to_string()))?;
                    move_file(&old_source_path, &new_source_path).map_err(|e| MoveFailed(e.to_string()))?;
                }
                if move_archive && let (Some(o), Some(n)) = (old_archive_path.as_ref(), new_archive_path.as_ref()) {
                    validate_move(o, n, &s.archive_dir)?;
                    pl_templating::create_source_path_directory(n).map_err(|e| MoveFailed(e.to_string()))?;
                    move_file(o, n).map_err(|e| MoveFailed(e.to_string()))?;
                }
                crate::lookup::forget_docs();
                sqlx::query("UPDATE documents_document SET filename = $2, archive_filename = $3, modified = $4 WHERE id = $1")
                    .bind(inst.id as i32)
                    .bind(&new_filename)
                    .bind(&new_archive_filename)
                    .bind(now_micros())
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| MoveFailed(e.to_string()))?;
                Ok(())
            }
            .await;
            if let Err(e) = &r {
                tracing::warn!(target: "paperless.handlers", "Exception during file handling: {}", e.0);
                if move_original && new_source_path.is_file() {
                    tracing::info!(target: "paperless.handlers", "Restoring previous original path");
                    let _ = move_file(&new_source_path, &old_source_path);
                }
                if move_archive
                    && let (Some(n), Some(o)) = (new_archive_path.as_ref(), old_archive_path.as_ref())
                    && n.is_file()
                {
                    tracing::info!(target: "paperless.handlers", "Restoring previous archive path");
                    let _ = move_file(n, o);
                }
            }
            r
        }
    };
    if let Err(e) = &result {
        tracing::warn!(target: "paperless.handlers", "Exception during file handling: {}", e.0);
    }
    // finally, remove any empty sub folders
    if !old_source_path.is_file()
        && let Some(parent) = old_source_path.parent()
    {
        pl_templating::delete_empty_directories(parent, &s.originals_dir);
    }
    if has_archive
        && let Some(oa) = old_archive_path.as_ref()
        && !oa.is_file()
        && let Some(parent) = oa.parent()
    {
        pl_templating::delete_empty_directories(parent, &s.archive_dir);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_stay_in_root() {
        let r = Path::new("/m");
        assert_eq!(safe_join(r, "a/b.pdf"), Some(PathBuf::from("/m/a/b.pdf")));
        // C-N1: `\` only a separator on Windows (like `pathlib`)
        if cfg!(windows) {
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(PathBuf::from("/m/b.pdf")));
            assert_eq!(safe_join(r, "ACME Corp\\2019\\a.pdf"), Some(PathBuf::from("/m/ACME Corp/2019/a.pdf")));
            assert_eq!(safe_join(r, "..\\x"), None);
            assert_eq!(safe_join(r, "a\\.. \\..\\x"), None);
        } else {
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(r.join("a\\..\\b.pdf")));
        }
        assert_eq!(safe_join(r, "../x"), None);
        assert_eq!(safe_join(r, "/etc/passwd"), None);
        assert!(join(r, "../x").ends_with(UNSAFE_MARKER));
    }

    #[test]
    fn same_rel_normalises() {
        assert_eq!(same_rel("a\\b.pdf", "a/./b.pdf"), cfg!(windows));
        assert!(same_rel("a//b.pdf", "a/./b.pdf"));
        assert!(!same_rel("a/b.pdf", "a/c.pdf"));
    }
}
