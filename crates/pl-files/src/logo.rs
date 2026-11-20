// upstream: src/documents/views.py@v3.4.2 (serve_logo)
//           src/paperless/views.py@v3.4.2 (FaviconView)
//           django/http/response.py (FileResponse(as_attachment=True, filename=...))

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_core::AppState;
use tokio::io::AsyncReadExt;

use crate::compat;
use crate::config;
use crate::db;
use crate::docfile;
use crate::naming;
use crate::serve::{self, OpenError};

/// Small stand-in for `magic.from_file(path, mime=True)` (Known difference 8).
pub fn sniff_mime(head: &[u8]) -> &'static str {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png";
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return "image/webp";
    }
    if head.starts_with(&[0, 0, 1, 0]) {
        return "image/vnd.microsoft.icon";
    }
    if head.starts_with(b"BM") {
        return "image/bmp";
    }
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.contains("<svg") && (t.starts_with('<')) {
        return "image/svg+xml";
    }
    if head.is_empty() {
        return "application/x-empty";
    }
    "application/octet-stream"
}

/// GET /logo[/<filename>]
pub async fn serve_logo(State(state): State<AppState>) -> Response {
    let cfg = state.config();
    let app_config = match db::app_config(state.db()).await {
        Ok(c) => c,
        Err(e) => return compat::internal("logo config", e),
    };
    // `ApplicationConfiguration.objects.first()` being None -> AttributeError -> 500
    let Some(app_config) = app_config else {
        return compat::internal("logo", "no ApplicationConfiguration row");
    };
    let uploaded = app_config.app_logo.filter(|s| !s.is_empty());
    let (path, name, uploaded_logo) = if let Some(name) = uploaded {
        // FileSystemStorage.path -> safe_join(MEDIA_ROOT, name)
        match serve::safe_join(&cfg.media_root, &name) {
            Some(p) => (p, name, true),
            // Review 1 N6: `safe_join` → `SuspiciousFileOperation` → Django 400 (bad_request)
            None => return suspicious_logo_path(),
        }
    } else {
        let Some(env_logo) = cfg.app_logo.as_deref().filter(|s| !s.is_empty()) else {
            return compat::django_404();
        };
        let logo_root = config::resolve(&cfg.media_root.join("logo"));
        let path = config::resolve(&cfg.media_root.join(env_logo.trim_start_matches('/')));
        if !path.starts_with(&logo_root) || !path.is_file() {
            return compat::django_404();
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (path, name, false)
    };
    let mut opened = match serve::open(Some(&path)).await {
        Ok(f) => f,
        // uploaded logo missing: magic.from_file raises -> 500
        Err(OpenError::NotFound) if !uploaded_logo => return compat::django_404(),
        Err(OpenError::NotFound) => return compat::internal("logo", "file missing"),
        Err(OpenError::Other(e)) => return compat::internal("logo", e),
    };
    let mut head = vec![0u8; 512];
    let n = match opened.file.read(&mut head).await {
        Ok(n) => n,
        Err(e) => return compat::internal("logo read", e),
    };
    head.truncate(n);
    let mime = sniff_mime(&head);
    // Reopen to stream from the start (keeps memory constant).
    let opened = match serve::open(Some(&path)).await {
        Ok(f) => f,
        Err(_) => return compat::internal("logo", "reopen failed"),
    };
    let mut r = docfile::file_response(opened, mime);
    let base = std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(cd) = naming::django_content_disposition(true, &base)
        && let Ok(v) = HeaderValue::from_str(&cd)
    {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    r
}

/// GET /favicon.ico
pub async fn favicon(State(state): State<AppState>) -> Response {
    let path = state
        .config()
        .static_root
        .join("paperless")
        .join("img")
        .join("favicon.ico");
    match serve::open(Some(&path)).await {
        Ok(f) => {
            let mut r = docfile::file_response(f, "image/x-icon");
            if let Some(cd) = naming::django_content_disposition(false, "favicon.ico")
                && let Ok(v) = HeaderValue::from_str(&cd)
            {
                r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            r
        }
        Err(_) => compat::plain(StatusCode::NOT_FOUND, "favicon.ico not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nxxxx"), "image/png");
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_mime(b"<?xml version=\"1.0\"?><svg xmlns=\"x\"/>"), "image/svg+xml");
        assert_eq!(sniff_mime(b"plain"), "application/octet-stream");
    }
}

/// Django `response_for_exception(SuspiciousFileOperation)` → 400 `bad_request` page.
fn suspicious_logo_path() -> Response {
    tracing::warn!("pl_files logo: uploaded logo path outside MEDIA_ROOT (SuspiciousFileOperation)");
    pl_core::ApiError::django_bad_request().into_response()
}

#[cfg(test)]
mod n6_tests {
    /// Review 1 N6: uploaded logo outside MEDIA_ROOT → 400 like Django, not 500.
    #[test]
    fn suspicious_logo_is_400() {
        assert!(crate::serve::safe_join(std::path::Path::new("/media"), "../etc/passwd").is_none());
        assert_eq!(super::suspicious_logo_path().status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
