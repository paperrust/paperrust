//! Response finishing for Rust-handled requests, mirroring upstream's Django middleware
//! stack (responses pass the middleware in reverse order of `MIDDLEWARE`).
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (MIDDLEWARE, X_FRAME_OPTIONS, CORS_*)
//! upstream: src/paperless/middleware.py@v3.4.2 (ApiVersionMiddleware, StreamAwareCompressionMiddleware)
//! upstream: django@5.2.18 middleware/security.py, middleware/clickjacking.py, middleware/locale.py,
//!           contrib/sessions/middleware.py, utils/cache.py::patch_vary_headers,
//!           http/request.py::get_host/split_domain_port/validate_host
//! upstream: django-cors-headers@4.9.0 corsheaders/middleware.py::CorsMiddleware

use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, header};
use axum::response::Response;
use regex::Regex;

use crate::config::DjangoSettings;
use crate::locale;

/// Latest API version and version string set by `ApiVersionMiddleware` for
/// authenticated users. upstream: settings REST_FRAMEWORK["ALLOWED_VERSIONS"][-1],
/// paperless/version.py::__full_version_str__
pub const API_VERSION_LATEST: &str = "10";
/// `X-Version` stays the emulated paperless-ngx version, not PaperRust's own: the Angular UI does
/// not read it (it compiles its version in and asks `/api/remote_version/`), but third-party
/// clients (mobile apps, scripts) use it to detect server features. PaperRust's own version is in
/// `/__rs/health` and [`pl_core::brand::PAPERRUST_VERSION`].
pub const PAPERLESS_VERSION: &str = pl_core::brand::UPSTREAM_VERSION;

/// Response extension "the request was authenticated" (shared type from pl-core; pl-authz
/// also reports it through [`pl_core::RequestNotes`], see `frontline`).
pub use pl_core::Authenticated;

/// Response extension: like `@xframe_options_exempt` (skip X-Frame-Options).
#[derive(Debug, Clone, Copy)]
pub struct XFrameOptionsExempt;

/// Response extension: answered by WhiteNoise (position 2 of MIDDLEWARE) – only
/// `SecurityMiddleware` (and compression) wrap it.
#[derive(Debug, Clone, Copy)]
pub struct SecurityOnly;

/// Response extension: CORS preflight answered by `CorsMiddleware` itself (Django off):
/// CORS headers + `SecurityMiddleware`, nothing from the inner middlewares.
#[derive(Debug, Clone, Copy)]
pub struct PreflightOnly;

/// What the finishing step needs from the request (captured before the handler
/// consumes it).
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub method: Method,
    pub path: String,
    pub origin: Option<String>,
    pub accept_encoding: String,
    pub language: String,
}

/// Header values as Django sees them in `request.META`/`request.headers`
/// (latin-1 decoded; duplicates joined with ",").
pub fn header_str(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    let mut out: Option<String> = None;
    for v in headers.get_all(name) {
        let s: String = v.as_bytes().iter().map(|&b| b as char).collect();
        match &mut out {
            None => out = Some(s),
            Some(o) => {
                o.push(',');
                o.push_str(&s);
            }
        }
    }
    out
}

impl RequestInfo {
    pub fn capture(method: &Method, path: &str, headers: &HeaderMap, settings: &DjangoSettings) -> Self {
        let cookie = headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join("; ");
        let lang_cookie = locale::cookie_value(&cookie, &settings.language_cookie_name);
        let accept_language = header_str(headers, &header::ACCEPT_LANGUAGE).unwrap_or_default();
        Self {
            method: method.clone(),
            path: path.to_owned(),
            origin: header_str(headers, &header::ORIGIN),
            accept_encoding: header_str(headers, &header::ACCEPT_ENCODING).unwrap_or_default(),
            language: locale::language_from_request(lang_cookie.as_deref(), &accept_language),
        }
    }
}

#[allow(clippy::expect_used)]
static CC_DELIM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*,\s*").expect("static regex"));

/// `django.utils.cache.patch_vary_headers`
pub fn patch_vary_headers(headers: &mut HeaderMap, new: &[&str]) {
    let existing = header_str(headers, &header::VARY);
    let mut vary: Vec<String> = match &existing {
        Some(v) => CC_DELIM_RE.split(v).map(str::to_owned).collect(),
        None => Vec::new(),
    };
    let lower: Vec<String> = vary.iter().map(|h| h.to_lowercase()).collect();
    for h in new {
        if !lower.contains(&h.to_lowercase()) {
            vary.push((*h).to_owned());
        }
    }
    let value = if vary.iter().any(|h| h == "*") { "*".to_owned() } else { vary.join(", ") };
    if let Ok(v) = HeaderValue::from_str(&value) {
        headers.insert(header::VARY, v);
    }
}

fn set_default(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    let name = HeaderName::from_static(name);
    if !headers.contains_key(&name) {
        headers.insert(name, HeaderValue::from_static(value));
    }
}

/// Python `urllib.parse.urlsplit(origin)` -> (scheme, netloc); `None` = ValueError.
fn urlsplit_scheme_netloc(url: &str) -> Option<(String, String)> {
    let url: String = url.trim_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let (scheme, rest) = match url.find(':') {
        Some(i)
            if i > 0
                && url[..i].starts_with(|c: char| c.is_ascii_alphabetic())
                && url[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            (url[..i].to_lowercase(), url[i + 1..].to_owned())
        }
        _ => (String::new(), url.clone()),
    };
    let netloc = match rest.strip_prefix("//") {
        Some(r) => r.split(['/', '?', '#']).next().unwrap_or("").to_owned(),
        None => String::new(),
    };
    if netloc.contains('[') != netloc.contains(']') {
        return None;
    }
    Some((scheme, netloc))
}

/// `CorsMiddleware.add_response_headers` (CORS_URLS_REGEX default matches every path,
/// no CORS_ALLOWED_ORIGIN_REGEXES, CORS_ALLOW_ALL_ORIGINS False).
fn add_cors_headers(headers: &mut HeaderMap, info: &RequestInfo, s: &DjangoSettings) {
    patch_vary_headers(headers, &["origin"]);
    let Some(origin) = info.origin.as_deref().filter(|o| !o.is_empty()) else {
        return;
    };
    let Some((scheme, netloc)) = urlsplit_scheme_netloc(origin) else {
        return;
    };
    let allowed = (origin == "null" && s.cors_allowed_origins.iter().any(|o| o == "null"))
        || s.cors_allowed_origins.iter().any(|o| urlsplit_scheme_netloc(o).is_some_and(|(sc, nl)| sc == scheme && nl == netloc));
    if !allowed {
        return;
    }
    let Ok(origin_value) = HeaderValue::from_str(origin) else {
        return;
    };
    headers.insert(HeaderName::from_static("access-control-allow-origin"), origin_value);
    if s.cors_allow_credentials {
        headers.insert(HeaderName::from_static("access-control-allow-credentials"), HeaderValue::from_static("true"));
    }
    if !s.cors_expose_headers.is_empty()
        && let Ok(v) = HeaderValue::from_str(&s.cors_expose_headers.join(", "))
    {
        headers.insert(HeaderName::from_static("access-control-expose-headers"), v);
    }
    if info.method == Method::OPTIONS {
        // corsheaders.defaults: CORS_ALLOW_HEADERS / CORS_ALLOW_METHODS
        headers.insert(
            HeaderName::from_static("access-control-allow-headers"),
            HeaderValue::from_static("accept, authorization, content-type, user-agent, x-csrftoken, x-requested-with"),
        );
        headers.insert(
            HeaderName::from_static("access-control-allow-methods"),
            HeaderValue::from_static("DELETE, GET, OPTIONS, PATCH, POST, PUT"),
        );
        headers.insert(HeaderName::from_static("access-control-max-age"), HeaderValue::from_static("86400"));
    }
}

/// Apply the header-only middleware effects to a Rust response (compression is separate).
pub fn finish_headers(resp: &mut Response, info: &RequestInfo, s: &DjangoSettings) {
    if resp.extensions().get::<SecurityOnly>().is_some() {
        security_headers(resp.headers_mut());
        return;
    }
    if resp.extensions().get::<PreflightOnly>().is_some() {
        add_cors_headers(resp.headers_mut(), info, s);
        security_headers(resp.headers_mut());
        return;
    }
    let xframe_exempt = resp.extensions().get::<XFrameOptionsExempt>().is_some();
    let authenticated = resp.extensions().get::<Authenticated>().is_some();
    let h = resp.headers_mut();

    // XFrameOptionsMiddleware (X_FRAME_OPTIONS = "SAMEORIGIN")
    if !xframe_exempt {
        set_default(h, "x-frame-options", "SAMEORIGIN");
    }
    // ApiVersionMiddleware
    if authenticated {
        h.insert(HeaderName::from_static("x-api-version"), HeaderValue::from_static(API_VERSION_LATEST));
        h.insert(HeaderName::from_static("x-version"), HeaderValue::from_static(PAPERLESS_VERSION));
    }
    // LocaleMiddleware
    patch_vary_headers(h, &["Accept-Language"]);
    if !h.contains_key(header::CONTENT_LANGUAGE)
        && let Ok(v) = HeaderValue::from_str(&info.language)
    {
        h.insert(header::CONTENT_LANGUAGE, v);
    }
    // CorsMiddleware
    add_cors_headers(h, info, s);
    // SessionMiddleware: the reference shows `Vary: Cookie` on every response, also with
    // Token/Basic auth and on 401 (session is always accessed), so always add it.
    patch_vary_headers(h, &["Cookie"]);
    security_headers(h);
}

/// SecurityMiddleware (defaults: nosniff, same-origin, same-origin; no HSTS)
fn security_headers(h: &mut HeaderMap) {
    set_default(h, "x-content-type-options", "nosniff");
    set_default(h, "referrer-policy", "same-origin");
    set_default(h, "cross-origin-opener-policy", "same-origin");
}

#[allow(clippy::expect_used)]
static HOST_VALIDATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z0-9.-]+|\[[a-f0-9]*:[a-f0-9.:]+\])(?::([0-9]+))?$").expect("static regex"));

/// `split_domain_port` -> lowercased domain without trailing dot; "" if invalid.
pub fn split_domain(host: &str) -> String {
    let lower = host.to_lowercase();
    match HOST_VALIDATION_RE.captures(&lower) {
        Some(c) => c.get(1).map(|m| m.as_str().strip_suffix('.').unwrap_or(m.as_str()).to_owned()).unwrap_or_default(),
        None => String::new(),
    }
}

/// `is_same_domain`
fn is_same_domain(host: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let p = pattern.to_lowercase();
    (p.starts_with('.') && (host.ends_with(&p) || host == &p[1..])) || p == host
}

/// `request.get_host()` succeeds (no `DisallowedHost`)? `None` host (no Host header,
/// Django would fall back to SERVER_NAME) counts as "let Django decide".
pub fn host_allowed(headers: &HeaderMap, s: &DjangoSettings) -> bool {
    let raw = if s.use_x_forwarded_host && headers.contains_key("x-forwarded-host") {
        header_str(headers, &HeaderName::from_static("x-forwarded-host"))
    } else {
        header_str(headers, &header::HOST)
    };
    let Some(raw) = raw else { return false };
    let mut allowed: Vec<String> = s.allowed_hosts.clone();
    if s.debug && allowed.is_empty() {
        allowed = vec![".localhost".into(), "127.0.0.1".into(), "[::1]".into()];
    }
    let domain = split_domain(&raw);
    !domain.is_empty() && allowed.iter().any(|p| p == "*" || is_same_domain(&domain, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vary_patching() {
        let mut h = HeaderMap::new();
        h.insert(header::VARY, HeaderValue::from_static("Accept , cookie"));
        patch_vary_headers(&mut h, &["Cookie", "origin", "Accept-Language"]);
        assert_eq!(h[header::VARY], "Accept, cookie, origin, Accept-Language");
        let mut h = HeaderMap::new();
        h.insert(header::VARY, HeaderValue::from_static("*"));
        patch_vary_headers(&mut h, &["Cookie"]);
        assert_eq!(h[header::VARY], "*");
    }

    #[test]
    fn host_validation() {
        let mut s = DjangoSettings::default();
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("Paper.example:8000"));
        assert!(host_allowed(&h, &s));
        s.allowed_hosts = vec![".example".into(), "localhost".into()];
        assert!(host_allowed(&h, &s));
        s.allowed_hosts = vec!["other".into()];
        assert!(!host_allowed(&h, &s));
        h.insert(header::HOST, HeaderValue::from_static("bad host"));
        s.allowed_hosts = vec!["*".into()];
        assert!(!host_allowed(&h, &s));
        assert_eq!(split_domain("[::1]:80"), "[::1]");
        assert_eq!(split_domain("example.com."), "example.com");
    }

    #[test]
    fn urlsplit() {
        assert_eq!(urlsplit_scheme_netloc("HTTP://localhost:8000/x"), Some(("http".into(), "localhost:8000".into())));
        assert_eq!(urlsplit_scheme_netloc("http://[::1"), None);
    }
}
