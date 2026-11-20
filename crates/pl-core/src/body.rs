//! Request-body limit like Django.
//!
//! upstream: django@5.2.18 conf/global_settings.py (`DATA_UPLOAD_MAX_MEMORY_SIZE = 2621440`,
//! `FILE_UPLOAD_MAX_MEMORY_SIZE = 2621440`; paperless settings override neither),
//! http/request.py (`HttpRequest.body` → `RequestDataTooBig`, a `SuspiciousOperation` → 400).
//!
//! `pl-server` sends every request whose body is larger than [`DATA_UPLOAD_MAX_MEMORY_SIZE`]
//! straight to Django (Rust handlers never see it) and caps axum's `DefaultBodyLimit` at the
//! same value. Handlers that read a body themselves use [`read_body`], never
//! `to_bytes(body, usize::MAX)`.

use axum::body::{Body, Bytes};

use crate::ApiError;

/// Django `DATA_UPLOAD_MAX_MEMORY_SIZE` (2.5 MiB). Also the largest body pl-server buffers.
pub const DATA_UPLOAD_MAX_MEMORY_SIZE: usize = 2_621_440;

/// Read the whole body, at most [`DATA_UPLOAD_MAX_MEMORY_SIZE`] bytes. Larger (or an
/// unreadable body) → Django's `Bad Request (400)` page, like `RequestDataTooBig`.
pub async fn read_body(body: Body) -> Result<Bytes, ApiError> {
    axum::body::to_bytes(body, DATA_UPLOAD_MAX_MEMORY_SIZE).await.map_err(|e| {
        tracing::warn!(error = %e, "request body rejected");
        ApiError::django_bad_request()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limit() {
        assert_eq!(&read_body(Body::from("abc")).await.unwrap()[..], b"abc");
        let big = vec![b'x'; DATA_UPLOAD_MAX_MEMORY_SIZE + 1];
        let e = read_body(Body::from(big)).await.unwrap_err();
        assert_eq!(e.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
