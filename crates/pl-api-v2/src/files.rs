//! `GET|HEAD /documents/{uuid}/files/{original|archive|preview|thumbnail}` with ETag/304 and
//! Range/If-Range/206/416 (docs/SYNC.md §3.4).

use std::io::SeekFrom;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use pl_domain::DomainError;
use pl_domain::model::FileRef;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::error::{V2Error, V2Result};

/// RFC 5987 `attr-char` (the rest is encoded).
const ATTR: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

pub fn content_disposition(inline: bool, filename: &str) -> String {
    let kind = if inline { "inline" } else { "attachment" };
    format!("{kind}; filename*=UTF-8''{}", utf8_percent_encode(filename, ATTR))
}

/// A byte range (including `end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// Result of the `Range` evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeReq {
    /// whole file
    Full,
    Partial(ByteRange),
    /// 416
    Unsatisfiable,
}

/// `Range: bytes=a-b | a- | -n`. Several ranges or other units → whole file
/// (permitted by RFC 9110 §14.2).
pub fn parse_range(h: Option<&str>, len: u64) -> RangeReq {
    let Some(h) = h else { return RangeReq::Full };
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return RangeReq::Full };
    if spec.contains(',') {
        return RangeReq::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return RangeReq::Full };
    let (a, b) = (a.trim(), b.trim());
    let r = if a.is_empty() {
        // Suffix: last n bytes
        let Ok(n) = b.parse::<u64>() else { return RangeReq::Full };
        if n == 0 || len == 0 {
            return RangeReq::Unsatisfiable;
        }
        ByteRange { start: len.saturating_sub(n), end: len - 1 }
    } else {
        let Ok(start) = a.parse::<u64>() else { return RangeReq::Full };
        let end = if b.is_empty() {
            len.saturating_sub(1)
        } else {
            match b.parse::<u64>() {
                Ok(e) if e >= start => e.min(len.saturating_sub(1)),
                _ => return RangeReq::Full,
            }
        };
        if start >= len {
            return RangeReq::Unsatisfiable;
        }
        ByteRange { start, end }
    };
    RangeReq::Partial(r)
}

/// `If-None-Match` matches (weak comparison, `*` always matches).
pub fn none_match(h: &HeaderMap, etag: &str) -> bool {
    let Some(v) = h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) else { return false };
    let bare = |s: &str| s.trim().trim_start_matches("W/").to_string();
    v.split(',').any(|t| t.trim() == "*" || bare(t) == bare(etag))
}

/// `If-Range` allows the partial fetch (strong comparison; never weak ETags).
pub fn if_range_ok(h: &HeaderMap, etag: &str) -> bool {
    match h.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(v) => !etag.starts_with("W/") && v.trim() == etag,
    }
}

pub async fn serve(method: &Method, h: &HeaderMap, f: FileRef) -> V2Result<Response> {
    let mut file = match tokio::fs::File::open(&f.path).await {
        Ok(x) => x,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(V2Error(DomainError::NotFound)),
        Err(e) => return Err(V2Error(DomainError::internal(e))),
    };
    let meta = file.metadata().await.map_err(DomainError::internal)?;
    if !meta.is_file() {
        return Err(V2Error(DomainError::NotFound));
    }
    let len = meta.len();
    let etag = match &f.checksum {
        Some(c) => format!("\"{c}\""),
        None => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("W/\"{mtime}-{len}\"")
        }
    };
    let mut b = Response::builder()
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "private, max-age=0")
        .header(header::ACCEPT_RANGES, "bytes");
    if none_match(h, &etag) {
        return b.status(StatusCode::NOT_MODIFIED).body(Body::empty()).map_err(|e| V2Error(DomainError::internal(e)));
    }
    b = b.header(header::CONTENT_TYPE, &f.content_type);
    if let Ok(cd) = HeaderValue::from_str(&content_disposition(f.inline, &f.filename)) {
        b = b.header(header::CONTENT_DISPOSITION, cd);
    }
    let range_hdr = h.get(header::RANGE).and_then(|v| v.to_str().ok());
    let range = if if_range_ok(h, &etag) { parse_range(range_hdr, len) } else { RangeReq::Full };
    let (status, start, count) = match range {
        RangeReq::Full => (StatusCode::OK, 0, len),
        RangeReq::Partial(r) => {
            b = b.header(header::CONTENT_RANGE, format!("bytes {}-{}/{len}", r.start, r.end));
            (StatusCode::PARTIAL_CONTENT, r.start, r.end - r.start + 1)
        }
        RangeReq::Unsatisfiable => {
            return b
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .header(header::CONTENT_LENGTH, 0)
                .body(Body::empty())
                .map_err(|e| V2Error(DomainError::internal(e)));
        }
    };
    b = b.status(status).header(header::CONTENT_LENGTH, count);
    if method == Method::HEAD {
        return b.body(Body::empty()).map_err(|e| V2Error(DomainError::internal(e)));
    }
    if start > 0 {
        file.seek(SeekFrom::Start(start)).await.map_err(DomainError::internal)?;
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(count), 64 * 1024);
    b.body(Body::from_stream(stream)).map_err(|e| V2Error(DomainError::internal(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 10), RangeReq::Full);
        assert_eq!(parse_range(Some("bytes=0-3"), 10), RangeReq::Partial(ByteRange { start: 0, end: 3 }));
        assert_eq!(parse_range(Some("bytes=5-"), 10), RangeReq::Partial(ByteRange { start: 5, end: 9 }));
        assert_eq!(parse_range(Some("bytes=-4"), 10), RangeReq::Partial(ByteRange { start: 6, end: 9 }));
        assert_eq!(parse_range(Some("bytes=-40"), 10), RangeReq::Partial(ByteRange { start: 0, end: 9 }));
        assert_eq!(parse_range(Some("bytes=8-100"), 10), RangeReq::Partial(ByteRange { start: 8, end: 9 }));
        assert_eq!(parse_range(Some("bytes=10-"), 10), RangeReq::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,4-5"), 10), RangeReq::Full);
        assert_eq!(parse_range(Some("items=0-1"), 10), RangeReq::Full);
        assert_eq!(parse_range(Some("bytes=5-2"), 10), RangeReq::Full);
    }

    #[test]
    fn etag_matching() {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"a\", W/\"b\""));
        assert!(none_match(&h, "\"b\""));
        assert!(none_match(&h, "W/\"a\""));
        assert!(!none_match(&h, "\"c\""));
        let mut h = HeaderMap::new();
        h.insert(header::IF_RANGE, HeaderValue::from_static("\"a\""));
        assert!(if_range_ok(&h, "\"a\""));
        assert!(!if_range_ok(&h, "\"b\""));
        assert!(!if_range_ok(&h, "W/\"a\""));
    }

    #[test]
    fn disposition() {
        assert_eq!(content_disposition(false, "Rechnung März.pdf"), "attachment; filename*=UTF-8''Rechnung%20M%C3%A4rz.pdf");
        assert_eq!(content_disposition(true, "a\"b.pdf"), "inline; filename*=UTF-8''a%22b.pdf");
    }
}
