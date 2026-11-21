//! PaperRust counterpart via API v2 (docs/api/v2.yaml, docs/contract/pl-api-v2.md).
//!
//! Login with device token (`Authorization: Bearer prt_…`); token only in the memory of this
//! object (loaded from the keychain), never in the log. No redirects (a redirect could carry
//! the token to a foreign origin), no cookies.

use std::sync::RwLock;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroizing;

use super::{Batch, ChangePage, DeviceGrant, Ids, RawResponse, Resolved, ServerInfo};
use crate::error::{Error, Result};
use crate::tls::{PIN_MISMATCH_MARKER, TlsOptions, client_config};

/// User agent of the client core.
pub const USER_AGENT: &str = concat!("PaperRust-Client/", env!("CARGO_PKG_VERSION"));

/// API prefix relative to the base URL.
pub const API: &str = "rs/api/v2/";

/// Boxed `Send` future (rust-lang/rust#102211 with borrowed arguments).
pub type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Default timeout per request (without long poll/files).
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct PaperRustRemote {
    base: Url,
    client: reqwest::Client,
    token: RwLock<Option<Zeroizing<String>>>,
}

impl std::fmt::Debug for PaperRustRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaperRustRemote").field("base", &self.base.as_str()).finish()
    }
}

/// Error chain as text (for classifying TLS/network).
fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// reqwest error → core error (without URL parts with secrets; tokens are never in URLs).
pub fn map_reqwest(e: reqwest::Error) -> Error {
    let text = chain(&e);
    if text.contains(PIN_MISMATCH_MARKER) {
        return Error::PinMismatch;
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("certificate") || lower.contains("tls handshake") || lower.contains("invalidcertificate") {
        return Error::Tls(text);
    }
    if e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() {
        return Error::Offline(text);
    }
    if e.is_decode() {
        return Error::Protocol(text);
    }
    Error::Offline(text)
}

/// Fehlerantwort (v2-Format) → Kern-Fehler.
async fn api_error(r: reqwest::Response) -> Error {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    let code = body["error"]["code"].as_str().unwrap_or("").to_string();
    let message = body["error"]["message"].as_str().unwrap_or("").to_string();
    match status {
        StatusCode::UNAUTHORIZED => Error::Unauthenticated,
        StatusCode::GONE => Error::CursorExpired,
        _ => Error::Api { status: status.as_u16(), code, message },
    }
}

/// Normalize base URL (always with `/` at the end, so that `join` keeps the subpath).
pub fn normalize_base(url: &str) -> Result<Url> {
    let mut u = Url::parse(url.trim()).map_err(|e| Error::Invalid(format!("server url: {e}")))?;
    if !matches!(u.scheme(), "https" | "http") {
        return Err(Error::Invalid("server url must be http(s)".into()));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(Error::Invalid("server url must not contain user info".into()));
    }
    u.set_query(None);
    u.set_fragment(None);
    if !u.path().ends_with('/') {
        let p = format!("{}/", u.path());
        u.set_path(&p);
    }
    Ok(u)
}

impl PaperRustRemote {
    pub fn new(base: &str, tls: &TlsOptions) -> Result<Self> {
        let base = normalize_base(base)?;
        let cfg = client_config(tls).map_err(Error::Tls)?;
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(cfg)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(60))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| Error::Tls(e.to_string()))?;
        Ok(Self { base, client, token: RwLock::new(None) })
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    pub fn set_token(&self, token: Option<Zeroizing<String>>) {
        if let Ok(mut t) = self.token.write() {
            *t = token;
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.read().map(|t| t.is_some()).unwrap_or(false)
    }

    fn url(&self, path: &str) -> Result<Url> {
        let path = path.trim_start_matches('/');
        self.base.join(path).map_err(|e| Error::Invalid(e.to_string()))
    }

    fn api_url(&self, rel: &str) -> Result<Url> {
        self.url(&format!("{API}{}", rel.trim_start_matches('/')))
    }

    /// `Authorization` value (`Bearer prt_…`) for the WebSocket relay ([`crate::wsrelay`]);
    /// never leaves the core (only written into the connection to the server).
    pub(crate) fn bearer(&self) -> Option<Zeroizing<String>> {
        self.token.read().ok().and_then(|t| t.as_ref().map(|t| Zeroizing::new(format!("Bearer {}", t.as_str()))))
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.token.read().ok().and_then(|t| t.clone()) {
            Some(t) => {
                let mut v = HeaderValue::from_str(&format!("Bearer {}", t.as_str())).unwrap_or(HeaderValue::from_static(""));
                v.set_sensitive(true);
                rb.header(reqwest::header::AUTHORIZATION, v)
            }
            None => rb,
        }
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let r = self.auth(rb).send().await.map_err(map_reqwest)?;
        if r.status().is_success() || r.status() == StatusCode::NOT_MODIFIED { Ok(r) } else { Err(api_error(r).await) }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, rel: &str, query: &[(&str, String)], timeout: Duration) -> Result<T> {
        let rb = self.client.get(self.api_url(rel)?).query(query).timeout(timeout).header("Accept", "application/json");
        let r = self.send(rb).await?;
        r.json::<T>().await.map_err(map_reqwest)
    }

    async fn post_json<T: serde::de::DeserializeOwned>(&self, rel: &'static str, body: Value) -> Result<T> {
        let rb = self.client.post(self.api_url(rel)?).json(&body).timeout(TIMEOUT).header("Accept", "application/json");
        let r = self.send(rb).await?;
        r.json::<T>().await.map_err(map_reqwest)
    }

    /// `GET /server` (with token, if present).
    pub async fn server(&self) -> Result<ServerInfo> {
        self.get_json("server", &[], Duration::from_secs(10)).await
    }

    /// `GET /me`.
    pub async fn me(&self) -> Result<Value> {
        self.get_json("me", &[], TIMEOUT).await
    }

    /// `GET /ui_settings`.
    pub async fn ui_settings(&self) -> Result<Value> {
        self.get_json("ui_settings", &[], TIMEOUT).await
    }

    /// Issue a device token (`POST /auth/devices`). The password is held by the caller as
    /// `Zeroizing`; the JSON buffer is overwritten after sending.
    pub async fn create_device(&self, username: &str, password: &str, code: Option<&str>, device_name: &str) -> Result<DeviceGrant> {
        let mut body = json!({"username": username, "password": password, "device_name": device_name});
        if let Some(c) = code.filter(|c| !c.trim().is_empty()) {
            body["code"] = json!(c.trim());
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&body)?);
        if let Some(p) = body.get_mut("password") {
            *p = Value::Null;
        }
        let rb = self
            .client
            .post(self.api_url("auth/devices")?)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .timeout(TIMEOUT)
            .body(bytes.to_vec());
        // without token (the output is anonymous)
        let r = rb.send().await.map_err(map_reqwest)?;
        if !r.status().is_success() {
            // 400 on a wrong login means "not logged in" here, not "invalid request"
            return Err(api_error(r).await);
        }
        let v: Value = r.json().await.map_err(map_reqwest)?;
        let token = v["token"].as_str().filter(|t| t.starts_with("prt_")).ok_or_else(|| Error::Protocol("device token missing".into()))?;
        Ok(DeviceGrant {
            token: Zeroizing::new(token.to_string()),
            device_uuid: v["device"]["uuid"].as_str().unwrap_or_default().to_string(),
            device_name: v["device"]["name"].as_str().unwrap_or(device_name).to_string(),
        })
    }

    /// Revoke own device (`DELETE /auth/devices/{uuid}`).
    pub async fn revoke_device(&self, device_uuid: &str) -> Result<()> {
        let rb = self.client.delete(self.api_url(&format!("auth/devices/{device_uuid}"))?).timeout(TIMEOUT);
        self.send(rb).await.map(|_| ())
    }

    /// `GET /sync/changes` (`wait` > 0 → Long-Poll).
    pub async fn changes(&self, cursor: Option<&str>, limit: u32, kinds: Option<&'static [&'static str]>, wait: u64) -> Result<ChangePage> {
        let mut q: Vec<(&str, String)> = vec![("limit", limit.to_string()), ("embed", "true".into())];
        if let Some(c) = cursor {
            q.push(("cursor", c.to_string()));
        }
        if let Some(k) = kinds {
            q.push(("kinds", k.join(",")));
        }
        if wait > 0 {
            q.push(("wait", wait.to_string()));
        }
        self.get_json("sync/changes", &q, TIMEOUT + Duration::from_secs(wait)).await
    }

    /// `POST /sync/resolve`.
    pub async fn resolve(&self, documents: &[String], tags: &[String], saved_views: &[String], tag_mode: Option<&str>) -> Result<Resolved> {
        let mut body = json!({"documents": documents, "tags": tags, "saved_views": saved_views});
        if let Some(m) = tag_mode {
            body["tag_mode"] = json!(m);
        }
        self.post_json("sync/resolve", body).await
    }

    /// `POST /objects/batch` (≤ 500 UUIDs).
    pub fn batch(&self, kind: &str, uuids: &[String], include: &[&str]) -> BoxFut<'_, Result<Batch>> {
        let body = json!({"kind": kind, "uuids": uuids, "include": include});
        Box::pin(async move { self.post_json("objects/batch", body).await })
    }

    /// `GET /sync/ids?kind=`.
    pub async fn ids(&self, kind: &str) -> Result<Ids> {
        self.get_json("sync/ids", &[("kind", kind.to_string())], Duration::from_secs(120)).await
    }

    /// Fetch a file. `if_none_match`: ETag of the cache state (→ 304). `resume`: (start byte,
    /// ETag of the partial state) → `Range` + `If-Range` (206 = continuation, 200 = the file has changed).
    pub async fn file(
        &self,
        uuid: &str,
        variant: &str,
        if_none_match: Option<&str>,
        resume: Option<(u64, &str)>,
    ) -> Result<reqwest::Response> {
        let mut h = HeaderMap::new();
        if let Some(e) = if_none_match.and_then(|e| HeaderValue::from_str(e).ok()) {
            h.insert(reqwest::header::IF_NONE_MATCH, e);
        }
        if let Some((start, etag)) = resume {
            if let Ok(v) = HeaderValue::from_str(&format!("bytes={start}-")) {
                h.insert(reqwest::header::RANGE, v);
            }
            if let Ok(v) = HeaderValue::from_str(etag) {
                h.insert(reqwest::header::IF_RANGE, v);
            }
        }
        let rb = self.client.get(self.api_url(&format!("documents/{uuid}/files/{variant}"))?).headers(h).timeout(Duration::from_secs(600));
        self.send(rb).await
    }

    /// Arbitrary request to the server (gateway, online mode). `path_and_query` is relative to the
    /// base URL (`rs/api/v2/…` or `api/…`); the caller checks the allow list.
    pub async fn raw(&self, method: &str, path_and_query: &str, headers: &[(String, String)], body: Vec<u8>) -> Result<RawResponse> {
        let method = Method::from_bytes(method.as_bytes()).map_err(|_| Error::Invalid("method".into()))?;
        let url = self.url(path_and_query)?;
        if url.origin() != self.base.origin() {
            return Err(Error::Invalid("path leaves the server origin".into()));
        }
        let mut rb = self.client.request(method, url).timeout(Duration::from_secs(120));
        for (k, v) in headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if !body.is_empty() {
            rb = rb.body(body);
        }
        let r = self.auth(rb).send().await.map_err(map_reqwest)?;
        let status = r.status().as_u16();
        let headers = r.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))).collect();
        let body = r.bytes().await.map_err(map_reqwest)?;
        Ok(RawResponse { status, headers, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_normalization() {
        assert_eq!(normalize_base("https://docs.example.ch").unwrap().as_str(), "https://docs.example.ch/");
        assert_eq!(normalize_base("https://h/paperless?x=1#y").unwrap().as_str(), "https://h/paperless/");
        assert!(normalize_base("ftp://h").is_err());
        assert!(normalize_base("https://u:p@h").is_err());
        let r = PaperRustRemote::new("https://h/sub", &TlsOptions { extra_roots: vec![], skip_native_roots: false, pin: None });
        if let Ok(r) = r {
            assert_eq!(r.api_url("sync/changes").unwrap().as_str(), "https://h/sub/rs/api/v2/sync/changes");
            assert!(r.url("https://evil.example/x").map(|u| u.origin() != r.base.origin()).unwrap_or(true));
        }
    }
}
