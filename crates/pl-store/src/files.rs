//! Files of a document: path, type, name, checksum. Permissions like v1 `download` (document
//! visible); the newest version is delivered (like v1 without `?version=`).

use pl_domain::model::{FileRef, FileVariant};
use pl_domain::{DomainError, DomainResult, Principal};
use uuid::Uuid;

use crate::documents::{self, FileRow};
use crate::{Store, auth, db_err};

pub(crate) async fn file(store: &Store, p: &Principal, uuid: Uuid, variant: FileVariant) -> DomainResult<FileRef> {
    let user = auth::user(p)?;
    let (id, _) = documents::visible_one(store, user, uuid).await?;
    let f: FileRow = documents::latest_files(store, &[id]).await?.remove(&id).ok_or(DomainError::NotFound)?;
    let (orig, arch, thumb) = documents::paths(store, &f).await?;
    let cx = pl_files::docfile::Ctx { db: &store.db, cfg: &store.cfg };
    let root = pl_files::db::load_doc(&store.db, id, false).await.map_err(db_err)?.ok_or(DomainError::NotFound)?;
    let row = f.doc_row();
    let use_archive = match variant {
        FileVariant::Archive => {
            if f.archive_filename.is_none() {
                return Err(DomainError::NotFound);
            }
            true
        }
        FileVariant::Preview => f.archive_filename.is_some(),
        FileVariant::Original => false,
        FileVariant::Thumbnail => {
            return Ok(FileRef {
                path: thumb,
                content_type: "image/webp".into(),
                filename: format!("{:07}.webp", f.id),
                checksum: None,
                inline: true,
            });
        }
    };
    let inline = variant == FileVariant::Preview;
    if use_archive {
        let path = arch.ok_or(DomainError::NotFound)?;
        let filename = pl_files::docfile::public_filename(cx, &row, &root, true, 0).await.map_err(db_err)?;
        return Ok(FileRef { path, content_type: "application/pdf".into(), filename, checksum: f.archive_checksum.clone(), inline });
    }
    let path = orig.ok_or(DomainError::NotFound)?;
    let filename = pl_files::docfile::public_filename(cx, &row, &root, false, 0).await.map_err(db_err)?;
    // like v1 serve_file: CSV inline as text, text kinds with charset
    let mut mime = f.mime_type.clone();
    if inline && (mime == "application/csv" || mime == "text/csv") {
        mime = "text/plain".into();
    }
    if matches!(mime.as_str(), "text/plain" | "text/csv" | "application/csv") {
        mime = format!("{mime}; charset=utf-8");
    }
    Ok(FileRef { path, content_type: mime, filename, checksum: Some(f.checksum.clone()), inline })
}
