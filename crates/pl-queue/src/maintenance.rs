// upstream: src/documents/tasks.py@v3.4.2 (cleanup_expired_share_link_bundles),
//           src/documents/models.py@v3.4.2 (ShareLinkBundle.absolute_file_path, remove_file, delete)
//
//! Maintenance tasks without a dedicated domain crate (gap A1).

use std::path::{Component, Path, PathBuf};

use sqlx::PgPool;

/// Task name (beat entry `SHARE_LINK_BUNDLE_CLEANUP_CRON`, see [`crate::beat`]).
pub const CLEANUP_EXPIRED_SHARE_LINK_BUNDLES: &str = "documents.tasks.cleanup_expired_share_link_bundles";

/// `ShareLinkBundle.absolute_file_path`: only relative paths inside
/// `SHARE_LINK_BUNDLE_DIR` (after resolving symlinks), otherwise `None`.
pub fn bundle_file(bundle_dir: &Path, file_path: &str) -> Option<PathBuf> {
    if file_path.is_empty() {
        return None;
    }
    let rel = PathBuf::from(file_path);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)) {
        return None;
    }
    let dir = bundle_dir.canonicalize().ok()?;
    let abs = dir.join(&rel).canonicalize().ok()?;
    abs.starts_with(&dir).then_some(abs)
}

/// `remove_file()`. PaperRust (Known difference): upstream ignores deletion errors
/// (`except OSError: pass`); here `Ok(())` means the file is gone (deleted, absent, or a path
/// outside the bundle directory that must not be touched), `Err` that it is still there.
fn remove_file(bundle_dir: &Path, file_path: &str) -> std::io::Result<()> {
    match bundle_file(bundle_dir, file_path).filter(|p| p.exists()) {
        Some(p) => match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
        None => Ok(()),
    }
}

/// `cleanup_expired_share_link_bundles()`: delete every bundle with `expiration < now` (file,
/// M2M rows, bundle); errors per bundle only as a warning. The row is deleted only after the ZIP is
/// gone; a bundle whose file cannot be deleted stays for the next run (otherwise the ZIP with
/// document contents would stay on disk without any reference). Returns: number of bundles really
/// deleted (upstream counts all expired ones, also failed ones).
pub async fn cleanup_expired_share_link_bundles(pool: &PgPool, bundle_dir: &Path) -> Result<usize, sqlx::Error> {
    let expired: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id::int8, file_path FROM documents_sharelinkbundle \
         WHERE expiration IS NOT NULL AND expiration < now() ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let found = expired.len();
    let mut count = 0usize;
    for (id, file_path) in expired {
        if let Err(e) = remove_file(bundle_dir, file_path.as_deref().unwrap_or("")) {
            tracing::warn!(target: "paperless.tasks", "Failed to delete the file of expired share link bundle {id}, keeping it for the next run: {e}");
            continue;
        }
        let r: Result<(), sqlx::Error> = async {
            let mut tx = pool.begin().await?;
            sqlx::query("DELETE FROM documents_sharelinkbundle_documents WHERE sharelinkbundle_id = $1::int8")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM documents_sharelinkbundle WHERE id = $1::int8").bind(id).execute(&mut *tx).await?;
            tx.commit().await
        }
        .await;
        match r {
            Ok(()) => count += 1,
            Err(e) => tracing::warn!(target: "paperless.tasks", "Failed to delete expired share link bundle {id}: {e}"),
        }
    }
    if found > 0 {
        tracing::info!(target: "paperless.tasks", "Deleted {count} of {found} expired share link bundle(s)");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_file_stays_inside_dir() {
        let dir = std::env::temp_dir().join(format!("pl-queue-bundle-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("x.zip");
        let _ = std::fs::write(&f, b"z");
        assert!(bundle_file(&dir, "x.zip").is_some());
        assert!(bundle_file(&dir, "").is_none());
        assert!(bundle_file(&dir, "../x.zip").is_none());
        assert!(bundle_file(&dir, "/etc/passwd").is_none());
        remove_file(&dir, "x.zip").unwrap();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: a ZIP that cannot be deleted is reported (so its row is kept); absent files
    /// and paths outside the directory count as gone.
    #[test]
    fn remove_file_reports_failure() {
        let dir = std::env::temp_dir().join(format!("pl-queue-bundle-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(remove_file(&dir, "missing.zip").is_ok());
        assert!(remove_file(&dir, "../outside.zip").is_ok());
        // a non-empty directory under the bundle name cannot be removed with remove_file
        std::fs::create_dir_all(dir.join("stuck.zip").join("x")).unwrap();
        let r = remove_file(&dir, "stuck.zip");
        let still_there = dir.join("stuck.zip").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.is_err());
        assert!(still_there);
    }

    /// Needs a database with the paperless schema (reference DB operated only by the reference agent,
    /// must not be modified) – hence not automatic.
    #[tokio::test]
    #[ignore = "braucht eine beschreibbare paperless-Datenbank (PAPERRUST_TEST_DATABASE_URL)"]
    async fn cleanup_against_database() {
        let Ok(url) = std::env::var("PAPERRUST_TEST_DATABASE_URL") else { return };
        let Ok(pool) = PgPool::connect(&url).await else { return };
        let n = cleanup_expired_share_link_bundles(&pool, &std::env::temp_dir()).await;
        assert!(n.is_ok());
    }
}
