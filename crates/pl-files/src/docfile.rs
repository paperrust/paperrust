// upstream: src/documents/views.py@v3.4.2 (serve_file)
//           src/documents/models.py@v3.4.2 (source_path, archive_path, thumbnail_path,
//           get_public_filename, file_type)

use std::path::PathBuf;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use pl_core::{AppState, Config};
use sqlx::PgPool;

use crate::compat;
use crate::db::{self, DocRow};
use crate::naming;
use crate::serve::{self, OpenError, OpenedFile};

/// Database + settings for one request.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub db: &'a PgPool,
    pub cfg: &'a Config,
}

impl<'a> Ctx<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Ctx { db: state.db(), cfg: state.config() }
    }
}

/// `Document.file_type` (queries the app config only for `image/tiff`, the one type whose
/// extension depends on the remote OCR configuration).
pub async fn file_type(cx: Ctx<'_>, mime_type: &str) -> Result<&'static str, sqlx::Error> {
    let remote = if mime_type == "image/tiff" {
        let row = db::app_config(cx.db).await?;
        db::remote_ocr_active(row.as_ref(), cx.cfg)
    } else {
        false
    };
    Ok(naming::default_file_extension(mime_type, remote))
}

/// `Document.source_path` (None = rejected by the traversal guard).
pub async fn source_path(cx: Ctx<'_>, doc: &DocRow) -> Result<Option<PathBuf>, sqlx::Error> {
    let name = match doc.filename.as_deref() {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => format!("{:07}{}", doc.id, file_type(cx, &doc.mime_type).await?),
    };
    Ok(serve::safe_join(&cx.cfg.originals_dir, &name))
}

/// `Document.archive_path`.
pub fn archive_path(cx: Ctx<'_>, doc: &DocRow) -> Option<PathBuf> {
    doc.archive_filename.as_deref().and_then(|f| serve::safe_join(&cx.cfg.archive_dir, f))
}

/// `Document.thumbnail_path`.
pub fn thumbnail_path(cx: Ctx<'_>, doc: &DocRow) -> PathBuf {
    cx.cfg.thumbnail_dir.join(format!("{:07}.webp", doc.id))
}

/// `get_public_filename(archive, counter)` – `context` is the root document.
pub async fn public_filename(cx: Ctx<'_>, doc: &DocRow, context: &DocRow, archive: bool, counter: u32) -> Result<String, sqlx::Error> {
    let ext = if archive { ".pdf" } else { file_type(cx, &doc.mime_type).await? };
    let s = naming::document_str(context.created, context.correspondent.as_deref(), &context.title);
    Ok(naming::public_filename(&s, ext, counter))
}

pub enum ServeError {
    NotFound,
    Db(sqlx::Error),
    Io(std::io::Error),
}

impl From<sqlx::Error> for ServeError {
    fn from(e: sqlx::Error) -> Self {
        ServeError::Db(e)
    }
}

impl From<OpenError> for ServeError {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::NotFound => ServeError::NotFound,
            OpenError::Other(e) => ServeError::Io(e),
        }
    }
}

pub fn file_response(f: OpenedFile, content_type: &str) -> Response {
    let len = f.len;
    let mut r = Response::new(serve::file_body(f.file, None));
    *r.status_mut() = StatusCode::OK;
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    r
}

/// `serve_file(doc=..., use_archive=..., disposition=..., follow_formatting=...)`.
/// `context` is the document whose metadata names the file (its root).
pub async fn serve_file(
    cx: Ctx<'_>,
    doc: &DocRow,
    context: &DocRow,
    use_archive: bool,
    disposition: &str,
    follow_formatting: bool,
) -> Result<Response, ServeError> {
    let (path, filename, mime) = if use_archive {
        let path = archive_path(cx, doc);
        let filename = if follow_formatting {
            doc.archive_filename.clone().unwrap_or_default()
        } else {
            public_filename(cx, doc, context, true, 0).await?
        };
        (path, filename, "application/pdf".to_string())
    } else {
        let path = source_path(cx, doc).await?;
        let filename =
            if follow_formatting { doc.filename.clone().unwrap_or_default() } else { public_filename(cx, doc, context, false, 0).await? };
        let mut mime = doc.mime_type.clone();
        // Support browser previewing csv files by using text mime type
        if (mime == "application/csv" || mime == "text/csv") && disposition == "inline" {
            mime = "text/plain".to_string();
        }
        if matches!(mime.as_str(), "text/plain" | "text/csv" | "application/csv") {
            mime = format!("{mime}; charset=utf-8");
        }
        (path, filename, mime)
    };
    let opened = serve::open(path.as_deref()).await?;
    let mut r = file_response(opened, &mime);
    let cd = naming::serve_file_disposition(disposition, &filename);
    match disposition_header(&cd) {
        Some(v) => {
            r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
        // Review 1 N7: Django `HttpResponse.__setitem__` → `BadHeaderError` (CR/LF in a DB
        // filename) → 500, not a 200 without Content-Disposition.
        None => return Ok(compat::internal("serve_file", "BadHeaderError: newline in Content-Disposition")),
    }
    Ok(r)
}

/// `Content-Disposition` value; `None` where Django raises `BadHeaderError` (CR/LF) or the
/// value is otherwise not a valid header.
pub fn disposition_header(cd: &str) -> Option<HeaderValue> {
    if cd.contains(['\n', '\r']) {
        return None;
    }
    HeaderValue::from_str(cd).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N7: CR/LF from DB filenames → no header (caller answers 500 like Django).
    #[test]
    fn disposition_with_newline_is_rejected() {
        assert!(disposition_header("attachment; filename=\"a\r\nb.pdf\"").is_none());
        assert!(disposition_header("inline; filename=\"a\nb.pdf\"").is_none());
        assert!(disposition_header("inline; filename=\"ab.pdf\"").is_some());
    }
}
