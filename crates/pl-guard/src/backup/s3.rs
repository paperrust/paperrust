//! Minimal S3 client (path-style addressing, AWS Signature Version 4) for backups:
//! PUT (with Object Lock), GET, HEAD, DELETE, ListObjectsV2, PutObjectRetention.
//!
//! Deliberately without the AWS SDK (a very large dependency). Signature according to
//! <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html>,
//! verified with the example from the AWS documentation (test `aws_documentation_example`).

use base64::Engine as _;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::config::{LockMode, S3Config};

type HmacSha256 = Hmac<Sha256>;

/// RFC 3986 "unreserved" stays, everything else is encoded (like AWS `UriEncode`).
const URI_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
const URI_ENCODE_PATH: &AsciiSet = &URI_ENCODE.remove(b'/');

pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length; the error branch exists only for form's sake
    let Ok(mut m) = <HmacSha256 as Mac>::new_from_slice(key) else { return Vec::new() };
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

pub fn sha256_hex(data: &[u8]) -> String {
    crate::guard::hex(&Sha256::digest(data))
}

/// A request to be signed.
pub struct SignInput<'a> {
    pub method: &'a str,
    /// already encoded path (`/bucket/key`)
    pub path: &'a str,
    /// (name, value) unencoded
    pub query: &'a [(String, String)],
    /// (lower-cased name, value) – at least `host`, `x-amz-date`, `x-amz-content-sha256`
    pub headers: &'a [(String, String)],
    pub payload_sha256: &'a str,
}

fn canonical_query(q: &[(String, String)]) -> String {
    let mut v: Vec<(String, String)> = q.iter().map(|(k, v)| (utf8_percent_encode(k, URI_ENCODE).to_string(), utf8_percent_encode(v, URI_ENCODE).to_string())).collect();
    v.sort();
    v.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// `Authorization` header (SigV4).
pub fn authorization(input: &SignInput<'_>, access_key: &str, secret_key: &str, region: &str, now: DateTime<Utc>) -> String {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let mut headers: Vec<(String, String)> = input.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method,
        input.path,
        canonical_query(input.query),
        canonical_headers,
        signed_headers,
        input.payload_sha256
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));
    let k_date = hmac(format!("AWS4{secret_key}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = crate::guard::hex(&hmac(&k_signing, string_to_sign.as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}")
}

/// S3 client for a bucket with prefix.
#[derive(Clone)]
pub struct Client {
    cfg: S3Config,
    http: reqwest::Client,
}

/// Error of an S3 request.
#[derive(Debug)]
pub struct S3Error {
    pub status: Option<u16>,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(f, "S3 error {s} {}: {}", self.code, self.message),
            None => write!(f, "S3 request failed: {}", self.message),
        }
    }
}

fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let start = body.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = body[start..].find(&format!("</{tag}>"))? + start;
    Some(xml_unescape(&body[start..end]))
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

impl Client {
    pub fn new(cfg: S3Config) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .user_agent(pl_core::brand::USER_AGENT)
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { cfg, http })
    }

    pub fn config(&self) -> &S3Config {
        &self.cfg
    }

    fn full_key(&self, key: &str) -> String {
        if self.cfg.prefix.is_empty() { key.to_string() } else { format!("{}/{key}", self.cfg.prefix) }
    }

    fn host(&self) -> String {
        let e = self.cfg.endpoint.split("://").nth(1).unwrap_or(&self.cfg.endpoint);
        e.split('/').next().unwrap_or(e).to_string()
    }

    /// Sign and send the request.
    async fn send(
        &self,
        method: reqwest::Method,
        key: Option<&str>,
        query: Vec<(String, String)>,
        extra: Vec<(String, String)>,
        body: reqwest::Body,
        payload_sha256: &str,
    ) -> Result<reqwest::Response, S3Error> {
        let path = match key {
            Some(k) => format!("/{}/{}", self.cfg.bucket, utf8_percent_encode(&self.full_key(k), URI_ENCODE_PATH)),
            None => format!("/{}", self.cfg.bucket),
        };
        let now = Utc::now();
        let mut headers = vec![
            ("host".to_string(), self.host()),
            ("x-amz-date".to_string(), now.format("%Y%m%dT%H%M%SZ").to_string()),
            ("x-amz-content-sha256".to_string(), payload_sha256.to_string()),
        ];
        headers.extend(extra);
        let auth = authorization(
            &SignInput { method: method.as_str(), path: &path, query: &query, headers: &headers, payload_sha256 },
            &self.cfg.access_key,
            &self.cfg.secret_key,
            &self.cfg.region,
            now,
        );
        let qs = canonical_query(&query);
        let url = if qs.is_empty() { format!("{}{path}", self.cfg.endpoint) } else { format!("{}{path}?{qs}", self.cfg.endpoint) };
        let mut req = self.http.request(method, url).header("authorization", auth);
        for (k, v) in &headers {
            if k != "host" {
                req = req.header(k.as_str(), v.as_str());
            }
        }
        req.body(body).send().await.map_err(|e| S3Error { status: None, code: String::new(), message: e.to_string() })
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, S3Error> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(S3Error { status: Some(status), code: xml_tag(&body, "Code").unwrap_or_default(), message: xml_tag(&body, "Message").unwrap_or_else(|| body.chars().take(200).collect()) })
    }

    fn lock_headers(&self, until: Option<DateTime<Utc>>) -> Vec<(String, String)> {
        match (self.cfg.lock.header(), until) {
            (Some(mode), Some(until)) => vec![
                ("x-amz-object-lock-mode".into(), mode.into()),
                ("x-amz-object-lock-retain-until-date".into(), until.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            ],
            _ => Vec::new(),
        }
    }

    /// Retention period for new objects (`None` without Object Lock).
    pub fn retain_until(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        (self.cfg.lock != LockMode::Off).then(|| now + chrono::Duration::days(self.cfg.lock_days))
    }

    /// Upload an object. With Object Lock: retention period + `Content-MD5` (required by AWS/MinIO).
    pub async fn put(&self, key: &str, data: Vec<u8>, until: Option<DateTime<Utc>>) -> Result<(), S3Error> {
        let sha = sha256_hex(&data);
        let md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(&data));
        let mut extra = vec![("content-md5".to_string(), md5), ("content-type".to_string(), "application/octet-stream".to_string())];
        extra.extend(self.lock_headers(until));
        let r = self.send(reqwest::Method::PUT, Some(key), Vec::new(), extra, reqwest::Body::from(data), &sha).await?;
        Self::check(r).await.map(|_| ())
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, S3Error> {
        let r = self.send(reqwest::Method::GET, Some(key), Vec::new(), Vec::new(), reqwest::Body::from(Vec::new()), EMPTY_SHA256).await?;
        if r.status().as_u16() == 404 {
            return Ok(None);
        }
        let r = Self::check(r).await?;
        r.bytes().await.map(|b| Some(b.to_vec())).map_err(|e| S3Error { status: None, code: String::new(), message: e.to_string() })
    }

    pub async fn exists(&self, key: &str) -> Result<bool, S3Error> {
        let r = self.send(reqwest::Method::HEAD, Some(key), Vec::new(), Vec::new(), reqwest::Body::from(Vec::new()), EMPTY_SHA256).await?;
        match r.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            s => Err(S3Error { status: Some(s), code: String::new(), message: "HEAD failed".into() }),
        }
    }

    /// Delete; `Ok(false)` if the retention period (Object Lock) prevents it.
    pub async fn delete(&self, key: &str) -> Result<bool, S3Error> {
        let r = self.send(reqwest::Method::DELETE, Some(key), Vec::new(), Vec::new(), reqwest::Body::from(Vec::new()), EMPTY_SHA256).await?;
        match Self::check(r).await {
            Ok(_) => Ok(true),
            Err(e) if e.status == Some(403) && (e.code == "AccessDenied" || e.code.contains("ObjectLock") || e.message.contains("retention")) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Extend the retention period of an existing object (PutObjectRetention).
    pub async fn extend_retention(&self, key: &str, until: DateTime<Utc>) -> Result<(), S3Error> {
        let Some(mode) = self.cfg.lock.header() else { return Ok(()) };
        let body = format!(
            "<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Mode>{mode}</Mode><RetainUntilDate>{}</RetainUntilDate></Retention>",
            until.format("%Y-%m-%dT%H:%M:%SZ")
        )
        .into_bytes();
        let sha = sha256_hex(&body);
        let md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(&body));
        let r = self
            .send(reqwest::Method::PUT, Some(key), vec![("retention".into(), String::new())], vec![("content-md5".into(), md5)], reqwest::Body::from(body), &sha)
            .await?;
        Self::check(r).await.map(|_| ())
    }

    /// All keys under `prefix` (relative to the target's prefix).
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>, S3Error> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        let base = self.full_key("");
        loop {
            let mut q = vec![("list-type".to_string(), "2".to_string()), ("prefix".to_string(), self.full_key(prefix))];
            if let Some(t) = &token {
                q.push(("continuation-token".into(), t.clone()));
            }
            let r = self.send(reqwest::Method::GET, None, q, Vec::new(), reqwest::Body::from(Vec::new()), EMPTY_SHA256).await?;
            let body = Self::check(r).await?.text().await.map_err(|e| S3Error { status: None, code: String::new(), message: e.to_string() })?;
            let mut rest = body.as_str();
            while let Some(i) = rest.find("<Key>") {
                let after = &rest[i + 5..];
                let Some(j) = after.find("</Key>") else { break };
                let k = xml_unescape(&after[..j]);
                out.push(k.strip_prefix(&base).unwrap_or(&k).to_string());
                rest = &after[j..];
            }
            if xml_tag(&body, "IsTruncated").as_deref() == Some("true") {
                token = xml_tag(&body, "NextContinuationToken");
                if token.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Example "GET Object" from the AWS documentation (Signature Version 4, header variant):
    /// <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html>
    #[test]
    fn aws_documentation_example() {
        let now = DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z").unwrap().with_timezone(&Utc);
        let headers = vec![
            ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
            ("range".to_string(), "bytes=0-9".to_string()),
            ("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string()),
            ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
        ];
        let auth = authorization(
            &SignInput { method: "GET", path: "/test.txt", query: &[], headers: &headers, payload_sha256: EMPTY_SHA256 },
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            now,
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn query_encoding() {
        assert_eq!(canonical_query(&[("prefix".into(), "a b/c".into()), ("list-type".into(), "2".into())]), "list-type=2&prefix=a%20b%2Fc");
        assert_eq!(canonical_query(&[("retention".into(), String::new())]), "retention=");
    }
}
