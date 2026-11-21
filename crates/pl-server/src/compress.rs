//! Response compression for Rust-handled responses, like upstream's
//! `StreamAwareCompressionMiddleware` (enabled unless `PAPERLESS_ENABLE_COMPRESSION=no`).
//!
//! upstream: src/paperless/middleware.py@v3.4.2 (StreamAwareCompressionMiddleware)
//! upstream: django-compression-middleware@0.5.0 compression_middleware/middleware.py,
//!           br.py (quality 4), zstd.py (level 7); django/utils/text.py (gzip level 6)
//!
//! "Streaming" (Django `StreamingHttpResponse`/`FileResponse`) is approximated as
//! "the body has no exact size" or the response carries [`Streaming`].

use std::io;
use std::pin::Pin;

use async_compression::Level;
use async_compression::tokio::bufread::{BrotliEncoder, GzipEncoder, ZstdEncoder};
use axum::body::Body;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use futures::TryStreamExt;
use http_body_util::BodyExt;
use hyper::body::Body as _;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::{ReaderStream, StreamReader};

use crate::django_mw::patch_vary_headers;

const MIN_LEN: u64 = 500;
const MIN_IMPROVEMENT: usize = 100;

/// Response extension: treat as a streaming response (like Django's FileResponse)
/// even if the body size is known.
#[derive(Debug, Clone, Copy)]
pub struct Streaming;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Zstd,
    Br,
    Gzip,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Zstd => "zstd",
            Encoding::Br => "br",
            Encoding::Gzip => "gzip",
        }
    }
}

/// `encoding_name`: q-values are ignored except q=0.
fn encoding_name(s: &str) -> Option<&str> {
    let mut name = s;
    if let Some((n, q)) = s.split_once(';') {
        name = n;
        if let Some((_, qv)) = q.split_once('=')
            && let Ok(v) = qv.trim().parse::<f64>()
            && v == 0.0
        {
            return None;
        }
    }
    Some(name.trim())
}

/// `compressor(accept_encoding)`: preference zstd > br > gzip; `*` = first choice.
pub fn choose(accept_encoding: &str) -> Option<Encoding> {
    let cut: String = accept_encoding.chars().take(200).collect();
    let client: Vec<Option<&str>> = cut.split(',').map(encoding_name).collect();
    let has = |n: &str| client.contains(&Some(n));
    if has("*") {
        return Some(Encoding::Zstd);
    }
    [Encoding::Zstd, Encoding::Br, Encoding::Gzip].into_iter().find(|e| has(e.name()))
}

type BoxRead = Pin<Box<dyn AsyncRead + Send>>;

fn encoder<R: tokio::io::AsyncBufRead + Send + 'static>(enc: Encoding, r: R) -> BoxRead {
    match enc {
        Encoding::Zstd => Box::pin(ZstdEncoder::with_quality(r, Level::Precise(7))),
        Encoding::Br => Box::pin(BrotliEncoder::with_quality(r, Level::Precise(4))),
        Encoding::Gzip => Box::pin(GzipEncoder::with_quality(r, Level::Precise(6))),
    }
}

fn is_event_stream(resp: &Response) -> bool {
    resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|ct| ct.starts_with("text/event-stream"))
}

fn weaken_etag(resp: &mut Response) {
    let weak = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|e| e.starts_with('"'))
        .and_then(|e| HeaderValue::from_str(&format!("W/{e}")).ok());
    if let Some(w) = weak {
        resp.headers_mut().insert(header::ETAG, w);
    }
}

/// Apply the compression middleware to a Rust response.
pub async fn compress(resp: Response, accept_encoding: &str) -> Response {
    if is_event_stream(&resp) || resp.headers().contains_key(header::CONTENT_ENCODING) {
        return resp;
    }
    let exact = resp.body().size_hint().exact();
    let streaming = resp.extensions().get::<Streaming>().is_some() || exact.is_none();
    if !streaming && exact.unwrap_or(0) < MIN_LEN {
        return resp;
    }
    let mut resp = resp;
    patch_vary_headers(resp.headers_mut(), &["Accept-Encoding"]);
    let Some(enc) = choose(accept_encoding) else {
        return resp;
    };

    let (mut parts, body) = resp.into_parts();
    if streaming {
        let stream = TryStreamExt::map_err(body.into_data_stream(), io::Error::other);
        let reader = StreamReader::new(stream);
        let body = Body::from_stream(ReaderStream::new(encoder(enc, reader)));
        parts.headers.remove(header::CONTENT_LENGTH);
        let mut resp = Response::from_parts(parts, body);
        weaken_etag(&mut resp);
        resp.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc.name()));
        return resp;
    }

    let content = match BodyExt::collect(body).await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "response body failed before compression");
            return Response::from_parts(parts, Body::empty());
        }
    };
    let mut out = Vec::new();
    let compressed = encoder(enc, io::Cursor::new(content.clone())).read_to_end(&mut out).await;
    if compressed.is_err() || out.len() >= content.len().saturating_sub(MIN_IMPROVEMENT) {
        return Response::from_parts(parts, Body::from(content));
    }
    let len = out.len();
    let mut resp = Response::from_parts(parts, Body::from(out));
    resp.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    weaken_etag(&mut resp);
    resp.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc.name()));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_like_upstream() {
        assert_eq!(choose(""), None);
        assert_eq!(choose("gzip, deflate, br, zstd"), Some(Encoding::Zstd));
        assert_eq!(choose("gzip, deflate, br"), Some(Encoding::Br));
        assert_eq!(choose("gzip;q=1.0, br;q=0.1"), Some(Encoding::Br));
        assert_eq!(choose("gzip, br;q=0"), Some(Encoding::Gzip));
        assert_eq!(choose("*"), Some(Encoding::Zstd));
        assert_eq!(choose("identity"), None);
    }

    #[tokio::test]
    async fn small_bodies_untouched_large_compressed() {
        let r = compress(Response::new(Body::from("x".repeat(100))), "gzip").await;
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(r.headers().get(header::VARY).is_none());

        let mut big = Response::new(Body::from("abc".repeat(1000)));
        big.headers_mut().insert(header::ETAG, HeaderValue::from_static("\"x\""));
        let r = compress(big, "gzip").await;
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(r.headers()[header::VARY], "Accept-Encoding");
        assert_eq!(r.headers()[header::ETAG], "W/\"x\"");

        let mut sse = Response::new(Body::from("a".repeat(1000)));
        sse.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        let r = compress(sse, "gzip").await;
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none());
    }
}
