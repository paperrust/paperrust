//! Content negotiation and API versioning like DRF.
//!
//! upstream: rest_framework/negotiation.py@3.18.3 (`DefaultContentNegotiation.select_renderer`),
//! rest_framework/utils/mediatypes.py (`_MediaType`, `order_parsed_by_precedence`),
//! rest_framework/versioning.py (`AcceptHeaderVersioning`), rest_framework/renderers.py
//! (`JSONRenderer.get_indent`), django/utils/http.py@5.2 (`parse_header_parameters`),
//! src/paperless/settings/__init__.py (`DEFAULT_VERSION="10"`, `ALLOWED_VERSIONS=["9","10"]`,
//! renderers = DRF default: JSONRenderer, BrowsableAPIRenderer).
//!
//! Flow in DRF `APIView.initial()`: first negotiation (406/404), then version (406), then
//! authentication – therefore use [`crate::RequestContext`] as the first extractor.

use axum::http::HeaderMap;
use axum::http::header::ACCEPT;

use crate::{ApiError, QueryParams};

/// Allowed versions (`ALLOWED_VERSIONS`), newest last.
pub const ALLOWED_VERSIONS: [&str; 2] = ["9", "10"];
/// `DEFAULT_VERSION`.
pub const DEFAULT_VERSION: &str = "10";

/// Negotiated API version (`request.version`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiVersion(u16);

impl ApiVersion {
    /// Version 9 (with `all` in lists).
    pub const V9: ApiVersion = ApiVersion(9);
    /// Version 10 (default).
    pub const V10: ApiVersion = ApiVersion(10);
    /// Newest version (for `X-Api-Version`).
    pub const LATEST: ApiVersion = ApiVersion(10);

    /// Numeric value (`int(request.version)`).
    pub fn as_u16(self) -> u16 {
        self.0
    }

    /// `StandardPagination._should_include_all`: `version < 10`.
    pub fn includes_all(self) -> bool {
        self.0 < 10
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        ApiVersion::V10
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Selected renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// `JSONRenderer` (`application/json`, format `json`).
    Json,
    /// `BrowsableAPIRenderer` (`text/html`, format `api`) – Rust does **not** render this HTML page;
    /// `pl-server` should hand such requests to Django (see [`prefers_browsable_api`]).
    BrowsableApi,
}

/// Result of the negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiation {
    pub renderer: Renderer,
    /// `request.accepted_media_type`.
    pub accepted_media_type: String,
    pub version: ApiVersion,
    /// `indent` parameter (JSON only, 1..=8; 0 → no indentation).
    pub indent: Option<usize>,
}

impl Default for Negotiation {
    fn default() -> Self {
        Self {
            renderer: Renderer::Json,
            accepted_media_type: "application/json".into(),
            version: ApiVersion::V10,
            indent: None,
        }
    }
}

/// Parsed media type like DRF `_MediaType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaType {
    pub orig: String,
    pub main_type: String,
    pub sub_type: String,
    /// Parameters in order (names lowercased).
    pub params: Vec<(String, String)>,
}

impl MediaType {
    /// Parses like `_MediaType(str)` (Django `parse_header_parameters`, email parameter rules).
    pub fn parse(s: &str) -> Self {
        let (full, params) = parse_header_parameters(s);
        let (main, sub) = match full.split_once('/') {
            Some((m, s)) => (m.to_string(), s.to_string()),
            None => (full.clone(), String::new()),
        };
        Self { orig: s.to_string(), main_type: main, sub_type: sub, params }
    }

    /// Parameter value.
    pub fn param(&self, k: &str) -> Option<&str> {
        // Python dict: a later identical key overwrites.
        self.params.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// `_MediaType.precedence`.
    pub fn precedence(&self) -> u8 {
        if self.main_type == "*" {
            0
        } else if self.sub_type == "*" {
            1
        } else if self.params.is_empty() || self.params.iter().all(|(k, _)| k == "q") {
            2
        } else {
            3
        }
    }

    /// `_MediaType.match(other)` (self = Renderer-Typ).
    pub fn matches(&self, other: &MediaType) -> bool {
        for (k, v) in &self.params {
            if k != "q" && other.param(k) != Some(v.as_str()) {
                return false;
            }
        }
        if self.sub_type != "*" && other.sub_type != "*" && other.sub_type != self.sub_type {
            return false;
        }
        if self.main_type != "*" && other.main_type != "*" && other.main_type != self.main_type {
            return false;
        }
        true
    }
}

/// Splits at `;` outside quotes (email `_parseparam`), lowercases names, trims.
fn parseparam(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut s = format!(";{line}");
    while s.starts_with(';') {
        s.remove(0);
        let bytes = s.as_bytes();
        let mut end = s.find(';').map(|i| i as isize).unwrap_or(-1);
        while end > 0 {
            let e = end as usize;
            let quotes = bytes[..e].iter().filter(|b| **b == b'"').count();
            let escaped = s[..e].matches("\\\"").count();
            if (quotes - escaped) % 2 == 1 {
                end = s[e + 1..].find(';').map(|i| (i + e + 1) as isize).unwrap_or(-1);
            } else {
                break;
            }
        }
        let end = if end < 0 { s.len() } else { end as usize };
        let mut f = s[..end].to_string();
        if let Some(i) = f.find('=') {
            f = format!("{}={}", f[..i].trim().to_lowercase(), f[i + 1..].trim());
        }
        out.push(f.trim().to_string());
        s = s[end..].to_string();
    }
    out
}

fn unquote_value(v: &str) -> String {
    if v.len() > 1 && v.starts_with('"') && v.ends_with('"') {
        return v[1..v.len() - 1].replace("\\\\", "\\").replace("\\\"", "\"");
    }
    if v.len() > 1 && v.starts_with('<') && v.ends_with('>') {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

/// Django 5.2 `parse_header_parameters(line)` → (main type lowercased, parameters).
pub fn parse_header_parameters(line: &str) -> (String, Vec<(String, String)>) {
    if line.is_empty() {
        return (String::new(), Vec::new());
    }
    if !line.contains(';') {
        return (line.trim().to_lowercase(), Vec::new());
    }
    let mut parts = parseparam(line).into_iter();
    let key = parts.next().unwrap_or_default();
    let key = match key.split_once('=') {
        Some((n, _)) => n.trim().to_lowercase(),
        None => key.trim().to_lowercase(),
    };
    let mut params = Vec::new();
    for p in parts {
        let (name, val) = match p.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => (p.trim().to_string(), String::new()),
        };
        if name.is_empty() {
            continue;
        }
        params.push((name, unquote_value(&val)));
    }
    (key, params)
}

/// DRF `DefaultContentNegotiation.get_accept_list`.
fn accept_list(headers: &HeaderMap) -> Vec<String> {
    let values: Vec<&str> = headers.get_all(ACCEPT).iter().filter_map(|v| v.to_str().ok()).collect();
    let mut header = if values.is_empty() { "*/*".to_string() } else { values.join(",") };
    if header.len() > 8192 {
        header = match header[..8192.min(header.len())].rfind(',') {
            Some(cut) => header[..=cut].to_string(),
            None => String::new(),
        };
    }
    let mut tokens = Vec::new();
    for t in header.split(',') {
        let t = t.trim();
        if t.is_empty() || t.len() > 256 {
            continue;
        }
        tokens.push(t.to_string());
        if tokens.len() >= 64 {
            break;
        }
    }
    tokens
}

/// DRF negotiation + `AcceptHeaderVersioning` + `JSONRenderer.get_indent`.
///
/// Errors: unknown `?format=` → 404 `Not found.`; no renderer matches → 406
/// `Could not satisfy the request Accept header.`; version not allowed → 406
/// `Invalid version in "Accept" header.`.
pub fn negotiate(headers: &HeaderMap, query: &QueryParams) -> Result<Negotiation, ApiError> {
    let mut renderers: Vec<(Renderer, MediaType)> = vec![
        (Renderer::Json, MediaType::parse("application/json")),
        (Renderer::BrowsableApi, MediaType::parse("text/html")),
    ];
    if let Some(fmt) = query.get("format").filter(|f| !f.is_empty()) {
        renderers.retain(|(r, _)| match r {
            Renderer::Json => fmt == "json",
            Renderer::BrowsableApi => fmt == "api",
        });
        if renderers.is_empty() {
            return Err(ApiError::not_found());
        }
    }
    let accepts: Vec<MediaType> = accept_list(headers).iter().map(|t| MediaType::parse(t)).collect();
    let mut selected: Option<(Renderer, String)> = None;
    'outer: for prec in [3u8, 2, 1, 0] {
        let bucket: Vec<&MediaType> = {
            let mut seen: Vec<&str> = Vec::new();
            accepts
                .iter()
                .filter(|m| m.precedence() == prec)
                .filter(|m| {
                    if seen.contains(&m.orig.as_str()) {
                        false
                    } else {
                        seen.push(&m.orig);
                        true
                    }
                })
                .collect()
        };
        if bucket.is_empty() {
            continue;
        }
        for (r, rmt) in &renderers {
            for mt in &bucket {
                if rmt.matches(mt) {
                    let accepted = if rmt.precedence() > mt.precedence() {
                        let base = match r {
                            Renderer::Json => "application/json",
                            Renderer::BrowsableApi => "text/html",
                        };
                        let mut s = base.to_string();
                        for (k, v) in &mt.params {
                            s.push_str(&format!(";{k}={v}"));
                        }
                        s
                    } else {
                        mt.orig.clone()
                    };
                    selected = Some((*r, accepted));
                    break 'outer;
                }
            }
        }
    }
    let Some((renderer, accepted_media_type)) = selected else {
        return Err(ApiError::not_acceptable());
    };
    let amt = MediaType::parse(&accepted_media_type);
    let version_str = amt.param("version").unwrap_or(DEFAULT_VERSION);
    let version = if version_str == DEFAULT_VERSION || ALLOWED_VERSIONS.contains(&version_str) {
        ApiVersion(version_str.parse().map_err(|_| ApiError::invalid_version())?)
    } else {
        return Err(ApiError::invalid_version());
    };
    let indent = if renderer == Renderer::Json {
        amt.param("indent")
            .and_then(crate::pagination::python_int)
            .map(|i| i.clamp(0, 8) as usize)
            .filter(|i| *i != 0)
    } else {
        None
    };
    Ok(Negotiation { renderer, accepted_media_type, version, indent })
}

/// `true` if DRF would render the browsable API HTML page for this request
/// (e.g. browser navigation with `Accept: text/html,…`). `pl-server` can pass such requests
/// on to Django so that the HTML page stays identical.
pub fn prefers_browsable_api(headers: &HeaderMap, query: &QueryParams) -> bool {
    matches!(negotiate(headers, query), Ok(Negotiation { renderer: Renderer::BrowsableApi, .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn neg(accept: Option<&str>, q: &str) -> Result<Negotiation, ApiError> {
        let mut h = HeaderMap::new();
        if let Some(a) = accept {
            h.insert(ACCEPT, HeaderValue::from_str(a).unwrap());
        }
        negotiate(&h, &QueryParams::parse(q))
    }

    #[test]
    fn default_version() {
        let n = neg(None, "").unwrap();
        assert_eq!(n.version, ApiVersion::V10);
        assert_eq!(n.renderer, Renderer::Json);
        assert_eq!(n.accepted_media_type, "application/json");
    }

    #[test]
    fn angular_header() {
        let n = neg(Some("application/json; version=9"), "").unwrap();
        assert_eq!(n.version, ApiVersion::V9);
        assert!(n.version.includes_all());
        let n = neg(Some("application/json, text/plain, */*; version=10"), "").unwrap();
        assert_eq!(n.version, ApiVersion::V10);
        let n = neg(Some("*/*; version=9"), "").unwrap();
        assert_eq!(n.version, ApiVersion::V9);
        assert_eq!(n.accepted_media_type, "application/json;version=9");
        let n = neg(Some("application/json; Version=\"9\""), "").unwrap();
        assert_eq!(n.version, ApiVersion::V9);
    }

    #[test]
    fn invalid_version_406() {
        for a in ["application/json; version=8", "application/json; version=", "application/json; version=09"] {
            let e = neg(Some(a), "").unwrap_err();
            assert_eq!(e.status().as_u16(), 406);
            assert_eq!(e.body_json().unwrap()["detail"], "Invalid version in \"Accept\" header.");
        }
    }

    #[test]
    fn not_acceptable_and_format() {
        let e = neg(Some("application/xml"), "").unwrap_err();
        assert_eq!(e.body_json().unwrap()["detail"], "Could not satisfy the request Accept header.");
        assert_eq!(neg(Some(""), "").unwrap_err().status().as_u16(), 406);
        assert_eq!(neg(None, "format=xml").unwrap_err().status().as_u16(), 404);
        assert_eq!(neg(Some("text/html"), "format=json").unwrap_err().status().as_u16(), 406);
        assert_eq!(neg(Some("text/html"), "format=api").unwrap().renderer, Renderer::BrowsableApi);
    }

    #[test]
    fn browser_gets_html() {
        let a = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
        assert_eq!(neg(Some(a), "").unwrap().renderer, Renderer::BrowsableApi);
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_static("text/html"));
        assert!(prefers_browsable_api(&h, &QueryParams::default()));
    }

    #[test]
    fn indent() {
        assert_eq!(neg(Some("application/json; indent=4"), "").unwrap().indent, Some(4));
        assert_eq!(neg(Some("application/json; indent=40"), "").unwrap().indent, Some(8));
        assert_eq!(neg(Some("application/json; indent=0"), "").unwrap().indent, None);
    }
}
