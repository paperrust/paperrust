// upstream: src/documents/signals/handlers.py@v3.4.2 (update_filename_and_move_files, validate_move,
//           _path_matches_checksum, _filename_template_uses_custom_fields, cleanup_document_deletion,
//           CannotMoveFilesException), src/documents/file_handling.py@v3.4.2 (generate_filename,
//           generate_unique_filename, create_source_path_directory, delete_empty_directories),
//           src/documents/models.py@v3.4.2 (source_path, archive_path, thumbnail_path, has_archive_version),
//           filelock 4.2.6 (FileLock(settings.MEDIA_LOCK))
//
//! File names after changes: rename/move under `MEDIA_LOCK`, collisions, rollback,
//! clean up empty folders; deletion when emptying the trash.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use pl_consumer::db::DocRow;
use pl_templating::{GenerateOptions, Options, generate_filename, generate_unique_filename};

use crate::docs::{self, now_micros};
use crate::error::ApiError;

/// `Document.MAX_STORED_FILENAME_LENGTH`.
pub const MAX_STORED_FILENAME_LENGTH: usize = 1024;

/// Paths and file name settings (django.conf.settings).
#[derive(Clone, Debug)]
pub struct MediaCfg {
    pub originals_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub thumbnail_dir: PathBuf,
    pub media_lock: PathBuf,
    pub empty_trash_dir: Option<PathBuf>,
    pub opts: Options,
}

impl MediaCfg {
    pub fn from_config(c: &pl_core::Config) -> Self {
        let mut opts = Options::from_env().unwrap_or_default();
        opts.time_zone = c.tz;
        MediaCfg {
            originals_dir: c.originals_dir.clone(),
            archive_dir: c.archive_dir.clone(),
            thumbnail_dir: c.thumbnail_dir.clone(),
            media_lock: c.media_lock.clone(),
            empty_trash_dir: c.empty_trash_dir.clone(),
            opts,
        }
    }

    /// `Document.source_path`. A file name that leads out of `ORIGINALS_DIR` (`..` above the
    /// root, absolute, drive, NUL) yields an invalid path ([`UNSAFE_MARKER`]) on which
    /// every file operation fails; check explicitly with [`MediaCfg::checked_source_path`].
    pub fn source_path(&self, d: &DocRow) -> PathBuf {
        join(&self.originals_dir, &self.source_name(d))
    }

    fn source_name(&self, d: &DocRow) -> String {
        match d.filename.as_deref().filter(|f| !f.is_empty()) {
            Some(f) => f.to_string(),
            None => format!("{:07}{}", d.id, pl_templating::default_file_extension(&d.mime_type)),
        }
    }

    /// `source_path` only within `ORIGINALS_DIR` (review H1).
    pub fn checked_source_path(&self, d: &DocRow) -> Result<PathBuf, String> {
        checked(&self.originals_dir, &self.source_name(d))
    }

    /// `Document.archive_path` (unsafe → invalid path as with [`MediaCfg::source_path`]).
    pub fn archive_path(&self, d: &DocRow) -> Option<PathBuf> {
        d.archive_filename.as_ref().map(|a| join(&self.archive_dir, a))
    }

    /// `archive_path` only within `ARCHIVE_DIR` (review H1).
    pub fn checked_archive_path(&self, d: &DocRow) -> Result<Option<PathBuf>, String> {
        d.archive_filename.as_ref().map(|a| checked(&self.archive_dir, a)).transpose()
    }

    /// `Document.thumbnail_path`.
    pub fn thumbnail_path(&self, d: &DocRow) -> PathBuf {
        self.thumbnail_dir.join(format!("{:07}.webp", d.id))
    }
}

/// Path separator like `pathlib.Path` of the platform (`\` only on Windows); shared with pl-files and
/// pl-consumer in [`pl_core::safepath`] (final security review C-N1).
pub use pl_core::safepath::PATH_SEPS;

/// Last component of the result of [`join`] for unsafe names (contains NUL → never exists,
/// every file operation fails with `InvalidInput`).
pub const UNSAFE_MARKER: &str = "\u{0}unsafe-path";

/// `(root / rel)` confined to `root` ([`pl_core::safepath::safe_join`], review H1/C-N1).
pub use pl_core::safepath::safe_join;

/// [`safe_join`]; unsafe names → `root/<NUL>unsafe-path` and a warning.
pub fn join(root: &Path, rel: &str) -> PathBuf {
    match safe_join(root, rel) {
        Some(p) => p,
        None => {
            tracing::warn!(target: "paperless.handlers", "Refusing path outside of root {}: {rel:?}", root.display());
            root.join(UNSAFE_MARKER)
        }
    }
}

fn checked(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = safe_join(root, rel).ok_or_else(|| format!("Refusing to access file outside of root {}: {rel}.", root.display()))?;
    pl_templating::validate_path_in_root(&p, root).map_err(|e| e.0)?;
    Ok(p)
}

/// `p` is a [`join`] result for an unsafe name or lies (resolved, incl. symlinks)
/// outside `root` (`validate_path_in_root`).
pub fn outside_root(p: &Path, root: &Path) -> bool {
    p.as_os_str().to_string_lossy().contains('\0') || pl_templating::validate_path_in_root(p, root).is_err()
}

/// `FileLock(settings.MEDIA_LOCK)`: shared implementation in pl-consumer (fixed lock order
/// "rows first, then MEDIA_LOCK", in-process queue, time limit; FINDINGS B1).
pub use pl_consumer::medialock::MediaLock;

fn checksum_matches(path: &Path, checksum: Option<&str>) -> bool {
    let Some(c) = checksum else { return false };
    if !path.is_file() {
        return false;
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let h = Sha256::digest(&bytes);
            let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
            hex == c
        }
        Err(_) => false,
    }
}

/// `shutil.move`: rename, on failure (other file system) copy + delete.
fn shutil_move(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// `CannotMoveFilesException` or IO/DB error in the try block.
#[derive(Debug)]
struct MoveFailed(String);

fn validate_move(old: &Path, new: &Path, root: &Path) -> Result<(), MoveFailed> {
    // Review H1: the source must also lie within the root.
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

/// `_filename_template_uses_custom_fields(doc)`.
pub async fn template_uses_custom_fields(conn: &mut PgConnection, cfg: &MediaCfg, doc: &DocRow) -> Result<bool, sqlx::Error> {
    let template: Option<String> = match doc.storage_path_id {
        Some(sp) => sqlx::query_scalar("SELECT path FROM documents_storagepath WHERE id = $1")
            .bind(sp as i32)
            .fetch_optional(&mut *conn)
            .await?,
        None => cfg.opts.filename_format.as_deref().map(pl_templating::convert_format_str_to_template_format),
    };
    Ok(template.is_some_and(|t| !t.is_empty() && t.contains("custom_fields")))
}

/// `post_save(CustomFieldInstance)` → `update_filename_and_move_files` only if the template
/// of the document uses `custom_fields`.
pub async fn after_custom_field_saved(conn: &mut PgConnection, cfg: &MediaCfg, doc_id: i64) -> Result<(), ApiError> {
    let Some(d) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    if template_uses_custom_fields(conn, cfg, &d).await? {
        update_filename_and_move_files(conn, cfg, doc_id).await?;
    }
    Ok(())
}

/// `update_filename_and_move_files(sender, instance)` for a document (and recursively its versions).
pub async fn update_filename_and_move_files(conn: &mut PgConnection, cfg: &MediaCfg, doc_id: i64) -> Result<(), ApiError> {
    Box::pin(update_one(conn, cfg, doc_id)).await
}

async fn update_one(conn: &mut PgConnection, cfg: &MediaCfg, doc_id: i64) -> Result<(), ApiError> {
    let Some(first) = docs::load_global(conn, doc_id).await? else { return Ok(()) };
    if first.filename.as_deref().is_none_or(str::is_empty) {
        return Ok(());
    }
    {
        // FINDINGS B1: fixed lock order – first the document rows (own transaction or
        // savepoint in the caller's, e.g. batch deletion), then MEDIA_LOCK. Under
        // MEDIA_LOCK only the already locked row is written.
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        // Root document before the version (order as when ingesting a version)
        pl_consumer::medialock::lock_document_rows(&mut tx, &[first.root_document_id.unwrap_or(doc_id)]).await?;
        pl_consumer::medialock::lock_document_rows(&mut tx, &[doc_id]).await?;
        let lock = MediaLock::acquire(&cfg.media_lock).await.map_err(|e| ApiError::Uncaught(format!("media lock: {e}")))?;
        // instance.refresh_from_db()
        let Some(inst) = docs::load_global(&mut tx, doc_id).await? else { return Ok(()) };
        // Without a move upstream returns with `return` in the try block → no versions.
        let moved = move_under_lock(&mut tx, cfg, &inst).await;
        let moved = match moved {
            Ok(m) => {
                // A failure while moving was logged and rolled back by move_under_lock; if
                // committing fails, sqlx rolls back the savepoint (caller transaction remains
                // usable) – only log, as upstream.
                if let Err(e) = tx.commit().await {
                    tracing::warn!(target: "paperless.handlers", "Exception during file handling: {e}");
                }
                m
            }
            Err(e) => return Err(e),
        };
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
            Box::pin(update_one(conn, cfg, i64::from(v))).await?;
        }
    }
    Ok(())
}

/// `Ok(false)` = early `return` (nothing to move), `Ok(true)` = end of the function reached.
async fn move_under_lock(conn: &mut PgConnection, cfg: &MediaCfg, inst: &DocRow) -> Result<bool, ApiError> {
    let ctx = pl_consumer::templating::document_context(conn, inst).await?;
    let old_filename = inst.filename.clone().unwrap_or_default();
    let old_source_path = cfg.source_path(inst);
    let old_archive_filename = inst.archive_filename.clone();
    let old_archive_path = cfg.archive_path(inst);
    let has_archive = inst.archive_filename.is_some();

    let mut move_original = false;
    let mut move_archive = false;
    let mut new_filename = old_filename.clone();
    let mut new_archive_filename = old_archive_filename.clone();

    let attempt: Result<(), MoveFailed> = async {
        let candidate = generate_filename(&ctx, GenerateOptions::default(), &cfg.opts)
            .map_err(|e| MoveFailed(format!("template: {e:?}")))?;
        if candidate.len() > MAX_STORED_FILENAME_LENGTH {
            let m = format!("Generated filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH})", candidate.len());
            tracing::warn!(target: "paperless.handlers", "{m}");
            return Err(MoveFailed(m));
        }
        let candidate_path = join(&cfg.originals_dir, &candidate);
        let mut original_already_moved = false;
        new_filename = if same_rel(&candidate, &old_filename) {
            old_filename.clone()
        } else if candidate_path.exists() && candidate_path != old_source_path {
            if !old_source_path.is_file() && checksum_matches(&candidate_path, Some(&inst.checksum)) {
                original_already_moved = true;
                candidate.clone()
            } else {
                generate_unique_filename(&ctx, false, &cfg.opts, |rel| join(&cfg.originals_dir, rel).exists())
                    .map_err(|e| MoveFailed(format!("template: {e:?}")))?
            }
        } else {
            candidate.clone()
        };
        move_original = old_filename != new_filename && !original_already_moved;

        if has_archive {
            let mut actx = ctx.clone();
            actx.filename = Some(new_filename.clone());
            let archive_candidate =
                generate_filename(&actx, GenerateOptions { archive_filename: true, ..Default::default() }, &cfg.opts)
                    .map_err(|e| MoveFailed(format!("template: {e:?}")))?;
            if archive_candidate.len() > MAX_STORED_FILENAME_LENGTH {
                let m = format!(
                    "Generated archive filename exceeds db path limit ({} > {MAX_STORED_FILENAME_LENGTH})",
                    archive_candidate.len()
                );
                tracing::warn!(target: "paperless.handlers", "{m}");
                return Err(MoveFailed(m));
            }
            let archive_candidate_path = join(&cfg.archive_dir, &archive_candidate);
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
                    generate_unique_filename(&actx, true, &cfg.opts, |rel| join(&cfg.archive_dir, rel).exists())
                        .map_err(|e| MoveFailed(format!("template: {e:?}")))?
                }
            } else {
                archive_candidate.clone()
            };
            move_archive = old_af != naf && !archive_already_moved;
            new_archive_filename = Some(naf);
        }
        Ok(())
    }
    .await;

    let result: Result<(), MoveFailed> = match attempt {
        Err(e) => Err(e),
        Ok(()) if !move_original && !move_archive => {
            // Document.objects.filter(pk=…).update(modified=now[, filename][, archive_filename])
            let fname = (old_filename != new_filename).then(|| new_filename.clone());
            let aname = (old_archive_filename != new_archive_filename).then(|| new_archive_filename.clone());
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
            let new_source_path = join(&cfg.originals_dir, &new_filename);
            let new_archive_path = new_archive_filename.as_ref().map(|a| join(&cfg.archive_dir, a));
            let r: Result<(), MoveFailed> = async {
                if move_original {
                    validate_move(&old_source_path, &new_source_path, &cfg.originals_dir)?;
                    pl_templating::create_source_path_directory(&new_source_path).map_err(|e| MoveFailed(e.to_string()))?;
                    shutil_move(&old_source_path, &new_source_path).map_err(|e| MoveFailed(e.to_string()))?;
                }
                if move_archive
                    && let (Some(o), Some(n)) = (old_archive_path.as_ref(), new_archive_path.as_ref())
                {
                    validate_move(o, n, &cfg.archive_dir)?;
                    pl_templating::create_source_path_directory(n).map_err(|e| MoveFailed(e.to_string()))?;
                    shutil_move(o, n).map_err(|e| MoveFailed(e.to_string()))?;
                }
                sqlx::query(
                    "UPDATE documents_document SET filename = $2, archive_filename = $3, modified = $4 WHERE id = $1",
                )
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
                    let _ = shutil_move(&new_source_path, &old_source_path);
                }
                if move_archive
                    && let (Some(n), Some(o)) = (new_archive_path.as_ref(), old_archive_path.as_ref())
                    && n.is_file()
                {
                    tracing::info!(target: "paperless.handlers", "Restoring previous archive path");
                    let _ = shutil_move(n, o);
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
        pl_templating::delete_empty_directories(parent, &cfg.originals_dir);
    }
    if has_archive
        && let Some(oa) = old_archive_path.as_ref()
        && !oa.is_file()
        && let Some(parent) = oa.parent()
    {
        pl_templating::delete_empty_directories(parent, &cfg.archive_dir);
    }
    Ok(true)
}

use pl_core::safepath::same_rel;

/// `cleanup_document_deletion` (post_delete when emptying the trash).
pub async fn cleanup_document_deletion(cfg: &MediaCfg, d: &DocRow) {
    let Ok(_lock) = MediaLock::acquire(&cfg.media_lock).await else { return };
    // Review H1: do not move or delete anything outside ORIGINALS_DIR/ARCHIVE_DIR.
    let (source, archive) = match (cfg.checked_source_path(d), cfg.checked_archive_path(d)) {
        (Ok(s), Ok(a)) => (s, a),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!(target: "paperless.handlers", "Skipping cleanup of document {}: {e}", d.id);
            return;
        }
    };
    if let Some(trash) = &cfg.empty_trash_dir {
        let name = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let p = Path::new(&name);
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = p.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
        let mut counter = 0u32;
        let target = loop {
            let cand = trash.join(format!("{stem}{}{ext}", if counter > 0 { format!("_{counter:02}") } else { String::new() }));
            if cand.exists() {
                counter += 1;
            } else {
                break cand;
            }
        };
        if let Err(e) = shutil_move(&source, &target) {
            tracing::error!(
                target: "paperless.handlers",
                "Failed to move {} to trash at {}: {e}. Skipping cleanup!",
                source.display(),
                target.display()
            );
            return;
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(a) = archive.clone() {
        files.push(a);
    }
    files.push(cfg.thumbnail_path(d));
    if cfg.empty_trash_dir.is_none() {
        files.push(source.clone());
    }
    for f in files {
        if f.is_file() {
            if let Err(e) = std::fs::remove_file(&f) {
                tracing::warn!(target: "paperless.handlers", "the file {} could not be deleted: {e}", f.display());
            }
        } else {
            tracing::warn!(target: "paperless.handlers", "Expected {} to exist, but it did not", f.display());
        }
    }
    if let Some(parent) = source.parent() {
        pl_templating::delete_empty_directories(parent, &cfg.originals_dir);
    }
    if let Some(a) = archive
        && let Some(parent) = a.parent()
    {
        pl_templating::delete_empty_directories(parent, &cfg.archive_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_normalizes() {
        let r = Path::new("/m/originals");
        assert_eq!(join(r, "a//b/./c.pdf"), PathBuf::from("/m/originals/a/b/c.pdf"));
        assert_eq!(join(r, "a/../b.pdf"), PathBuf::from("/m/originals/b.pdf"));
        assert!(same_rel("a/b.pdf", "a//b.pdf"));
    }

    /// Review H1: no stored name leads out of the root.
    #[test]
    fn join_never_escapes_root() {
        let r = Path::new("/m/originals");
        let mut bad_names = vec!["../x", "a/../../x", "/etc/passwd", "x\0y"];
        if cfg!(windows) {
            bad_names.extend(["..\\archive\\0000042.pdf", "\\\\srv\\x", "C:\\x", "a/C:x", "a\\.. \\..\\x", "a/.../b"]);
        }
        for bad in bad_names {
            assert_eq!(safe_join(r, bad), None, "{bad:?}");
            let j = join(r, bad);
            assert!(j.to_string_lossy().contains('\0'), "{bad:?}");
            assert!(outside_root(&j, r), "{bad:?}");
        }
        // C-N1: `\` is a separator only on Windows (like `pathlib`)
        if cfg!(windows) {
            assert_eq!(safe_join(r, "ACME Corp\\2019\\a.pdf"), Some(PathBuf::from("/m/originals/ACME Corp/2019/a.pdf")));
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(PathBuf::from("/m/originals/b.pdf")));
        } else {
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(r.join("a\\..\\b.pdf")));
            assert_eq!(safe_join(r, "..\\x"), Some(r.join("..\\x")));
        }
    }

    fn doc(filename: &str, archive: Option<&str>) -> DocRow {
        DocRow {
            id: 42,
            title: String::new(),
            content: String::new(),
            mime_type: "application/pdf".into(),
            checksum: String::new(),
            archive_checksum: None,
            page_count: None,
            created: chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap_or_default(),
            modified: chrono::Utc::now(),
            added: chrono::Utc::now(),
            filename: Some(filename.into()),
            archive_filename: archive.map(str::to_string),
            original_filename: None,
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
        }
    }

    fn media(root: &Path) -> MediaCfg {
        MediaCfg {
            originals_dir: root.join("originals"),
            archive_dir: root.join("archive"),
            thumbnail_dir: root.join("thumbnails"),
            media_lock: root.join("media.lock"),
            empty_trash_dir: None,
            opts: Options::default(),
        }
    }

    fn victim(m: &MediaCfg, name: &str) -> PathBuf {
        std::fs::create_dir_all(&m.archive_dir).unwrap();
        std::fs::create_dir_all(&m.originals_dir).unwrap();
        let v = m.archive_dir.join(name);
        std::fs::write(&v, b"x").unwrap();
        v
    }

    /// Review H1, path 1 (move via PATCH): a source outside the root is not moved.
    #[test]
    fn move_refuses_source_outside_root() {
        let t = tempfile::tempdir().unwrap();
        let m = media(t.path());
        let v = victim(&m, "0000042.pdf");
        let old = m.source_path(&doc("..\\archive\\0000042.pdf", None));
        assert!(validate_move(&old, &m.originals_dir.join("new.pdf"), &m.originals_dir).is_err());
        // a lexically clean path outside (e.g. from a template) is also rejected
        assert!(validate_move(&v, &m.originals_dir.join("new.pdf"), &m.originals_dir).is_err());
        let inside = m.originals_dir.join("a.pdf");
        std::fs::write(&inside, b"y").unwrap();
        assert!(validate_move(&inside, &m.originals_dir.join("../archive/b.pdf"), &m.originals_dir).is_err());
        assert!(validate_move(&inside, &m.originals_dir.join("sub/b.pdf"), &m.originals_dir).is_ok());
        assert!(v.is_file());
    }

    /// Review H1, path 2 (reading via bulk_edit rotate/split/edit_pdf/merge): checked paths.
    #[test]
    fn checked_paths_reject_escape() {
        let t = tempfile::tempdir().unwrap();
        let m = media(t.path());
        // Backslash is a path separator only on Windows; on Linux "..\archive\x" is one harmless file
        // name inside the root (found by the arm64 Linux test run 2026-11-20).
        #[cfg(windows)]
        assert!(m.checked_source_path(&doc("..\\archive\\0000042.pdf", None)).is_err());
        assert!(m.checked_source_path(&doc("../archive/0000042.pdf", None)).is_err());
        assert!(m.checked_archive_path(&doc("a.pdf", Some("../originals/x.pdf"))).is_err());
        assert!(m.checked_source_path(&doc("sub/a.pdf", Some("a.pdf"))).is_ok());
        assert!(m.checked_archive_path(&doc("a.pdf", Some("a.pdf"))).unwrap().is_some());
    }

    /// Review H1, path 3 (deleting when emptying the trash): foreign files remain.
    #[tokio::test]
    async fn cleanup_does_not_delete_outside_root() {
        let t = tempfile::tempdir().unwrap();
        let m = media(t.path());
        let v = victim(&m, "0000043.pdf");
        cleanup_document_deletion(&m, &doc("..\\archive\\0000043.pdf", None)).await;
        assert!(v.is_file());
        cleanup_document_deletion(&m, &doc("ok.pdf", Some("../../archive/0000043.pdf"))).await;
        assert!(v.is_file());
    }
}
