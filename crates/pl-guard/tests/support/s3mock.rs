//! Small S3 replica for tests (127.0.0.1 only): PUT/GET/HEAD/DELETE, ListObjectsV2 with pages
//! (at most 3 keys per page), PutObjectRetention, Object Lock (delete/overwrite before
//! expiry → 403), checks the SigV4 signature of every request and `Content-MD5` on locked PUTs.
//!
//! **Not a replacement for a real S3 service** (versioning, governance bypass, multipart are missing).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use sha2::Digest;

#[derive(Clone, Default)]
pub struct Obj {
    pub data: Vec<u8>,
    pub retain_until: Option<DateTime<Utc>>,
    pub mode: Option<String>,
}

#[derive(Default)]
pub struct Bucket {
    pub objects: BTreeMap<String, Obj>,
    pub requests: usize,
    pub bad_signatures: usize,
}

pub type Shared = Arc<Mutex<Bucket>>;

pub const ACCESS: &str = "TESTACCESSKEY";
pub const SECRET: &str = "test/secret+key";
pub const BUCKET: &str = "paperrust-test";

fn err(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, format!("<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>{msg}</Message></Error>")).into_response()
}

fn verify(method: &Method, path: &str, query: &[(String, String)], headers: &HeaderMap) -> bool {
    let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) else { return false };
    let Some(signed) = auth.split("SignedHeaders=").nth(1).and_then(|s| s.split(',').next()) else { return false };
    let Some(amz) = headers.get("x-amz-date").and_then(|v| v.to_str().ok()) else { return false };
    let Ok(now) = chrono::NaiveDateTime::parse_from_str(amz, "%Y%m%dT%H%M%SZ") else { return false };
    let payload = headers.get("x-amz-content-sha256").and_then(|v| v.to_str().ok()).unwrap_or("");
    let hs: Vec<(String, String)> =
        signed.split(';').map(|h| (h.to_string(), headers.get(h).and_then(|v| v.to_str().ok()).unwrap_or("").to_string())).collect();
    let want = pl_guard::backup::s3::authorization(
        &pl_guard::backup::s3::SignInput { method: method.as_str(), path, query, headers: &hs, payload_sha256: payload },
        ACCESS,
        SECRET,
        "us-east-1",
        now.and_utc(),
    );
    want == auth
}

fn parse_query(q: Option<&str>) -> Vec<(String, String)> {
    q.unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let d = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string();
            (d(k), d(v))
        })
        .collect()
}

async fn handle(State(b): State<Shared>, req: Request) -> Response {
    let method = req.method().clone();
    let raw_path = req.uri().path().to_string();
    let query = parse_query(req.uri().query());
    let headers = req.headers().clone();
    let body: Bytes = match axum::body::to_bytes(req.into_body(), 64 << 20).await {
        Ok(b) => b,
        Err(_) => return err(StatusCode::BAD_REQUEST, "BadBody", "body"),
    };
    let mut g = b.lock().unwrap();
    g.requests += 1;
    if !verify(&method, &raw_path, &query, &headers) {
        g.bad_signatures += 1;
        return err(StatusCode::FORBIDDEN, "SignatureDoesNotMatch", "signature");
    }
    if headers.get("x-amz-content-sha256").and_then(|v| v.to_str().ok()) != Some(&pl_guard::backup::s3::sha256_hex(&body)) {
        return err(StatusCode::BAD_REQUEST, "XAmzContentSHA256Mismatch", "payload hash");
    }
    let path = percent_encoding::percent_decode_str(&raw_path).decode_utf8_lossy().to_string();
    let rest = path.trim_start_matches('/');
    let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket != BUCKET {
        return err(StatusCode::NOT_FOUND, "NoSuchBucket", "bucket");
    }
    let now = Utc::now();
    let q = |k: &str| query.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    match (method, key.is_empty()) {
        (Method::GET, true) => {
            let prefix = q("prefix").unwrap_or_default();
            let start = q("continuation-token").unwrap_or_default();
            let keys: Vec<&String> = g.objects.keys().filter(|k| k.starts_with(&prefix) && k.as_str() > start.as_str()).collect();
            let page: Vec<&String> = keys.iter().take(3).copied().collect();
            let truncated = keys.len() > 3;
            let mut x = String::from("<?xml version=\"1.0\"?><ListBucketResult>");
            for k in &page {
                let size = g.objects.get(k.as_str()).map(|o| o.data.len()).unwrap_or(0);
                x.push_str(&format!("<Contents><Key>{}</Key><Size>{size}</Size></Contents>", k.replace('&', "&amp;")));
            }
            x.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
            if truncated && let Some(last) = page.last() {
                x.push_str(&format!("<NextContinuationToken>{last}</NextContinuationToken>"));
            }
            x.push_str("</ListBucketResult>");
            x.into_response()
        }
        (Method::PUT, false) if q("retention").is_some() => {
            let s = String::from_utf8_lossy(&body).to_string();
            let until = s
                .split("<RetainUntilDate>")
                .nth(1)
                .and_then(|r| r.split('<').next())
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&Utc));
            let Some(o) = g.objects.get_mut(key) else { return err(StatusCode::NOT_FOUND, "NoSuchKey", "key") };
            if let (Some(old), Some(new)) = (o.retain_until, until)
                && new < old
            {
                return err(StatusCode::FORBIDDEN, "AccessDenied", "cannot shorten retention");
            }
            o.retain_until = until;
            StatusCode::OK.into_response()
        }
        (Method::PUT, false) => {
            if let Some(o) = g.objects.get(key)
                && o.retain_until.is_some_and(|u| u > now)
            {
                return err(StatusCode::FORBIDDEN, "AccessDenied", "object is locked (retention)");
            }
            let mode = headers.get("x-amz-object-lock-mode").and_then(|v| v.to_str().ok()).map(str::to_string);
            let until = headers
                .get("x-amz-object-lock-retain-until-date")
                .and_then(|v| v.to_str().ok())
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&Utc));
            if mode.is_some() {
                let md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(&body));
                if headers.get("content-md5").and_then(|v| v.to_str().ok()) != Some(md5.as_str()) {
                    return err(StatusCode::BAD_REQUEST, "InvalidRequest", "Content-MD5 required for Object Lock");
                }
            }
            g.objects.insert(key.to_string(), Obj { data: body.to_vec(), retain_until: until, mode });
            StatusCode::OK.into_response()
        }
        (Method::GET, false) => match g.objects.get(key) {
            Some(o) => o.data.clone().into_response(),
            None => err(StatusCode::NOT_FOUND, "NoSuchKey", "key"),
        },
        (Method::HEAD, false) => {
            if g.objects.contains_key(key) {
                StatusCode::OK.into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        (Method::DELETE, false) => {
            if g.objects.get(key).is_some_and(|o| o.retain_until.is_some_and(|u| u > now)) {
                return err(StatusCode::FORBIDDEN, "AccessDenied", "object is protected by retention");
            }
            g.objects.remove(key);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => err(StatusCode::METHOD_NOT_ALLOWED, "MethodNotAllowed", "method"),
    }
}

/// Starts the replica; returns endpoint and state.
pub async fn start() -> (String, Shared) {
    let state: Shared = Arc::default();
    let app = axum::Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), state)
}

/// S3 target for the replica.
pub fn target(endpoint: &str, prefix: &str, lock: pl_guard::config::LockMode) -> pl_guard::config::Target {
    pl_guard::config::Target::S3(pl_guard::config::S3Config {
        endpoint: endpoint.to_string(),
        region: "us-east-1".into(),
        access_key: ACCESS.into(),
        secret_key: SECRET.into(),
        bucket: BUCKET.into(),
        prefix: prefix.into(),
        lock,
        lock_days: 2,
    })
}
