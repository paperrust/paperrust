//! Remote OCR via Azure AI Document Intelligence – upstream `paperless/parsers/remote.py`
//! (`RemoteDocumentParser`, `_azure_ai_vision_parse`) and `paperless/network.py`
//! (`validate_outbound_http_url`, `is_public_ip`).
//!
//! upstream uses the Python SDK `azure-ai-documentintelligence` (API version 2024-11-30):
//! `begin_analyze_document(model_id="prebuilt-read", body=AnalyzeDocumentRequest(bytes_source=…),
//! output_content_format=TEXT, output=[PDF])` → poll until done → `result.content` (text) and
//! `get_analyze_result_pdf(...)` (searchable PDF as archive). Here the same REST calls with
//! reqwest:
//!
//! 1. `POST {endpoint}/documentintelligence/documentModels/prebuilt-read:analyze?api-version=2024-11-30&outputContentFormat=text&output=pdf`
//!    header `Ocp-Apim-Subscription-Key`, body `{"base64Source": "…"}` → 202 + `Operation-Location`.
//! 2. `GET Operation-Location` until `status` = `succeeded` (wait time from `Retry-After`, otherwise 1 s).
//! 3. `GET {endpoint}/documentintelligence/documentModels/prebuilt-read/analyzeResults/{id}/pdf?api-version=…`.
//!
//! Security like upstream: endpoint via `validate_outbound_http_url` (with
//! `REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS=false` no internal addresses), every further request
//! (poll URL, redirects) checked again; the key never goes to another host
//! (upstream: `blocked_redirect_headers`). The key appears in no log and no error message.
//!
//! Security review 2026-11-19 (F10): the host name is resolved once and checked, after that
//! the client connects only to exactly this address (upstream `PinnedHostHTTPTransport`, like
//! pl-ai `pinned_client`), without a proxy from `HTTP(S)_PROXY`. Responses are read at most up to
//! [`MAX_RESPONSE_BYTES`].

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::RemoteOcrConfig;
use crate::parser::{RemoteDecision, remote_decision};
use crate::{OcrError, Result};

/// API version of the Python SDK 1.0 (`azure-ai-documentintelligence`).
pub const API_VERSION: &str = "2024-11-30";
/// Model like upstream.
pub const MODEL_ID: &str = "prebuilt-read";
/// Maximum total duration of the poll (the SDK waits indefinitely; here protection against hangs).
pub const MAX_POLL: Duration = Duration::from_secs(30 * 60);

/// Maximum size of a response from Azure (JSON or archive PDF), security review 2026-11-19 F10.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Pinned address: host name and checked socket address.
pub type Pin = (String, std::net::SocketAddr);

fn user_agent() -> String {
    format!("PaperRust/{} remote-ocr", env!("CARGO_PKG_VERSION"))
}

// ----------------------------------------------------------------------------------------------
// paperless.network
// ----------------------------------------------------------------------------------------------

fn v4_in(ip: std::net::Ipv4Addr, net: [u8; 4], bits: u32) -> bool {
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    (u32::from(ip) & mask) == (u32::from(std::net::Ipv4Addr::from(net)) & mask)
}

fn v6_in(ip: std::net::Ipv6Addr, net: [u16; 8], bits: u32) -> bool {
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    (u128::from(ip) & mask) == (u128::from(std::net::Ipv6Addr::from(net)) & mask)
}

fn v4_is_private(ip: std::net::Ipv4Addr) -> bool {
    // Python 3.12 ipaddress._private_networks (IPv4)
    const NETS: &[([u8; 4], u32)] = &[
        ([0, 0, 0, 0], 8),
        ([10, 0, 0, 0], 8),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24),
        ([192, 0, 2, 0], 24),
        ([192, 168, 0, 0], 16),
        ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([255, 255, 255, 255], 32),
    ];
    if ip == std::net::Ipv4Addr::new(192, 0, 0, 9) || ip == std::net::Ipv4Addr::new(192, 0, 0, 10) {
        return false;
    }
    NETS.iter().any(|(n, b)| v4_in(ip, *n, *b))
}

/// `paperless.network.is_public_ip`.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4_is_private(v4) || v4.is_loopback() || v4.is_link_local() || v4.is_multicast() || v4.is_unspecified() || v4_in(v4, [100, 64, 0, 0], 10))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped()
                && !is_public_ip(IpAddr::V4(mapped))
            {
                return false;
            }
            const NETS: &[([u16; 8], u32)] = &[
                ([0, 0, 0, 0, 0, 0, 0, 1], 128),
                ([0, 0, 0, 0, 0, 0, 0, 0], 128),
                ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
                ([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
                ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
                ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
                ([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28),
                ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
                ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
                ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
                ([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96),
            ];
            !NETS.iter().any(|(n, b)| v6_in(v6, *n, *b))
        }
    }
}

/// `validate_outbound_http_url(url, allow_internal=…)`: error texts like upstream (`ValueError`).
pub fn validate_outbound_http_url(raw: &str, allow_internal: bool) -> std::result::Result<reqwest::Url, String> {
    validate_and_pin(raw, allow_internal).map(|(u, _)| u)
}

/// Like [`validate_outbound_http_url`], additionally returns the checked address for a DNS name
/// (`None` for IP literal or `allow_internal`). The caller connects exactly there, so that a DNS
/// answer changed between check and connection does not point to an internal service
/// (security review 2026-11-19 F10; same rule as pl-ai `validate_and_pin`).
pub fn validate_and_pin(raw: &str, allow_internal: bool) -> std::result::Result<(reqwest::Url, Option<Pin>), String> {
    let bad = || "Invalid URL scheme or hostname.".to_string();
    // urlparse: "http:///x" has no host
    if let Some((_, rest)) = raw.split_once("://")
        && (rest.starts_with('/') || rest.starts_with('\\'))
    {
        return Err(bad());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| bad())?;
    let scheme = url.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(bad());
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) if !d.is_empty() => d.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        _ => return Err(bad()),
    };
    let mut pin = None;
    if !allow_internal {
        let port = url.port_or_known_default().unwrap_or(443);
        let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => {
                use std::net::ToSocketAddrs;
                let addrs = (host.as_str(), port).to_socket_addrs().map_err(|_| format!("Could not resolve hostname: {host}"))?;
                let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
                if let Some(first) = ips.first() {
                    pin = Some((host.clone(), std::net::SocketAddr::new(*first, port)));
                }
                ips
            }
        };
        if ips.is_empty() {
            return Err(format!("Could not resolve hostname: {host}"));
        }
        if ips.iter().any(|ip| !is_public_ip(*ip)) {
            return Err(format!("Connection blocked: {host} resolves to a non-public address"));
        }
    }
    Ok((url, pin))
}

// ----------------------------------------------------------------------------------------------
// RemoteDocumentParser
// ----------------------------------------------------------------------------------------------

/// Result of [`parse`] (`get_text()`, `get_archive_path()`; `get_date()` is always `None`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteOutcome {
    pub text: String,
    pub archive_path: Option<PathBuf>,
}

/// `RemoteDocumentParser.parse(document_path, mime_type, produce_archive=…)`.
/// `extract_pdf_text`: local text extraction (upstream `extract_pdf_text`, pdftotext).
pub fn parse(
    cfg: &RemoteOcrConfig,
    tempdir: &Path,
    document: &Path,
    mime_type: &str,
    produce_archive: bool,
    extract_pdf_text: &dyn Fn(&Path) -> Option<String>,
) -> Result<RemoteOutcome> {
    match remote_decision(cfg.engine_is_valid(), cfg.engine.as_deref(), mime_type, produce_archive) {
        Some(RemoteDecision::EmptyNoEngine) => {
            tracing::warn!("No valid remote parser engine is configured, content will be empty.");
            Ok(RemoteOutcome::default())
        }
        Some(RemoteDecision::LocalText) => {
            tracing::debug!("Remote OCR: skipped — no archive requested, using locally-extracted text");
            let t = crate::text::post_process_text(extract_pdf_text(document).as_deref()).unwrap_or_default();
            Ok(RemoteOutcome { text: t, archive_path: None })
        }
        Some(RemoteDecision::CallAzure) => {
            let endpoint = cfg.endpoint.clone().unwrap_or_default();
            let key = cfg.api_key.clone().unwrap_or_default();
            let data = std::fs::read(document)?;
            let (text, pdf) = azure_analyze(&endpoint, &key, data, cfg.allow_internal_endpoints)?;
            std::fs::create_dir_all(tempdir)?;
            let archive = tempdir.join("archive.pdf");
            std::fs::write(&archive, pdf)?;
            Ok(RemoteOutcome { text, archive_path: Some(archive) })
        }
        None => Ok(RemoteOutcome::default()),
    }
}

/// Azure call (blocking). Runs in its own thread with its own Tokio runtime so that it can be
/// called from synchronous as well as asynchronous context.
pub fn azure_analyze(endpoint: &str, api_key: &str, document: Vec<u8>, allow_internal: bool) -> Result<(String, Vec<u8>)> {
    let (base, pin) = validate_and_pin(endpoint, allow_internal)
        .map_err(|e| OcrError::Parse(format!("Invalid remote OCR endpoint: {e}")))?;
    run_azure(base, endpoint, api_key, document, allow_internal, pin)
}

/// For tests only: like [`azure_analyze`], but with a given pinned address instead of
/// its own DNS check (shows that the connection goes exactly there). Without address check –
/// do not use in production code.
#[doc(hidden)]
pub fn azure_analyze_pinned(endpoint: &str, api_key: &str, document: Vec<u8>, pin: Option<Pin>) -> Result<(String, Vec<u8>)> {
    let base = validate_outbound_http_url(endpoint, true).map_err(|e| OcrError::Parse(format!("Invalid remote OCR endpoint: {e}")))?;
    run_azure(base, endpoint, api_key, document, true, pin)
}

fn run_azure(base: reqwest::Url, endpoint: &str, api_key: &str, document: Vec<u8>, allow_internal: bool, pin: Option<Pin>) -> Result<(String, Vec<u8>)> {
    let endpoint = endpoint.trim_end_matches('/').to_string();
    let key = api_key.to_string();
    let handle = std::thread::Builder::new()
        .name("remote-ocr".into())
        .spawn(move || -> std::result::Result<(String, Vec<u8>), String> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(azure_flow(base, endpoint, key, document, allow_internal, pin))
        })
        .map_err(|e| OcrError::Parse(format!("Azure AI Vision parsing failed: {e}")))?;
    let res = handle.join().map_err(|_| OcrError::Parse("Azure AI Vision parsing failed: worker thread panicked".into()))?;
    res.map_err(|e| {
        tracing::error!("Azure AI Vision parsing failed: {e}");
        OcrError::Parse(format!("Azure AI Vision parsing failed: {e}"))
    })
}

fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme() && a.host_str().map(str::to_ascii_lowercase) == b.host_str().map(str::to_ascii_lowercase) && a.port_or_known_default() == b.port_or_known_default()
}

/// Error text from an Azure error response (`{"error": {"code", "message"}}`), truncated like
/// `HttpResponseError.__str__`: `(Code) Message`.
fn azure_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let e = &v["error"];
    match (e["code"].as_str(), e["message"].as_str()) {
        (Some(c), Some(m)) => format!("({c}) {m}"),
        (None, Some(m)) => m.to_string(),
        _ => format!("Operation returned an invalid status '{}'", status.canonical_reason().unwrap_or(status.as_str())),
    }
}

/// Reads the response body up to [`MAX_RESPONSE_BYTES`] at most (`Content-Length` upfront, then
/// chunk by chunk; security review 2026-11-19 F10).
async fn read_limited(mut resp: reqwest::Response) -> std::result::Result<Vec<u8>, String> {
    let too_large = || format!("response from Azure exceeds the limit of {} MiB", MAX_RESPONSE_BYTES / (1024 * 1024));
    if resp.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
        return Err(too_large());
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
        if out.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(too_large());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Checks a follow-up URL (poll URL, redirect); the same origin is additionally required by the
/// caller. With a pinned address the connection goes only there anyway (no new
/// DNS resolution needed), otherwise checked again like upstream.
fn check_follow_up(url: &str, pinned: bool, allow_internal: bool) -> std::result::Result<reqwest::Url, String> {
    if pinned {
        let u = reqwest::Url::parse(url).map_err(|_| "Invalid URL scheme or hostname.".to_string())?;
        if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none_or(str::is_empty) {
            return Err("Invalid URL scheme or hostname.".into());
        }
        Ok(u)
    } else {
        validate_outbound_http_url(url, allow_internal)
    }
}

async fn azure_flow(base: reqwest::Url, endpoint: String, key: String, document: Vec<u8>, allow_internal: bool, pin: Option<Pin>) -> std::result::Result<(String, Vec<u8>), String> {
    use base64::Engine as _;
    let origin = base.clone();
    let pinned = pin.is_some();
    // Redirects only within the same host (the key stays there) and checked again.
    let policy_origin = origin.clone();
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        if !same_origin(attempt.url(), &policy_origin) {
            return attempt.error("redirect to another host refused");
        }
        match check_follow_up(attempt.url().as_str(), pinned, allow_internal) {
            Ok(_) => attempt.follow(),
            Err(e) => attempt.error(e),
        }
    });
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .redirect(policy)
        .timeout(Duration::from_secs(300))
        // Security review 2026-11-19 F10: no proxy from HTTP(S)_PROXY (it would resolve itself).
        .no_proxy();
    if let Some((host, addr)) = &pin {
        // connect only to the checked address (TLS/SNI still on the host name)
        builder = builder.resolve(host, *addr);
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let analyze = format!(
        "{endpoint}/documentintelligence/documentModels/{MODEL_ID}:analyze?api-version={API_VERSION}&outputContentFormat=text&output=pdf"
    );
    let body = serde_json::json!({ "base64Source": base64::engine::general_purpose::STANDARD.encode(&document) });
    let resp = client
        .post(&analyze)
        .header("Ocp-Apim-Subscription-Key", &key)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    let op = resp.headers().get("operation-location").and_then(|v| v.to_str().ok()).map(str::to_string);
    if status.as_u16() != 202 {
        let b = read_limited(resp).await?;
        return Err(azure_error(status, &b));
    }
    let op = op.ok_or("missing Operation-Location header")?;
    let op_url = check_follow_up(&op, pinned, allow_internal).map_err(|e| format!("Invalid operation location: {e}"))?;
    if !same_origin(&op_url, &origin) {
        return Err("operation location on another host refused".into());
    }
    // resultId = last path segment of …/analyzeResults/{resultId}
    let result_id = op_url
        .path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .filter(|s| !s.is_empty())
        .ok_or("operation location without result id")?;
    let started = std::time::Instant::now();
    let content = loop {
        let r = client.get(op_url.clone()).header("Ocp-Apim-Subscription-Key", &key).send().await.map_err(|e| e.without_url().to_string())?;
        let status = r.status();
        let retry = r
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|s| Duration::from_secs(s.clamp(1, 60)))
            .unwrap_or(Duration::from_secs(1));
        let b = read_limited(r).await?;
        if !status.is_success() {
            return Err(azure_error(status, &b));
        }
        let v: serde_json::Value = serde_json::from_slice(&b).map_err(|e| format!("invalid JSON from Azure: {e}"))?;
        match v["status"].as_str() {
            Some("succeeded") => break v["analyzeResult"]["content"].as_str().unwrap_or_default().to_string(),
            Some("failed") | Some("canceled") => return Err(azure_error(status, &b)),
            _ => {}
        }
        if started.elapsed() > MAX_POLL {
            return Err("analysis did not finish in time".into());
        }
        tokio::time::sleep(retry).await;
    };
    let pdf_url = format!("{endpoint}/documentintelligence/documentModels/{MODEL_ID}/analyzeResults/{result_id}/pdf?api-version={API_VERSION}");
    let r = client.get(&pdf_url).header("Ocp-Apim-Subscription-Key", &key).send().await.map_err(|e| e.without_url().to_string())?;
    let status = r.status();
    let b = read_limited(r).await?;
    if !status.is_success() {
        return Err(azure_error(status, &b));
    }
    Ok((content, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_validation() {
        assert_eq!(validate_outbound_http_url("ftp://x.example", true).unwrap_err(), "Invalid URL scheme or hostname.");
        assert_eq!(validate_outbound_http_url("http:///x", true).unwrap_err(), "Invalid URL scheme or hostname.");
        assert!(validate_outbound_http_url("http://127.0.0.1:9/", true).is_ok());
        assert_eq!(
            validate_outbound_http_url("http://127.0.0.1:9/", false).unwrap_err(),
            "Connection blocked: 127.0.0.1 resolves to a non-public address"
        );
        assert!(validate_outbound_http_url("http://[::1]/", false).is_err());
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_public_ip("100.64.1.1".parse().unwrap()));
        assert!(!is_public_ip("::ffff:10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn error_texts() {
        let b = br#"{"error":{"code":"InvalidRequest","message":"Invalid request."}}"#;
        assert_eq!(azure_error(reqwest::StatusCode::BAD_REQUEST, b), "(InvalidRequest) Invalid request.");
        assert!(azure_error(reqwest::StatusCode::UNAUTHORIZED, b"").contains("Unauthorized"));
    }
}
