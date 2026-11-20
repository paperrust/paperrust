//! Webhook sending (workflow action "webhook") with SSRF protection.
//!
//! upstream: src/documents/workflows/webhooks.py@v3.4.2 (send_webhook: `retry_backoff=True`,
//! `autoretry_for=(httpx.HTTPStatusError,)`, `max_retries=3`), src/paperless/network.py@v3.4.2
//! (validate_outbound_http_url, PinnedHostHTTPTransport/_rewrite_request_to_pinned_ip,
//! is_public_ip, resolve_hostname_ips), src/paperless/settings/__init__.py@v3.4.2
//! (WEBHOOKS_ALLOWED_SCHEMES, WEBHOOKS_ALLOWED_PORTS, WEBHOOKS_ALLOW_INTERNAL_REQUESTS),
//! httpx 0.28.1 (`_content.encode_request`: content > files > data > json; `encode_json`
//! compact with `ensure_ascii=False`; multipart like `_multipart.MultipartStream`; timeout 5 s;
//! `follow_redirects=False`; `raise_for_status` on every status except 2xx).
//!
//! Protection: the target IP is resolved **once** and checked, the connection goes to exactly that IP
//! (`reqwest::ClientBuilder::resolve`), Host header/SNI stay the host name – no DNS rebinding.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{Map, Value, json};

use crate::imap::{is_public_ip, resolve_hostname_ips};

pub const TASK_SEND_WEBHOOK: &str = "documents.workflows.webhooks.send_webhook";

/// `WEBHOOKS_*` settings.
#[derive(Debug, Clone)]
pub struct WebhookSettings {
    pub allowed_schemes: BTreeSet<String>,
    pub allowed_ports: BTreeSet<u16>,
    pub allow_internal: bool,
    pub timeout: Duration,
}

impl WebhookSettings {
    /// From the environment (`get_list_from_env` / `get_bool_from_env`).
    pub fn from_env(env: &pl_core::config::Env) -> Result<Self, String> {
        let schemes = env.list("PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES", &["http", "https"]).into_iter().map(|s| s.to_lowercase()).collect();
        let mut ports = BTreeSet::new();
        for p in env.list("PAPERLESS_WEBHOOKS_ALLOWED_PORTS", &[]) {
            let v: i64 = p.trim().parse().map_err(|_| format!("invalid literal for int() with base 10: '{p}'"))?;
            // ports outside 0..65535 can never match
            if let Ok(v) = u16::try_from(v) {
                ports.insert(v);
            }
        }
        let allow_internal = env.bool("PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS", "true").map_err(|e| format!("{e:?}"))?;
        Ok(Self { allowed_schemes: schemes, allowed_ports: ports, allow_internal, timeout: Duration::from_secs(5) })
    }
}

/// Webhook error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookError {
    /// `ValueError` from `validate_outbound_http_url` ("Webhook blocked"), no retry.
    Blocked(String),
    /// `httpx.ConnectError` (incl. blocking of internal addresses); PaperRust: retry like `Status` (tests/chaos C4).
    Connect(String),
    /// `httpx.HTTPStatusError` → automatic retry.
    Status(u16),
    /// other `httpx.HTTPError` (timeout, …), no retry.
    Http(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebhookError::Blocked(m) | WebhookError::Connect(m) | WebhookError::Http(m) => f.write_str(m),
            WebhookError::Status(s) => write!(f, "HTTP status {s}"),
        }
    }
}

/// Validated target.
#[derive(Debug, Clone)]
pub struct Target {
    pub url: url::Url,
    pub host: String,
    pub port: u16,
}

/// `validate_outbound_http_url(url, allowed_schemes, allowed_ports, allow_internal=True)`.
pub fn validate_outbound_url(raw: &str, s: &WebhookSettings) -> Result<Target, WebhookError> {
    let bad = || WebhookError::Blocked("Invalid URL scheme or hostname.".into());
    // urlparse: "http:///x" has no host (WHATWG would read "x" as the host)
    if let Some((_, rest)) = raw.split_once("://")
        && (rest.starts_with('/') || rest.starts_with('\\'))
    {
        return Err(bad());
    }
    let url = url::Url::parse(raw).map_err(|_| bad())?;
    let scheme = url.scheme().to_lowercase();
    let host = match url.host() {
        Some(url::Host::Domain(d)) if !d.is_empty() => d.to_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        _ => return Err(bad()),
    };
    if !s.allowed_schemes.contains(&scheme) {
        return Err(bad());
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = url.port().unwrap_or(default_port);
    if !s.allowed_ports.is_empty() && !s.allowed_ports.contains(&port) {
        return Err(WebhookError::Blocked("Destination port not permitted.".into()));
    }
    // Only http(s) can actually be sent (httpx knows no other schemes).
    if scheme != "http" && scheme != "https" {
        return Err(WebhookError::Http(format!("Request URL has an unsupported protocol '{scheme}://'.")));
    }
    Ok(Target { url, host, port })
}

/// `_rewrite_request_to_pinned_ip`: resolve, block internal addresses, use the first IP.
pub async fn pin(target: &Target, allow_internal: bool) -> Result<IpAddr, WebhookError> {
    let ips: Vec<IpAddr> = match target.host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => resolve_hostname_ips(&target.host).await.map_err(WebhookError::Connect)?,
    };
    if !allow_internal && ips.iter().any(|ip| !is_public_ip(*ip)) {
        return Err(WebhookError::Connect(format!("Connection blocked: {} resolves to a non-public address", target.host)));
    }
    ips.first().copied().ok_or_else(|| WebhookError::Connect(format!("Could not resolve hostname: {}", target.host)))
}

/// File attachment (`files={"file": (name, bytes, mime)}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookFile {
    pub name: String,
    pub content: Vec<u8>,
    pub mime: String,
}

/// Arguments of `send_webhook`.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookRequest {
    pub url: String,
    /// `str` (text) or `dict` (form/JSON object).
    pub data: Value,
    pub headers: Vec<(String, String)>,
    pub file: Option<WebhookFile>,
    pub as_json: bool,
}

impl WebhookRequest {
    /// kwargs for the queue (file base64-encoded).
    pub fn to_kwargs(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("url".into(), json!(self.url));
        m.insert("data".into(), self.data.clone());
        let mut h = Map::new();
        for (k, v) in &self.headers {
            h.insert(k.clone(), json!(v));
        }
        m.insert("headers".into(), Value::Object(h));
        m.insert(
            "files".into(),
            match &self.file {
                None => Value::Null,
                Some(f) => json!({"file": [f.name, {"__type__": "bytes", "__value__": base64::engine::general_purpose::STANDARD.encode(&f.content)}, f.mime]}),
            },
        );
        m.insert("as_json".into(), json!(self.as_json));
        m
    }

    pub fn from_kwargs(kw: &Map<String, Value>) -> Result<Self, String> {
        let url = kw.get("url").and_then(Value::as_str).ok_or("missing url")?.to_string();
        let data = kw.get("data").cloned().unwrap_or(Value::Object(Map::new()));
        let headers = match kw.get("headers") {
            Some(Value::Object(h)) => h.iter().map(|(k, v)| (k.clone(), crate::pyval::to_str(v))).collect(),
            _ => Vec::new(),
        };
        let file = match kw.get("files").and_then(|f| f.get("file")).and_then(Value::as_array) {
            Some(a) if a.len() == 3 => {
                let name = a[0].as_str().unwrap_or("").to_string();
                let b64 = a[1].get("__value__").and_then(Value::as_str).unwrap_or("");
                let content = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?;
                Some(WebhookFile { name, content, mime: a[2].as_str().unwrap_or("").to_string() })
            }
            _ => None,
        };
        let as_json = kw.get("as_json").and_then(Value::as_bool).unwrap_or(false);
        Ok(Self { url, data, headers, file, as_json })
    }
}

/// `_HTML5_FORM_ENCODING_REPLACEMENTS` (httpx `_format_form_param`).
fn form_param(name: &str, value: &str) -> String {
    let mut v = String::new();
    for c in value.chars() {
        match c {
            '"' => v.push_str("%22"),
            '\\' => v.push_str("\\\\"),
            c if (c as u32) < 0x20 && c as u32 != 0x1b => v.push_str(&format!("%{:02X}", c as u32)),
            c => v.push(c),
        }
    }
    format!("{name}=\"{v}\"")
}

/// `primitive_value_to_str`.
fn primitive(v: &Value) -> String {
    match v {
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => crate::pyval::to_str(other),
    }
}

/// Body + content type like `httpx._content.encode_request(content, data, files, json)`.
pub fn encode_body(req: &WebhookRequest) -> (Option<String>, Vec<u8>) {
    // content=data (str) takes precedence over files
    if !req.as_json
        && let Value::String(s) = &req.data
    {
        return (None, s.as_bytes().to_vec());
    }
    let form: Vec<(String, String)> = match (&req.data, req.as_json) {
        (Value::Object(m), false) => m.iter().map(|(k, v)| (k.clone(), primitive(v))).collect(),
        _ => Vec::new(),
    };
    if let Some(f) = &req.file {
        let boundary = hex16();
        let mut body = Vec::new();
        for (k, v) in &form {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; {}\r\n\r\n", form_param("name", k)).as_bytes());
            body.extend_from_slice(v.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; {}", form_param("name", "file")).as_bytes());
        if !f.name.is_empty() {
            body.extend_from_slice(format!("; {}", form_param("filename", &f.name)).as_bytes());
        }
        if !f.mime.is_empty() {
            body.extend_from_slice(format!("\r\nContent-Type: {}", f.mime).as_bytes());
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(&f.content);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        return (Some(format!("multipart/form-data; boundary={boundary}")), body);
    }
    if !form.is_empty() {
        let s: String = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form.iter()).finish();
        return (Some("application/x-www-form-urlencoded".into()), s.into_bytes());
    }
    if req.as_json && !req.data.is_null() {
        return (Some("application/json".into()), compact_json(&req.data).into_bytes());
    }
    (None, Vec::new())
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`.
fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn hex16() -> String {
    let b: [u8; 16] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `send_webhook(url, data, headers, files, as_json=…)` – one attempt.
pub async fn send_webhook(req: &WebhookRequest, s: &WebhookSettings) -> Result<u16, WebhookError> {
    let target = match validate_outbound_url(&req.url, s) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target: "paperless.workflows.webhooks", "Webhook blocked: {e}");
            return Err(e);
        }
    };
    let r = send_pinned(req, &target, s).await;
    match &r {
        Ok(_) => tracing::info!(target: "paperless.workflows.webhooks", "Webhook sent to {}", req.url),
        Err(e) => tracing::error!(target: "paperless.workflows.webhooks", "Failed attempt sending webhook to {}: {e}", req.url),
    }
    r
}

async fn send_pinned(req: &WebhookRequest, target: &Target, s: &WebhookSettings) -> Result<u16, WebhookError> {
    let ip = pin(target, s.allow_internal).await?;
    let mut b = reqwest::Client::builder().user_agent(pl_core::brand::USER_AGENT).timeout(s.timeout).redirect(reqwest::redirect::Policy::none()).no_proxy();
    if target.host.parse::<IpAddr>().is_err() {
        b = b.resolve(&target.host, SocketAddr::new(ip, target.port));
    }
    let client = b.build().map_err(|e| WebhookError::Http(e.to_string()))?;
    let (ct, body) = encode_body(req);
    let mut rb = client.post(target.url.clone());
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("host") {
            continue;
        }
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(ct) = ct {
        rb = rb.header(reqwest::header::CONTENT_TYPE, ct);
    }
    let resp = rb.body(body).send().await.map_err(|e| {
        if e.is_connect() {
            WebhookError::Connect(e.without_url().to_string())
        } else {
            WebhookError::Http(e.without_url().to_string())
        }
    })?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(WebhookError::Status(status));
    }
    Ok(status)
}

/// Handler for pl-queue (`documents.workflows.webhooks.send_webhook`).
pub async fn send_webhook_task(s: Arc<WebhookSettings>, ctx: pl_queue::TaskContext) -> Result<Value, pl_queue::TaskError> {
    let req = WebhookRequest::from_kwargs(&ctx.kwargs).map_err(|e| pl_queue::TaskError::fail("TypeError", e))?;
    match send_webhook(&req, &s).await {
        Ok(_) => Ok(Value::Null),
        Err(WebhookError::Status(code)) => {
            Err(pl_queue::TaskError::autoretry("HTTPStatusError", format!("Server error '{code}' for url '{}'", req.url)))
        }
        Err(WebhookError::Blocked(m)) => Err(pl_queue::TaskError::fail("ValueError", m)),
        // tests/chaos C4: target unreachable (network gone, receiver restarting) → retry with the same
        // rule as for server errors (3×, backoff). The request never arrived, so a retry
        // cannot trigger anything twice. upstream: only HTTPStatusError is retried.
        Err(WebhookError::Connect(m)) if connect_retryable(&m) => Err(pl_queue::TaskError::autoretry("ConnectError", m)),
        Err(WebhookError::Connect(m)) => Err(pl_queue::TaskError::fail("ConnectError", m)),
        Err(WebhookError::Http(m)) => Err(pl_queue::TaskError::fail("HTTPError", m)),
    }
}

/// Connection error where a retry helps (target unreachable) – not the SSRF block of internal
/// addresses ("non-public"), which is final.
pub fn connect_retryable(msg: &str) -> bool {
    !msg.contains("non-public")
}

fn retryable(e: &WebhookError) -> bool {
    match e {
        WebhookError::Status(_) => true,
        WebhookError::Connect(m) => connect_retryable(m),
        _ => false,
    }
}

/// Direct sending without a queue with the same retry rule (3 retries,
/// backoff `2^n` s with jitter, max. 600 s).
pub async fn send_with_retries(req: &WebhookRequest, s: &WebhookSettings) -> Result<u16, WebhookError> {
    let policy = pl_queue::registry::task_def(TASK_SEND_WEBHOOK).and_then(|d| d.autoretry);
    let mut retries = 0u32;
    loop {
        match send_webhook(req, s).await {
            Err(e @ WebhookError::Status(_)) | Err(e @ WebhookError::Connect(_)) if retryable(&e) => {
                let Some(p) = policy else { return Err(e) };
                if p.exhausted(retries) {
                    return Err(e);
                }
                let wait = p.countdown(retries, &mut rand::thread_rng());
                tokio::time::sleep(Duration::from_secs(wait)).await;
                retries += 1;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> WebhookSettings {
        WebhookSettings {
            allowed_schemes: ["http".to_string(), "https".to_string()].into_iter().collect(),
            allowed_ports: BTreeSet::new(),
            allow_internal: false,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn validation() {
        let s = settings();
        assert!(validate_outbound_url("http://example.org/x", &s).is_ok());
        assert_eq!(validate_outbound_url("ftp://example.org/x", &s).unwrap_err(), WebhookError::Blocked("Invalid URL scheme or hostname.".into()));
        assert_eq!(validate_outbound_url("http:///x", &s).unwrap_err(), WebhookError::Blocked("Invalid URL scheme or hostname.".into()));
        let mut s2 = settings();
        s2.allowed_ports.insert(443);
        assert_eq!(validate_outbound_url("http://example.org/x", &s2).unwrap_err(), WebhookError::Blocked("Destination port not permitted.".into()));
        assert!(validate_outbound_url("https://example.org/x", &s2).is_ok());
    }

    #[tokio::test]
    async fn internal_addresses_blocked() {
        let s = settings();
        for u in ["http://127.0.0.1/", "http://2130706433/", "http://[::1]/", "http://10.0.0.1/", "http://169.254.169.254/latest", "http://localhost:8080/"] {
            let t = validate_outbound_url(u, &s).unwrap();
            let r = pin(&t, false).await;
            assert!(matches!(r, Err(WebhookError::Connect(ref m)) if m.starts_with("Connection blocked")), "{u}: {r:?}");
        }
        let t = validate_outbound_url("http://127.0.0.1:9/", &s).unwrap();
        assert!(pin(&t, true).await.is_ok());
    }

    #[test]
    fn body_encoding_like_httpx() {
        let mut r = WebhookRequest { url: "http://x/".into(), data: json!("{\"id\": \"5\"}"), headers: vec![], file: None, as_json: true };
        assert_eq!(encode_body(&r), (Some("application/json".into()), b"\"{\\\"id\\\": \\\"5\\\"}\"".to_vec()));
        r.as_json = false;
        assert_eq!(encode_body(&r), (None, b"{\"id\": \"5\"}".to_vec()));
        r.data = json!({"a": "1 2", "b": "ä"});
        assert_eq!(encode_body(&r), (Some("application/x-www-form-urlencoded".into()), b"a=1+2&b=%C3%A4".to_vec()));
        r.data = json!({});
        assert_eq!(encode_body(&r).1, Vec::<u8>::new());
        r.file = Some(WebhookFile { name: "a\"b.pdf".into(), content: b"PDF".to_vec(), mime: "application/pdf".into() });
        let (ct, body) = encode_body(&r);
        assert!(ct.unwrap().starts_with("multipart/form-data; boundary="));
        let b = String::from_utf8(body).unwrap();
        assert!(b.contains("Content-Disposition: form-data; name=\"file\"; filename=\"a%22b.pdf\"\r\nContent-Type: application/pdf\r\n\r\nPDF\r\n"));
    }
}
