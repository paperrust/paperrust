//! Per-route request gate in front of every Rust handler:
//!
//! 1. `POST`/`PUT`/`PATCH` with a form or multipart body on a route that does not parse it
//!    (no [`RouteFlags::accepts_form`] / [`RouteFlags::accepts_multipart`]) is not supported:
//!    below `/api/` DRF's `415 {"detail": "Unsupported media type \"…\" in request."}` (what
//!    DRF answers for a parser the view does not have; parity run 2026-11-20: clearer than the
//!    former 404), elsewhere the 404 of a path without a route (docs/STANDALONE-GAPS.md §5; DRF's
//!    default parsers accept JSON, form and multipart, most Rust handlers only JSON).
//! 0. A request whose DRF format suffix was rejected ([`crate::frontend::urlconf::RejectFormat`],
//!    e.g. `/api/tags/1.5/`) gets DRF's `404 {"detail": "Not found."}` with the view's `Allow`
//!    before any authentication (content negotiation fails in `APIView.initial` first).
//! 2. The request body is buffered up to [`BODY_BUFFER_LIMIT`] (= Django's
//!    `DATA_UPLOAD_MAX_MEMORY_SIZE`, 2.5 MiB). A larger body is **never** handled in Rust:
//!    Django's 400 page (`RequestDataTooBig`). This caps the memory an anonymous client can make
//!    Rust allocate before any permission check (Review 2, H1).
//! 3. A handler answering [`pl_core::markers::DelegateToDjango`] ("case not supported by
//!    PaperRust") gets the 404 of a path without a route.
//! 4. **Streaming routes** ([`Gate::streaming`], `crate::upload::STREAMING_ROUTES`, e.g.
//!    `documents.post_document`): the body is not buffered; the handler reads it itself (file
//!    parts spooled to disk, non-file data limited like Django's `DATA_UPLOAD_MAX_MEMORY_SIZE`).
//!    Such handlers must not answer "not supported".
//!
//! upstream: rest_framework/settings.py@3.16 (DEFAULT_PARSER_CLASSES: JSON, Form, MultiPart),
//! django@5.2.18 conf/global_settings.py (DATA_UPLOAD_MAX_MEMORY_SIZE)

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use pl_core::RouteFlags;
use pl_core::markers::is_delegation;

/// Largest request body handled (and buffered) in Rust; larger bodies get Django's 400.
pub const BODY_BUFFER_LIMIT: usize = pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE;

/// State of the gate of one route.
#[derive(Clone)]
pub struct Gate {
    pub flags: RouteFlags,
    pub limit: usize,
    /// Pass the body through unbuffered (upload routes that spool to disk themselves).
    pub streaming: bool,
    /// `Allow` of the view (all methods of the path), for answers given by the gate itself.
    pub allow: Option<std::sync::Arc<str>>,
}

/// Body that the route cannot parse → reason for the 404.
pub fn unsupported_body(method: &Method, headers: &axum::http::HeaderMap, flags: &RouteFlags) -> Option<&'static str> {
    if !matches!(*method, Method::POST | Method::PUT | Method::PATCH) {
        return None;
    }
    let ct = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let media = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match media.as_str() {
        "application/x-www-form-urlencoded" if !flags.accepts_form => Some("form body on a JSON-only route"),
        "multipart/form-data" if !flags.accepts_multipart => Some("multipart body on a JSON-only route"),
        _ => None,
    }
}

enum Buffered {
    /// Whole body in memory.
    Full(Bytes),
    /// Over the limit; the rest of the body is still unread.
    TooLarge(Body),
    /// Reading the body failed (client went away, malformed chunking).
    Failed,
}

async fn buffer(body: Body, limit: usize) -> Buffered {
    let hint = body.size_hint();
    if hint.exact() == Some(0) {
        return Buffered::Full(Bytes::new());
    }
    if hint.lower() > limit as u64 {
        return Buffered::TooLarge(body);
    }
    let mut body = body;
    let mut chunks: Vec<Bytes> = Vec::new();
    let mut total = 0usize;
    loop {
        match body.frame().await {
            None => break,
            Some(Err(e)) => {
                tracing::debug!(error = %e, "reading request body failed");
                return Buffered::Failed;
            }
            Some(Ok(frame)) => {
                // Request trailers are not supported by Django either; drop them.
                let Ok(data) = frame.into_data() else { continue };
                total += data.len();
                chunks.push(data);
                if total > limit {
                    return Buffered::TooLarge(body);
                }
            }
        }
    }
    Buffered::Full(match chunks.len() {
        0 => Bytes::new(),
        1 => chunks.pop().unwrap_or_default(),
        _ => {
            let mut v = Vec::with_capacity(total);
            for c in &chunks {
                v.extend_from_slice(c);
            }
            Bytes::from(v)
        }
    })
}

/// Upper bound for reading and discarding the rest of an oversized body (bytes / time).
const DRAIN_MAX_BYTES: u64 = 64 * 1024 * 1024;
const DRAIN_MAX_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// Reads and discards the rest of an oversized body (bounded), so the 400 answer reaches the
/// client: closing a socket with unread data makes the OS (notably Windows) send RST, and the
/// client then sees a connection reset instead of the response. Bodies larger than the bound
/// are cut off (the connection closes as before).
async fn drain(mut body: Body) {
    if body.size_hint().lower() > DRAIN_MAX_BYTES {
        return;
    }
    let _ = tokio::time::timeout(DRAIN_MAX_TIME, async {
        let mut seen = 0u64;
        while let Some(Ok(frame)) = body.frame().await {
            if let Ok(data) = frame.into_data() {
                seen += data.len() as u64;
                if seen > DRAIN_MAX_BYTES {
                    break;
                }
            }
        }
    })
    .await;
}

/// The answer for a body the route cannot parse: DRF's 415 below `/api/`, else the 404 of a path
/// without a route.
fn not_supported(parts: &Parts, why: &str, allow: Option<&str>) -> Response {
    let path = parts.uri.path();
    if path == "/api" || path.starts_with("/api/") {
        tracing::debug!(method = %parts.method, path, why, "body type not supported by PaperRust: 415");
        let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
        let mut resp = pl_core::ApiError::unsupported_media_type(&ct).into_response();
        if let Some(a) = allow {
            pl_core::route::apply_drf_view_headers(resp.headers_mut(), a);
        }
        return resp;
    }
    tracing::debug!(method = %parts.method, path, why, "not supported by PaperRust: 404");
    crate::frontend::not_found(path)
}

pub async fn gate(State(g): State<Gate>, req: Request, next: Next) -> Response {
    if req.extensions().get::<crate::frontend::urlconf::RejectFormat>().is_some() {
        let mut resp = pl_core::ApiError::not_found().into_response();
        if let Some(a) = g.allow.as_deref() {
            pl_core::route::apply_drf_view_headers(resp.headers_mut(), a);
        }
        return resp;
    }
    if let Some(reason) = unsupported_body(req.method(), req.headers(), &g.flags) {
        let (parts, _) = req.into_parts();
        return not_supported(&parts, reason, g.allow.as_deref());
    }
    if g.streaming {
        let resp = next.run(req).await;
        if is_delegation(&resp) {
            // A streaming handler must not answer "not supported" (it may have consumed the body).
            tracing::error!("streaming route answered \"not supported\"");
            return pl_core::ApiError::internal("delegation from a streaming route").into_response();
        }
        return resp;
    }
    let (parts, body) = req.into_parts();
    let t0 = std::time::Instant::now();
    let buffered = buffer(body, g.limit).await;
    if let Some(t) = parts.extensions.get::<crate::frontline::AccessTrace>() {
        t.set_gate(t0.elapsed());
    }
    match buffered {
        Buffered::Full(bytes) => {
            let path = parts.uri.path().to_owned();
            let resp = next.run(Request::from_parts(parts, Body::from(bytes))).await;
            if is_delegation(&resp) {
                tracing::debug!(path = %path, "handler answered \"not supported\": 404");
                return crate::frontend::not_found(&path);
            }
            resp
        }
        Buffered::TooLarge(rest) => {
            // docs/STANDALONE-GAPS.md §5: `RequestDataTooBig` → Django's 400 page (JSON/form
            // bodies count against DATA_UPLOAD_MAX_MEMORY_SIZE).
            tracing::debug!(path = %parts.uri.path(), "body over the limit: 400");
            drain(rest).await;
            pl_core::ApiError::django_bad_request().into_response()
        }
        Buffered::Failed => pl_core::ApiError::django_bad_request().into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn content_types() {
        let f = RouteFlags::default();
        let mut h = HeaderMap::new();
        assert_eq!(unsupported_body(&Method::POST, &h, &f), None);
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert_eq!(unsupported_body(&Method::POST, &h, &f), None);
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("Multipart/Form-Data; boundary=x"));
        assert!(unsupported_body(&Method::PATCH, &h, &f).is_some());
        assert_eq!(unsupported_body(&Method::GET, &h, &f), None);
        let f = RouteFlags { accepts_multipart: true, ..RouteFlags::default() };
        assert_eq!(unsupported_body(&Method::PATCH, &h, &f), None);
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-www-form-urlencoded"));
        assert!(unsupported_body(&Method::PUT, &h, &f).is_some());
    }

    #[tokio::test]
    async fn buffering_limit() {
        match buffer(Body::from("abc"), 3).await {
            Buffered::Full(b) => assert_eq!(&b[..], b"abc"),
            _ => panic!("expected full"),
        }
        let chunks = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from("ab")), Ok(Bytes::from("cd"))]);
        assert!(matches!(buffer(Body::from_stream(chunks), 3).await, Buffered::TooLarge(_)));
    }
}
