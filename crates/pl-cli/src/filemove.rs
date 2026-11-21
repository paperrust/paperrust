//! `update_filename_and_move_files` (post_save of Document) and `document_renamer`.
//!
//! upstream: src/documents/signals/handlers.py (`update_filename_and_move_files`,
//! `_path_matches_checksum`, carrying versions along), src/documents/file_handling.py
//! (`generate_filename`, `generate_unique_filename`, `create_source_path_directory`,
//! `delete_empty_directories`), src/documents/management/commands/document_renamer.py (v3.4.2).
//! Not replicated: `clear_document_caches` (Django cache in Redis; see Known differences).

use std::path::{Path, PathBuf};

use sqlx::PgConnection;

use crate::CliResult;
use crate::args;
use crate::ctx::Ctx;
use crate::export::medialock::MediaLock;

const MAX_STORED_FILENAME_LENGTH: usize = 1024;

fn sha256(p: &Path) -> Option<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(crate::export::fernet::hex_encode(&h.finalize()))
}

fn matches_checksum(p: &Path, checksum: Option<&str>) -> bool {
    match checksum {
        Some(c) if p.is_file() => sha256(p).as_deref() == Some(c),
        _ => false,
    }
}

/// `shutil.move` (rename, otherwise copy + delete).
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            crate::export::copy_file_with_basic_stats(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// Join the stored or generated name with the root – only via `safe_join`
/// (security review 2026-11-19 F9). Unsafe name → path that never exists (`<NUL>unsafe-path`,
/// warning in the log): the file counts as missing, `validate_move` aborts, nothing is moved.
fn mjoin(root: &Path, rel: &str) -> PathBuf {
    pl_consumer::files::join(root, rel)
}

fn norm(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Result for statistics/tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveResult {
    pub moved_original: bool,
    pub moved_archive: bool,
}

/// `update_filename_and_move_files(sender=Document, instance=doc)`. Errors while moving are
/// logged and rolled back like upstream (no error for the caller).
pub async fn update_filename_and_move_files(c: &Ctx, conn: &mut PgConnection, doc_id: i64) -> CliResult<MoveResult> {
    let mut res = MoveResult::default();
    let Some(first) = pl_consumer::db::load_document(conn, doc_id).await? else { return Ok(res) };
    if first.filename.as_deref().is_none_or(str::is_empty) {
        return Ok(res);
    }
    {
        // Lock order as in the server (BEFUNDE B1, pl_consumer::medialock): first the document row
        // is locked in a (sub)transaction, then MEDIA_LOCK – under MEDIA_LOCK only the
        // own, already locked row is written (never wait for a foreign row lock).
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        pl_consumer::medialock::lock_document_rows(&mut tx, &[doc_id]).await?;
        let conn: &mut PgConnection = &mut tx;
        let _lock = MediaLock::acquire(&c.cfg.media_lock).await?;
        // refresh_from_db
        let Some(doc) = pl_consumer::db::load_document(conn, doc_id).await? else { return Ok(res) };
        let opts = c.templating_options();
        let originals = &c.cfg.originals_dir;
        let archive_dir = &c.cfg.archive_dir;
        let old_filename = doc.filename.clone().unwrap_or_default();
        let old_source = norm(&mjoin(originals, &old_filename));
        let old_archive_filename = doc.archive_filename.clone();
        let old_archive = old_archive_filename.as_ref().map(|f| norm(&mjoin(archive_dir, f)));
        let ctx = pl_consumer::templating::document_context(conn, &doc).await?;
        let repr = pl_consumer::docops::DocInstance::from_db(doc.clone()).repr(conn).await?;

        let outcome: Result<(String, Option<String>, bool, bool), String> = (|| {
            let candidate = pl_templating::generate_filename(&ctx, Default::default(), &opts).map_err(|e| e.to_string())?;
            if candidate.len() > MAX_STORED_FILENAME_LENGTH {
                let m = format!(
                    "Document {repr}: Generated filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH}): {candidate}",
                    candidate.len()
                );
                tracing::warn!(target: "paperless.handlers", "{m}");
                return Err(m);
            }
            let cand_path = norm(&mjoin(originals, &candidate));
            let mut original_already_moved = false;
            let new_filename = if candidate == old_filename {
                old_filename.clone()
            } else if cand_path.exists() && cand_path != old_source {
                if !old_source.is_file() && matches_checksum(&cand_path, Some(&doc.checksum)) {
                    original_already_moved = true;
                    candidate.clone()
                } else {
                    pl_templating::generate_unique_filename(&ctx, false, &opts, |rel| mjoin(originals, rel).exists())
                        .map_err(|e| e.to_string())?
                }
            } else {
                candidate.clone()
            };
            let move_original = old_filename != new_filename && !original_already_moved;
            let mut new_archive = old_archive_filename.clone();
            let mut move_archive = false;
            if let (Some(old_af), Some(old_ap)) = (&old_archive_filename, &old_archive) {
                let gopt = pl_templating::GenerateOptions { archive_filename: true, ..Default::default() };
                let acand = pl_templating::generate_filename(&ctx, gopt, &opts).map_err(|e| e.to_string())?;
                if acand.len() > MAX_STORED_FILENAME_LENGTH {
                    let m = format!(
                        "Document {repr}: Generated archive filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH}): {acand}",
                        acand.len()
                    );
                    tracing::warn!(target: "paperless.handlers", "{m}");
                    return Err(m);
                }
                let acand_path = norm(&mjoin(archive_dir, &acand));
                let mut archive_already_moved = false;
                let naf = if &acand == old_af {
                    old_af.clone()
                } else if acand_path.exists() && &acand_path != old_ap {
                    if !old_ap.is_file() && matches_checksum(&acand_path, doc.archive_checksum.as_deref()) {
                        archive_already_moved = true;
                        acand.clone()
                    } else {
                        pl_templating::generate_unique_filename(&ctx, true, &opts, |rel| mjoin(archive_dir, rel).exists())
                            .map_err(|e| e.to_string())?
                    }
                } else {
                    acand.clone()
                };
                move_archive = old_af != &naf && !archive_already_moved;
                new_archive = Some(naf);
            }
            Ok((new_filename, new_archive, move_original, move_archive))
        })();

        match outcome {
            Err(e) => {
                tracing::warn!(target: "paperless.handlers", "Exception during file handling: {e}");
            }
            Ok((new_filename, new_archive, move_original, move_archive)) => {
                if !move_original && !move_archive {
                    // upstream sets filename/archive_filename only on change – same result
                    sqlx::query("UPDATE documents_document SET modified = now(), filename = $2, archive_filename = $3 WHERE id = $1 AND deleted_at IS NULL")
                        .bind(doc_id as i32)
                        .bind(&new_filename)
                        .bind(&new_archive)
                        .execute(&mut *conn)
                        .await?;
                } else {
                    let new_source = norm(&mjoin(originals, &new_filename));
                    let new_archive_path = new_archive.as_ref().map(|f| norm(&mjoin(archive_dir, f)));
                    let r: Result<(), String> = async {
                        if move_original {
                            validate_move(&repr, &old_source, &new_source, originals)?;
                            let _ = pl_templating::create_source_path_directory(&new_source);
                            move_file(&old_source, &new_source).map_err(|e| e.to_string())?;
                        }
                        if move_archive && let (Some(oa), Some(na)) = (&old_archive, &new_archive_path) {
                            validate_move(&repr, oa, na, archive_dir)?;
                            let _ = pl_templating::create_source_path_directory(na);
                            move_file(oa, na).map_err(|e| e.to_string())?;
                        }
                        sqlx::query("UPDATE documents_document SET filename = $2, archive_filename = $3, modified = now() WHERE id = $1")
                            .bind(doc_id as i32)
                            .bind(&new_filename)
                            .bind(&new_archive)
                            .execute(&mut *conn)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(())
                    }
                    .await;
                    match r {
                        Ok(()) => {
                            res.moved_original = move_original;
                            res.moved_archive = move_archive;
                        }
                        Err(e) => {
                            tracing::warn!(target: "paperless.handlers", "Exception during file handling: {e}");
                            if move_original && new_source.is_file() {
                                tracing::info!(target: "paperless.handlers", "Restoring previous original path");
                                let _ = move_file(&new_source, &old_source);
                            }
                            if move_archive
                                && let (Some(oa), Some(na)) = (&old_archive, &new_archive_path)
                                && na.is_file()
                            {
                                tracing::info!(target: "paperless.handlers", "Restoring previous archive path");
                                let _ = move_file(na, oa);
                            }
                        }
                    }
                }
            }
        }
        if !old_source.is_file()
            && let Some(p) = old_source.parent()
        {
            pl_templating::delete_empty_directories(p, originals);
        }
        if let Some(oa) = &old_archive
            && !oa.is_file()
            && let Some(p) = oa.parent()
        {
            pl_templating::delete_empty_directories(p, archive_dir);
        }
        tx.commit().await?;
    }
    // keep versions in sync with the root
    if first.root_document_id.is_none() {
        let versions: Vec<i32> =
            sqlx::query_scalar("SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL ORDER BY id")
                .bind(doc_id as i32)
                .fetch_all(&mut *conn)
                .await?;
        for v in versions {
            Box::pin(update_filename_and_move_files(c, conn, i64::from(v))).await?;
        }
    }
    Ok(res)
}

fn validate_move(repr: &str, old: &Path, new: &Path, root: &Path) -> Result<(), String> {
    if !new.starts_with(norm(root)) {
        let m = format!("Document {repr}: Refusing to move file outside root {}: {}.", root.display(), new.display());
        tracing::warn!(target: "paperless.handlers", "{m}");
        return Err(m);
    }
    if !old.is_file() {
        let m = format!("Document {repr}: File {} doesn't exist.", old.display());
        tracing::error!(target: "paperless.handlers", "{m}");
        return Err(m);
    }
    if new.is_file() {
        let m = format!("Document {repr}: Cannot rename file since target path {} already exists.", new.display());
        tracing::warn!(target: "paperless.handlers", "{m}");
        return Err(m);
    }
    Ok(())
}

/// `document_renamer`: `post_save` for every document (`Document.objects.all()`, order `-created`).
pub async fn renamer_command(argv: Vec<String>) -> CliResult {
    args::parse(&argv, &[args::PROGRESS], 0)?;
    let c = Ctx::connect().await?;
    rename_all(&c).await.map(|_| ())
}

pub async fn rename_all(c: &Ctx) -> CliResult<usize> {
    let ids: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM documents_document WHERE deleted_at IS NULL ORDER BY created DESC").fetch_all(&c.pool).await?;
    let mut conn = c.pool.acquire().await?;
    let mut moved = 0;
    for id in ids {
        let r = update_filename_and_move_files(c, &mut conn, i64::from(id)).await?;
        if r.moved_original || r.moved_archive {
            moved += 1;
        }
    }
    Ok(moved)
}
