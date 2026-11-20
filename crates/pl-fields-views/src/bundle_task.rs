//! Worker task `documents.tasks.build_share_link_bundle(bundle_id)` (W14) – runs in the
//! Rust worker (registered by pl-server).
//!
//! upstream: src/documents/tasks.py@v3.4.2 (`build_share_link_bundle`),
//! src/documents/bulk_download.py@v3.4.2 (`OriginalsOnlyStrategy`, `ArchiveOnlyStrategy`,
//! `_filename_only`), src/documents/models.py@v3.4.2 (`ShareLinkBundle.remove_file`).
//!
//! Flow as upstream: remove old file, status `processing` + clear fields, write documents
//! (`bundle.documents.all()` = without deleted, ordered by pk) into a ZIP_DEFLATED file in
//! `SCRATCH_DIR` (`tmp….zip`), move to `SHARE_LINK_BUNDLE_DIR/<slug>.zip`,
//! status `ready`. Error → status `failed`, `last_error` = `{bundle_id, exception_type,
//! message, timestamp}`, task fails (`raise`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use pl_core::AppState;
use pl_files::db::{self, DocRow};
use pl_files::docfile::{self, Ctx};
use pl_files::resolve;
use pl_files::zipw::{self, Method, ZipWriter};
use serde_json::{Map, Value, json};

pub const TASK_NAME: &str = crate::celery::BUILD_SHARE_LINK_BUNDLE;

/// Error like a Python exception (`exc.__class__.__name__`, `str(exc)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFailure {
    pub exception_type: String,
    pub message: String,
}

impl TaskFailure {
    fn new(t: &str, m: impl Into<String>) -> Self {
        TaskFailure { exception_type: t.into(), message: m.into() }
    }
}

impl From<sqlx::Error> for TaskFailure {
    fn from(e: sqlx::Error) -> Self {
        pl_core::dbconn::note_error(&e);
        TaskFailure::new("OperationalError", e.to_string())
    }
}

/// Argument `bundle_id` (kwarg or `args[0]`).
pub fn bundle_id_arg(args: &[Value], kwargs: &Map<String, Value>) -> Result<i64, TaskFailure> {
    let v = kwargs.get("bundle_id").or_else(|| args.first());
    match v {
        None => Err(TaskFailure::new(
            "TypeError",
            "build_share_link_bundle() missing 1 required positional argument: 'bundle_id'",
        )),
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| TaskFailure::new("ValueError", format!("Field 'id' expected a number but got {n}."))),
        Some(Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| TaskFailure::new("ValueError", format!("Field 'id' expected a number but got '{s}'."))),
        Some(other) => Err(TaskFailure::new("TypeError", format!("Field 'id' expected a number but got {other}."))),
    }
}

/// Python text of an `OSError` from `zipf.write`/`shutil.move`.
fn io_failure(e: &std::io::Error, path: &Path) -> TaskFailure {
    let p = path.display();
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            TaskFailure::new("FileNotFoundError", format!("[Errno 2] No such file or directory: '{p}'"))
        }
        std::io::ErrorKind::PermissionDenied => {
            TaskFailure::new("PermissionError", format!("[Errno 13] Permission denied: '{p}'"))
        }
        _ => TaskFailure::new("OSError", e.to_string()),
    }
}

struct Planned {
    src: PathBuf,
    name: String,
}

/// `strategy.add_document(document)` for all documents (originals or archive version).
async fn plan(cx: Ctx<'_>, docs: &[DocRow], archive_strategy: bool) -> Result<Vec<Planned>, TaskFailure> {
    let mut names: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let archive = archive_strategy && doc.has_archive_version();
        let src = if archive { docfile::archive_path(cx, doc) } else { docfile::source_path(cx, doc).await? };
        let Some(src) = src else {
            return Err(TaskFailure::new("ValueError", format!("path of document {} rejected", doc.id)));
        };
        let context = resolve::root_of(cx.db, doc).await?;
        let mut counter = 0u32;
        let name = loop {
            let candidate = docfile::public_filename(cx, doc, &context, archive, counter).await?;
            if names.contains(&zipw::normalize_arcname(&candidate)) {
                counter += 1;
            } else {
                break candidate;
            }
        };
        names.insert(zipw::normalize_arcname(&name));
        out.push(Planned { src, name });
    }
    Ok(out)
}

/// ZIP in `tmp_path` schreiben (ZIP_DEFLATED).
fn write_zip(planned: &[Planned], tmp_path: &Path, tz: chrono_tz::Tz) -> Result<(), TaskFailure> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(tmp_path)
        .map_err(|e| io_failure(&e, tmp_path))?;
    let mut z = ZipWriter::new(file, Method::Deflated, tz);
    for p in planned {
        z.write_file(&p.src, &p.name).map_err(|e| io_failure(&e, &p.src))?;
    }
    z.finish().map_err(|e| io_failure(&e, tmp_path))?;
    Ok(())
}

/// `mkstemp(suffix=".zip", dir=SCRATCH_DIR)`.
fn mkstemp(dir: &Path) -> Result<PathBuf, TaskFailure> {
    std::fs::create_dir_all(dir).map_err(|e| io_failure(&e, dir))?;
    for _ in 0..16 {
        let p = dir.join(format!("tmp{}.zip", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_failure(&e, &p)),
        }
    }
    Err(TaskFailure::new("FileExistsError", "No usable temporary file name found"))
}

/// Build and move the file; returns (`file_path`, `size_bytes`).
async fn build(state: &AppState, slug: &str, docs: &[DocRow], archive_strategy: bool, tmp: &Path) -> Result<(String, i64), TaskFailure> {
    let cfg = state.config();
    let planned = plan(Ctx::new(state), docs, archive_strategy).await?;
    let (tmp_owned, tz) = (tmp.to_path_buf(), cfg.tz);
    tokio::task::spawn_blocking(move || write_zip(&planned, &tmp_owned, tz))
        .await
        .map_err(|e| TaskFailure::new("RuntimeError", e.to_string()))??;
    let out_dir = &cfg.share_link_bundle_dir;
    std::fs::create_dir_all(out_dir).map_err(|e| io_failure(&e, out_dir))?;
    let file_name = format!("{slug}.zip");
    let final_path = out_dir.join(&file_name);
    if final_path.exists() {
        std::fs::remove_file(&final_path).map_err(|e| io_failure(&e, &final_path))?;
    }
    // shutil.move: rename, across file systems copy + unlink
    if std::fs::rename(tmp, &final_path).is_err() {
        std::fs::copy(tmp, &final_path).map_err(|e| io_failure(&e, tmp))?;
        let _ = std::fs::remove_file(tmp);
    }
    let size = std::fs::metadata(&final_path).map_err(|e| io_failure(&e, &final_path))?.len();
    Ok((file_name, size as i64))
}

/// `build_share_link_bundle(bundle_id)`. `Ok(())` also if the bundle no longer exists
/// (upstream: warning, `return`).
pub async fn build_share_link_bundle(state: &AppState, bundle_id: i64) -> Result<(), TaskFailure> {
    let db = state.db();
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT slug, file_version, file_path FROM documents_sharelinkbundle WHERE id = $1::int8")
            .bind(bundle_id)
            .fetch_optional(db)
            .await?;
    let Some((slug, file_version, file_path)) = row else {
        tracing::warn!(target: "paperless.tasks", "Share link bundle {bundle_id} no longer exists.");
        return Ok(());
    };
    crate::bundles::remove_file(&state.config().share_link_bundle_dir, &file_path);
    sqlx::query(
        "UPDATE documents_sharelinkbundle SET status = 'processing', last_error = NULL, size_bytes = NULL, \
         built_at = NULL, file_path = '' WHERE id = $1::int8",
    )
    .bind(bundle_id)
    .execute(db)
    .await?;

    // bundle.documents.all().order_by("pk") – default manager (without soft-deleted documents)
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT d.id::int8 FROM documents_sharelinkbundle_documents t JOIN documents_document d ON d.id = t.document_id \
         WHERE t.sharelinkbundle_id = $1::int8 AND d.deleted_at IS NULL ORDER BY d.id",
    )
    .bind(bundle_id)
    .fetch_all(db)
    .await?;
    let mut docs = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(d) = db::load_doc(db, id, false).await? {
            docs.push(d);
        }
    }

    let tmp = mkstemp(&state.config().scratch_dir)?;
    let result = build(state, &slug, &docs, file_version == "archive", &tmp).await;
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok((file_name, size)) => {
            sqlx::query(
                "UPDATE documents_sharelinkbundle SET file_path = $2, size_bytes = $3, status = 'ready', \
                 built_at = $4, last_error = NULL WHERE id = $1::int8",
            )
            .bind(bundle_id)
            .bind(&file_name)
            .bind(size)
            .bind(Utc::now())
            .execute(db)
            .await?;
            tracing::info!(target: "paperless.tasks", "Built share link bundle {bundle_id}");
            Ok(())
        }
        Err(f) => {
            tracing::error!(target: "paperless.tasks",
                "Failed to build share link bundle {bundle_id}: {}", f.message);
            let last_error = json!({
                "bundle_id": bundle_id,
                "exception_type": f.exception_type,
                "message": f.message,
                "timestamp": pl_core::datetime::python_isoformat(&Utc::now()),
            });
            sqlx::query("UPDATE documents_sharelinkbundle SET status = 'failed', last_error = $2 WHERE id = $1::int8")
                .bind(bundle_id)
                .bind(&last_error)
                .execute(db)
                .await?;
            Err(f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_id_argument() {
        let kw: Map<String, Value> = json!({"bundle_id": 5}).as_object().unwrap().clone();
        assert_eq!(bundle_id_arg(&[], &kw), Ok(5));
        assert_eq!(bundle_id_arg(&[json!(7)], &Map::new()), Ok(7));
        assert_eq!(bundle_id_arg(&[json!("8")], &Map::new()), Ok(8));
        assert_eq!(bundle_id_arg(&[], &Map::new()).unwrap_err().exception_type, "TypeError");
        assert_eq!(bundle_id_arg(&[json!("x")], &Map::new()).unwrap_err().exception_type, "ValueError");
    }

    #[test]
    fn io_errors_like_python() {
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        let f = io_failure(&e, Path::new("/m/o/1.pdf"));
        assert_eq!(f.exception_type, "FileNotFoundError");
        assert_eq!(f.message, "[Errno 2] No such file or directory: '/m/o/1.pdf'");
    }

    #[test]
    fn mkstemp_creates_unique_zip() {
        let dir = std::env::temp_dir().join(format!("pl-fv-mkstemp-{}", std::process::id()));
        let a = mkstemp(&dir).unwrap();
        let b = mkstemp(&dir).unwrap();
        assert_ne!(a, b);
        assert!(a.extension().is_some_and(|e| e == "zip"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
