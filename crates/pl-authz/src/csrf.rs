//! CSRF check exactly like DRF `SessionAuthentication.enforce_csrf` → Django `CsrfViewMiddleware`
//! (`process_request` + `process_view`), plus the `HttpRequest` helpers it relies on.
//!
//! upstream: rest_framework/authentication.py@3.16 (SessionAuthentication.enforce_csrf, CSRFCheck),
//!           django/middleware/csrf.py@5.2 (CsrfViewMiddleware, _unmask_cipher_token,
//!           _check_token_format, _does_token_match, REASON_*),
//!           django/http/request.py@5.2 (get_host, _get_raw_host, is_secure/scheme,
//!           split_domain_port, validate_host, parse_cookie via django/http/cookie.py),
//!           django/utils/http.py@5.2 (is_same_domain), urllib.parse.urlsplit (CPython 3.12).

use crate::config::AuthConfig;
use crate::signing::constant_time_eq;
use axum::http::HeaderMap;

pub const CSRF_SECRET_LENGTH: usize = 32;
pub const CSRF_TOKEN_LENGTH: usize = 2 * CSRF_SECRET_LENGTH;
/// `CSRF_ALLOWED_CHARS = string.ascii_letters + string.digits` (order matters for unmasking).
pub const CSRF_ALLOWED_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub const REASON_BAD_ORIGIN: &str = "Origin checking failed - %s does not match any trusted origins.";
pub const REASON_NO_REFERER: &str = "Referer checking failed - no Referer.";
pub const REASON_BAD_REFERER: &str = "Referer checking failed - %s does not match any trusted origins.";
pub const REASON_NO_CSRF_COOKIE: &str = "CSRF cookie not set.";
pub const REASON_CSRF_TOKEN_MISSING: &str = "CSRF token missing.";
pub const REASON_MALFORMED_REFERER: &str = "Referer checking failed - Referer is malformed.";
pub const REASON_INSECURE_REFERER: &str = "Referer checking failed - Referer is insecure while host is secure.";
pub const REASON_INCORRECT_LENGTH: &str = "has incorrect length";
pub const REASON_INVALID_CHARACTERS: &str = "has invalid characters";

/// Django's `settings.CSRF_HEADER_NAME` default (`HTTP_X_CSRFTOKEN`), as shown in messages by
/// `HttpHeaders.parse_header_name`: `'X-Csrftoken'`.
const CSRF_HEADER_DISPLAY: &str = "X-Csrftoken";

fn char_index(c: u8) -> Option<usize> {
    CSRF_ALLOWED_CHARS.iter().position(|&x| x == c)
}

/// `_unmask_cipher_token(token)` for a 64-char token of allowed characters.
pub fn unmask_cipher_token(token: &str) -> Option<String> {
    let b = token.as_bytes();
    if b.len() != CSRF_TOKEN_LENGTH {
        return None;
    }
    let (mask, cipher) = b.split_at(CSRF_SECRET_LENGTH);
    let n = CSRF_ALLOWED_CHARS.len() as isize;
    let mut out = String::with_capacity(CSRF_SECRET_LENGTH);
    for (x, y) in cipher.iter().zip(mask) {
        let xi = char_index(*x)? as isize;
        let yi = char_index(*y)? as isize;
        // Python chars[x - y] with negative indexing == (x - y) mod n
        out.push(CSRF_ALLOWED_CHARS[(xi - yi).rem_euclid(n) as usize] as char);
    }
    Some(out)
}

/// `_mask_cipher_secret(secret)` with a random mask (for `get_token`/rotation).
pub fn mask_cipher_secret(secret: &str) -> String {
    let mask = crate::hashers::get_random_string(CSRF_SECRET_LENGTH);
    mask_with(secret, &mask)
}

fn mask_with(secret: &str, mask: &str) -> String {
    let n = CSRF_ALLOWED_CHARS.len();
    let cipher: String = secret
        .bytes()
        .zip(mask.bytes())
        .map(|(x, y)| {
            let xi = char_index(x).unwrap_or(0);
            let yi = char_index(y).unwrap_or(0);
            CSRF_ALLOWED_CHARS[(xi + yi) % n] as char
        })
        .collect();
    format!("{mask}{cipher}")
}

/// `_check_token_format(token)`: Err(reason) = `InvalidTokenFormat(reason)`.
pub fn check_token_format(token: &str) -> Result<(), &'static str> {
    let len = token.chars().count();
    if len != CSRF_TOKEN_LENGTH && len != CSRF_SECRET_LENGTH {
        return Err(REASON_INCORRECT_LENGTH);
    }
    if !token.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err(REASON_INVALID_CHARACTERS);
    }
    Ok(())
}

/// `_does_token_match(request_csrf_token, csrf_secret)` (token format already checked).
pub fn does_token_match(request_token: &str, secret: &str) -> bool {
    let unmasked;
    let tok = if request_token.len() == CSRF_TOKEN_LENGTH {
        match unmask_cipher_token(request_token) {
            Some(u) => {
                unmasked = u;
                unmasked.as_str()
            }
            None => return false,
        }
    } else {
        request_token
    };
    constant_time_eq(tok.as_bytes(), secret.as_bytes())
}

/// `django.http.cookie.parse_cookie`: last occurrence wins, values unquoted.
pub fn parse_cookies(headers: &HeaderMap) -> Vec<(String, String)> {
    // ASGI joins repeated Cookie headers with "; " (django/core/handlers/asgi.py).
    let mut joined = String::new();
    for v in headers.get_all(axum::http::header::COOKIE) {
        if !joined.is_empty() {
            joined.push_str("; ");
        }
        joined.push_str(&latin1(v.as_bytes()));
    }
    parse_cookie_str(&joined)
}

/// Parse a Cookie header string like `django.http.cookie.parse_cookie`.
pub fn parse_cookie_str(cookie: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for chunk in cookie.split(';') {
        let (k, v) = match chunk.split_once('=') {
            Some((k, v)) => (k, v),
            None => ("", chunk),
        };
        let (k, v) = (k.trim(), v.trim());
        if !k.is_empty() || !v.is_empty() {
            let v = unquote_cookie(v);
            if let Some(e) = out.iter_mut().find(|(ek, _)| ek == k) {
                e.1 = v;
            } else {
                out.push((k.to_string(), v));
            }
        }
    }
    out
}

/// Lookup in a parsed cookie list.
pub fn cookie<'a>(cookies: &'a [(String, String)], name: &str) -> Option<&'a str> {
    cookies.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// `http.cookies._unquote`.
fn unquote_cookie(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() < 2 || b[0] != b'"' || b[b.len() - 1] != b'"' {
        return s.to_string();
    }
    let inner = &s[1..s.len() - 1];
    let ib = inner.as_bytes();
    let mut out = Vec::with_capacity(ib.len());
    let mut i = 0;
    while i < ib.len() {
        if ib[i] == b'\\' && i + 1 < ib.len() {
            // \ooo octal or \x quoted char
            // _OctalPatt = r"\\[0-3][0-7][0-7]", _QuotePatt = r'[\\].'
            if ib.len() >= i + 4 && (b'0'..=b'3').contains(&ib[i + 1]) && ib[i + 2..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                let o = (ib[i + 1] - b'0') as u32 * 64 + (ib[i + 2] - b'0') as u32 * 8 + (ib[i + 3] - b'0') as u32;
                out.push((o & 0xff) as u8);
                i += 4;
            } else {
                out.push(ib[i + 1]);
                i += 2;
            }
        } else {
            out.push(ib[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode header bytes like the ASGI handler does (`latin-1`).
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// A header as Django sees it in `request.META` for ASGI: repeated headers joined with ",",
/// bytes decoded as latin-1. Matching is on the META key (`-` and `_` both map to `_`).
pub fn meta_header(headers: &HeaderMap, meta_key: &str) -> Option<String> {
    let key = meta_key.strip_prefix("HTTP_")?;
    let mut out: Option<String> = None;
    for (name, value) in headers.iter() {
        // Django ASGIRequest: "Prevent spoofing via ambiguity between underscores and hyphens" –
        // headers with `_` in the name are discarded (otherwise `Remote_User: admin` past a proxy
        // that only sanitises `Remote-User`; security review 2026-11-19 F2).
        if name.as_str().contains('_') {
            continue;
        }
        let n = name.as_str().to_ascii_uppercase().replace('-', "_");
        if n == key {
            let v = latin1(value.as_bytes());
            out = Some(match out {
                Some(prev) => format!("{prev},{v}"),
                None => v,
            });
        }
    }
    out
}

/// Request data Django's CSRF/host logic needs.
pub struct RequestInfo<'a> {
    pub method: &'a str,
    pub headers: &'a HeaderMap,
}

/// `HttpRequest.is_secure()` / `scheme`.
pub fn is_secure(cfg: &AuthConfig, headers: &HeaderMap) -> bool {
    if let Some((meta, secure_value)) = &cfg.secure_proxy_ssl_header
        && let Some(v) = meta_header(headers, meta)
    {
        let first = v.split(',').next().unwrap_or("");
        return first.trim() == secure_value;
    }
    cfg.server_scheme_https
}

/// `HttpRequest._get_raw_host()`.
pub fn raw_host(cfg: &AuthConfig, headers: &HeaderMap) -> String {
    if cfg.use_x_forwarded_host
        && let Some(h) = meta_header(headers, "HTTP_X_FORWARDED_HOST")
    {
        return h;
    }
    if let Some(h) = meta_header(headers, "HTTP_HOST") {
        return h;
    }
    // SERVER_NAME/SERVER_PORT fallback (HTTP/1.1 always sends Host).
    cfg.server_name.clone()
}

/// `split_domain_port(host)`.
pub fn split_domain_port(host: &str) -> (String, String) {
    let h = host.to_lowercase();
    // ^([a-z0-9.-]+|\[[a-f0-9]*:[a-f0-9.:]+\])(?::([0-9]+))?$
    let (domain, port) = if h.starts_with('[') {
        let Some(end) = h.find(']') else {
            return (String::new(), String::new());
        };
        let d = &h[..=end];
        let inner = &d[1..d.len() - 1];
        let Some(colon) = inner.find(':') else {
            return (String::new(), String::new());
        };
        let (a, b) = inner.split_at(colon);
        let b = &b[1..];
        if !a.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            || b.is_empty()
            || !b.bytes().all(|c| (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) || c == b'.' || c == b':')
        {
            return (String::new(), String::new());
        }
        (d.to_string(), &h[end + 1..])
    } else {
        let (d, p) = match h.find(':') {
            Some(i) => (&h[..i], &h[i..]),
            None => (h.as_str(), ""),
        };
        if d.is_empty() || !d.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-') {
            return (String::new(), String::new());
        }
        (d.to_string(), p)
    };
    let port = if port.is_empty() {
        String::new()
    } else if let Some(p) = port.strip_prefix(':') {
        if p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()) {
            return (String::new(), String::new());
        }
        p.to_string()
    } else {
        return (String::new(), String::new());
    };
    (domain.strip_suffix('.').map(str::to_string).unwrap_or(domain), port)
}

/// `django.utils.http.is_same_domain(host, pattern)`.
pub fn is_same_domain(host: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let pattern = pattern.to_lowercase();
    (pattern.starts_with('.') && (host.ends_with(&pattern) || host == &pattern[1..])) || pattern == host
}

/// `validate_host(domain, allowed_hosts)`.
pub fn validate_host(domain: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|p| p == "*" || is_same_domain(domain, p))
}

/// `HttpRequest.get_host()`; `None` = `DisallowedHost`.
pub fn get_host(cfg: &AuthConfig, headers: &HeaderMap) -> Option<String> {
    let host = raw_host(cfg, headers);
    let default_dbg;
    let allowed: &[String] = if cfg.debug && cfg.allowed_hosts.is_empty() {
        default_dbg = vec![".localhost".into(), "127.0.0.1".into(), "[::1]".into()];
        &default_dbg
    } else {
        &cfg.allowed_hosts
    };
    let (domain, _port) = split_domain_port(&host);
    if !domain.is_empty() && validate_host(&domain, allowed) { Some(host) } else { None }
}

/// Result of CPython `urllib.parse.urlsplit` (only what CSRF needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

impl SplitUrl {
    /// `SplitResult.geturl()` (= `urlunsplit`).
    pub fn geturl(&self) -> String {
        let mut url = self.path.clone();
        if !self.netloc.is_empty() {
            if !url.is_empty() && !url.starts_with('/') {
                url = format!("/{url}");
            }
            url = format!("//{}{}", self.netloc, url);
        }
        if !self.scheme.is_empty() {
            url = format!("{}:{}", self.scheme, url);
        }
        if !self.query.is_empty() {
            url = format!("{url}?{}", self.query);
        }
        if !self.fragment.is_empty() {
            url = format!("{url}#{}", self.fragment);
        }
        url
    }
}

/// CPython `urlsplit(url)`; `None` = `ValueError` (e.g. "Invalid IPv6 URL").
pub fn urlsplit(url: &str) -> Option<SplitUrl> {
    // strip leading C0 control or space, remove \t \r \n
    let url: String = url.trim_start_matches(|c: char| (c as u32) <= 0x20).chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest: &str = &url;
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let cand = &rest[..i];
        let first_ok = cand.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if i > 0 && first_ok && cand.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            scheme = cand.to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = String::new();
    if rest.starts_with("//") {
        let after = &rest[2..];
        let delim = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..delim].to_string();
        rest = &after[delim..];
        let (ob, cb) = (netloc.contains('['), netloc.contains(']'));
        if ob != cb {
            return None;
        }
    }
    let (rest, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, b.to_string()),
        None => (rest, String::new()),
    };
    let (path, query) = match rest.split_once('?') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (rest.to_string(), String::new()),
    };
    Some(SplitUrl { scheme, netloc, path, query, fragment })
}

/// Django `CsrfViewMiddleware._origin_verified` (also for the WebSocket origin check in pl-server).
pub fn origin_verified(cfg: &AuthConfig, headers: &HeaderMap, origin: &str) -> bool {
    if let Some(good_host) = get_host(cfg, headers) {
        let scheme = if is_secure(cfg, headers) { "https" } else { "http" };
        if origin == format!("{scheme}://{good_host}") {
            return true;
        }
    }
    if cfg.csrf_trusted_origins.iter().any(|o| !o.contains('*') && o == origin) {
        return true;
    }
    let Some(parsed) = urlsplit(origin) else {
        return false;
    };
    cfg.csrf_trusted_origins
        .iter()
        .filter(|o| o.contains('*'))
        .filter_map(|o| urlsplit(o))
        .filter(|p| p.scheme == parsed.scheme)
        .any(|p| is_same_domain(&parsed.netloc, p.netloc.trim_start_matches('*')))
}

fn check_referer(cfg: &AuthConfig, headers: &HeaderMap) -> Result<(), String> {
    let Some(referer) = meta_header(headers, "HTTP_REFERER") else {
        return Err(REASON_NO_REFERER.into());
    };
    let Some(r) = urlsplit(&referer) else {
        return Err(REASON_MALFORMED_REFERER.into());
    };
    if r.scheme.is_empty() || r.netloc.is_empty() {
        return Err(REASON_MALFORMED_REFERER.into());
    }
    if r.scheme != "https" {
        return Err(REASON_INSECURE_REFERER.into());
    }
    let trusted_hosts =
        cfg.csrf_trusted_origins.iter().map(|o| urlsplit(o).map(|p| p.netloc.trim_start_matches('*').to_string()).unwrap_or_default());
    for h in trusted_hosts {
        if is_same_domain(&r.netloc, &h) {
            return Ok(());
        }
    }
    // CSRF_COOKIE_DOMAIN is never set by paperless -> good_referer = request.get_host()
    let Some(good) = get_host(cfg, headers) else {
        return Err(REASON_BAD_REFERER.replace("%s", &r.geturl()));
    };
    if !is_same_domain(&r.netloc, &good) {
        return Err(REASON_BAD_REFERER.replace("%s", &r.geturl()));
    }
    Ok(())
}

fn bad_token_message(reason: &str) -> String {
    format!("CSRF token from the '{CSRF_HEADER_DISPLAY}' HTTP header {reason}.")
}

/// `_bad_token_message(reason, token_source="POST")`.
fn bad_form_token_message(reason: &str) -> String {
    format!("CSRF token from POST {reason}.")
}

/// `CSRFCheck(...).process_request(request); reason = check.process_view(request, None, (), {})`.
///
/// Returns `Err(reason)`; DRF turns it into `PermissionDenied("CSRF Failed: %s" % reason)`.
/// Only the `X-CSRFToken` header; with a form body use [`enforce_csrf_with_form`].
pub fn enforce_csrf(cfg: &AuthConfig, req: &RequestInfo<'_>) -> Result<(), String> {
    enforce_csrf_with_form(cfg, req, None)
}

/// [`enforce_csrf`] with `request.POST.get("csrfmiddlewaretoken", "")`: for a POST a non-empty
/// form value is checked instead of the header (`_check_token`, token source "POST").
pub fn enforce_csrf_with_form(cfg: &AuthConfig, req: &RequestInfo<'_>, form_token: Option<&str>) -> Result<(), String> {
    if matches!(req.method, "GET" | "HEAD" | "OPTIONS" | "TRACE") {
        return Ok(());
    }
    if let Some(origin) = meta_header(req.headers, "HTTP_ORIGIN") {
        if !origin_verified(cfg, req.headers, &origin) {
            return Err(REASON_BAD_ORIGIN.replace("%s", &origin));
        }
    } else if is_secure(cfg, req.headers) {
        check_referer(cfg, req.headers)?;
    }
    // _check_token
    let cookies = parse_cookies(req.headers);
    let secret = match cookie(&cookies, &cfg.csrf_cookie_name()) {
        None => return Err(REASON_NO_CSRF_COOKIE.into()),
        Some(c) => {
            check_token_format(c).map_err(|r| format!("CSRF cookie {r}."))?;
            if c.len() == CSRF_TOKEN_LENGTH { unmask_cipher_token(c).unwrap_or_default() } else { c.to_string() }
        }
    };
    if let Some(form) = form_token.filter(|t| !t.is_empty() && req.method == "POST") {
        check_token_format(form).map_err(bad_form_token_message)?;
        if !does_token_match(form, &secret) {
            return Err(bad_form_token_message("incorrect"));
        }
        return Ok(());
    }
    let Some(token) = meta_header(req.headers, "HTTP_X_CSRFTOKEN") else {
        return Err(REASON_CSRF_TOKEN_MISSING.into());
    };
    check_token_format(&token).map_err(bad_token_message)?;
    if !does_token_match(&token, &secret) {
        return Err(bad_token_message("incorrect"));
    }
    Ok(())
}

// ------------------------------------------------------------------ csrfmiddlewaretoken (S6)

/// `csrfmiddlewaretoken` read from the body of a form POST by [`form_token_middleware`]
/// (request extension; the extractors pass it to [`crate::Authz::authenticate_with`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormCsrfToken(pub String);

/// Django `DATA_UPLOAD_MAX_MEMORY_SIZE` (2.5 MiB): an urlencoded body up to this size is read.
pub const FORM_BODY_LIMIT: usize = 2_621_440;
/// For `multipart/form-data` only a prefix is buffered while looking for the field (uploads can
/// be large; HTML forms put the hidden field first). See Known differences.
pub const MULTIPART_SCAN_LIMIT: usize = 1 << 20;

const FORM_FIELD: &str = "csrfmiddlewaretoken";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormKind {
    UrlEncoded,
    Multipart,
}

fn form_kind(headers: &HeaderMap) -> Option<(FormKind, Option<String>)> {
    let ct = headers.get(axum::http::header::CONTENT_TYPE)?.to_str().ok()?;
    let mut parts = ct.split(';');
    let mime = parts.next()?.trim().to_ascii_lowercase();
    match mime.as_str() {
        "application/x-www-form-urlencoded" => Some((FormKind::UrlEncoded, None)),
        "multipart/form-data" => {
            let boundary = parts.find_map(|p| {
                let (k, v) = p.split_once('=')?;
                (k.trim().eq_ignore_ascii_case("boundary")).then(|| v.trim().trim_matches('"').to_string())
            })?;
            (!boundary.is_empty()).then_some((FormKind::Multipart, Some(boundary)))
        }
        _ => None,
    }
}

/// `QueryDict(body).get("csrfmiddlewaretoken")` (last value wins like `QueryDict.get`).
pub fn urlencoded_field(body: &[u8], name: &str) -> Option<String> {
    url_pairs(body).into_iter().rev().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn url_pairs(body: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in body.split(|b| *b == b'&' || *b == b';') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.iter().position(|b| *b == b'=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, &b""[..]),
        };
        out.push((percent_decode_plus(k), percent_decode_plus(v)));
    }
    out
}

fn percent_decode_plus(b: &[u8]) -> String {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Result of scanning a multipart prefix.
#[derive(Debug, PartialEq, Eq)]
pub enum MultipartScan {
    Found(String),
    /// Not (yet) in the buffered prefix.
    NeedMore,
    /// Closing boundary seen without the field.
    Absent,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Look for the non-file part `name` in a (prefix of a) `multipart/form-data` body.
pub fn multipart_field(buf: &[u8], boundary: &str, name: &str) -> MultipartScan {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let Some(mut pos) = find(buf, delim, 0) else { return MultipartScan::NeedMore };
    loop {
        let after = pos + delim.len();
        if buf.len() < after + 2 {
            return MultipartScan::NeedMore;
        }
        if &buf[after..after + 2] == b"--" {
            return MultipartScan::Absent;
        }
        let Some(head_end) = find(buf, b"\r\n\r\n", after) else { return MultipartScan::NeedMore };
        let head = String::from_utf8_lossy(&buf[after..head_end]).to_ascii_lowercase();
        let body_start = head_end + 4;
        let mut next_delim = b"\r\n".to_vec();
        next_delim.extend_from_slice(delim);
        let Some(body_end) = find(buf, &next_delim, body_start) else { return MultipartScan::NeedMore };
        let disposition = head.lines().find(|l| l.trim_start().starts_with("content-disposition:")).unwrap_or("");
        let is_field = disposition.contains(&format!("name=\"{name}\"")) && !disposition.contains("filename");
        if is_field {
            return MultipartScan::Found(String::from_utf8_lossy(&buf[body_start..body_end]).into_owned());
        }
        pos = body_end + 2;
    }
}

/// Middleware: for a form `POST` without `X-CSRFToken` header, read `csrfmiddlewaretoken` from
/// the body (urlencoded up to [`FORM_BODY_LIMIT`], multipart from a prefix up to
/// [`MULTIPART_SCAN_LIMIT`]), store it as [`FormCsrfToken`] and pass the unchanged body on.
/// Install it before the routes (pl-server, CORE-REQUEST in docs/contract/pl-authz.md).
pub async fn form_token_middleware(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let wanted = req.method() == axum::http::Method::POST
        && req.headers().get("x-csrftoken").is_none()
        && req.headers().contains_key(axum::http::header::COOKIE);
    let kind = if wanted { form_kind(req.headers()) } else { None };
    let Some((kind, boundary)) = kind else {
        return next.run(req).await;
    };
    let (mut parts, body) = req.into_parts();
    let (token, body) = scan_body(kind, boundary.as_deref(), body).await;
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        parts.extensions.insert(FormCsrfToken(t));
    }
    next.run(axum::extract::Request::from_parts(parts, body)).await
}

/// Reads as much of `body` as needed and returns the token plus an equivalent body
/// (buffered prefix followed by the unread rest).
async fn scan_body(kind: FormKind, boundary: Option<&str>, body: axum::body::Body) -> (Option<String>, axum::body::Body) {
    use futures::StreamExt as _;
    let limit = match kind {
        FormKind::UrlEncoded => FORM_BODY_LIMIT,
        FormKind::Multipart => MULTIPART_SCAN_LIMIT,
    };
    let mut stream = body.into_data_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut token = None;
    let mut ended = false;
    let mut failed: Option<axum::Error> = None;
    loop {
        if let (FormKind::Multipart, Some(b)) = (kind, boundary) {
            match multipart_field(&buf, b, FORM_FIELD) {
                MultipartScan::Found(t) => {
                    token = Some(t);
                    break;
                }
                MultipartScan::Absent => break,
                MultipartScan::NeedMore => {}
            }
        }
        if buf.len() >= limit {
            break;
        }
        match stream.next().await {
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
            Some(Err(e)) => {
                failed = Some(e);
                break;
            }
            None => {
                ended = true;
                break;
            }
        }
    }
    if kind == FormKind::UrlEncoded && ended {
        token = urlencoded_field(&buf, FORM_FIELD);
    }
    let prefix = futures::stream::iter(std::iter::once(Ok::<bytes::Bytes, axum::Error>(bytes::Bytes::from(buf))));
    let body = match failed {
        Some(e) => axum::body::Body::from_stream(prefix.chain(futures::stream::iter(std::iter::once(Err(e))))),
        None if ended => axum::body::Body::from_stream(prefix),
        None => axum::body::Body::from_stream(prefix.chain(stream)),
    };
    (token, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = include_str!("testvectors/csrf_secret.txt");
    const MASKED: &str = include_str!("testvectors/csrf_masked.txt");

    /// Security review F2: `Remote_User`/`X_Forwarded_For` do not count as `Remote-User` etc.
    #[test]
    fn meta_header_ignores_underscore_names() {
        let mut h = HeaderMap::new();
        h.insert("remote_user", HeaderValue::from_static("admin"));
        h.insert("x_forwarded_for", HeaderValue::from_static("1.2.3.4"));
        assert_eq!(meta_header(&h, "HTTP_REMOTE_USER"), None);
        assert_eq!(meta_header(&h, "HTTP_X_FORWARDED_FOR"), None);
        h.insert("remote-user", HeaderValue::from_static("alice"));
        assert_eq!(meta_header(&h, "HTTP_REMOTE_USER").as_deref(), Some("alice"));
    }

    fn cfg() -> AuthConfig {
        let mut c = AuthConfig::for_tests("s3cret");
        c.allowed_hosts = vec!["paperless.example".into(), "localhost".into()];
        c.csrf_trusted_origins = vec!["https://trusted.example".into(), "https://*.wild.example".into()];
        c
    }

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn unmask_vector() {
        assert_eq!(unmask_cipher_token(MASKED).as_deref(), Some(SECRET));
        let m = mask_cipher_secret(SECRET);
        assert_eq!(unmask_cipher_token(&m).as_deref(), Some(SECRET));
        assert!(does_token_match(MASKED, SECRET));
        assert!(does_token_match(SECRET, SECRET));
        assert_eq!(mask_with(SECRET, &MASKED[..32]), MASKED);
    }

    #[test]
    fn token_format() {
        assert_eq!(check_token_format("abc"), Err(REASON_INCORRECT_LENGTH));
        assert_eq!(check_token_format(&"-".repeat(32)), Err(REASON_INVALID_CHARACTERS));
        assert!(check_token_format(SECRET).is_ok());
    }

    #[test]
    fn csrf_flow() {
        let c = cfg();
        let cookie = format!("csrftoken={SECRET}");
        let ok = hm(&[("host", "paperless.example"), ("cookie", &cookie), ("x-csrftoken", MASKED)]);
        assert_eq!(enforce_csrf(&c, &RequestInfo { method: "POST", headers: &ok }), Ok(()));
        assert_eq!(enforce_csrf(&c, &RequestInfo { method: "GET", headers: &hm(&[]) }), Ok(()));

        let no_cookie = hm(&[("host", "paperless.example")]);
        assert_eq!(enforce_csrf(&c, &RequestInfo { method: "POST", headers: &no_cookie }), Err("CSRF cookie not set.".into()));
        let missing = hm(&[("host", "paperless.example"), ("cookie", &cookie)]);
        assert_eq!(enforce_csrf(&c, &RequestInfo { method: "PUT", headers: &missing }), Err("CSRF token missing.".into()));
        let bad = hm(&[("host", "paperless.example"), ("cookie", &cookie), ("x-csrftoken", "abc")]);
        assert_eq!(
            enforce_csrf(&c, &RequestInfo { method: "PATCH", headers: &bad }),
            Err("CSRF token from the 'X-Csrftoken' HTTP header has incorrect length.".into())
        );
        let wrong = "b".repeat(32);
        let inc = hm(&[("host", "paperless.example"), ("cookie", &cookie), ("x-csrftoken", &wrong)]);
        assert_eq!(
            enforce_csrf(&c, &RequestInfo { method: "DELETE", headers: &inc }),
            Err("CSRF token from the 'X-Csrftoken' HTTP header incorrect.".into())
        );
        let badc = hm(&[("host", "paperless.example"), ("cookie", "csrftoken=short")]);
        assert_eq!(enforce_csrf(&c, &RequestInfo { method: "POST", headers: &badc }), Err("CSRF cookie has incorrect length.".into()));
    }

    #[test]
    fn origin_checks() {
        let c = cfg();
        let cookie = format!("csrftoken={SECRET}");
        for (origin, ok) in [
            ("http://paperless.example", true),
            ("https://paperless.example", false),
            ("https://trusted.example", true),
            ("https://a.wild.example", true),
            ("http://a.wild.example", false),
            ("https://evil.example", false),
        ] {
            let h = hm(&[("host", "paperless.example"), ("origin", origin), ("cookie", &cookie), ("x-csrftoken", SECRET)]);
            let r = enforce_csrf(&c, &RequestInfo { method: "POST", headers: &h });
            if ok {
                assert_eq!(r, Ok(()), "{origin}");
            } else {
                assert_eq!(r, Err(format!("Origin checking failed - {origin} does not match any trusted origins.")), "{origin}");
            }
        }
    }

    #[test]
    fn referer_checks_when_secure() {
        let mut c = cfg();
        c.secure_proxy_ssl_header = Some(("HTTP_X_FORWARDED_PROTO".into(), "https".into()));
        let cookie = format!("csrftoken={SECRET}");
        let base = |r: Option<&str>| {
            let mut v =
                vec![("host", "paperless.example"), ("x-forwarded-proto", "https"), ("cookie", cookie.as_str()), ("x-csrftoken", SECRET)];
            if let Some(r) = r {
                v.push(("referer", r));
            }
            hm(&v)
        };
        let run = |h: &HeaderMap| enforce_csrf(&c, &RequestInfo { method: "POST", headers: h });
        assert_eq!(run(&base(None)), Err(REASON_NO_REFERER.into()));
        assert_eq!(run(&base(Some("nonsense"))), Err(REASON_MALFORMED_REFERER.into()));
        assert_eq!(run(&base(Some("http://paperless.example/"))), Err(REASON_INSECURE_REFERER.into()));
        assert_eq!(run(&base(Some("https://paperless.example/x"))), Ok(()));
        assert_eq!(run(&base(Some("https://trusted.example/x"))), Ok(()));
        assert_eq!(
            run(&base(Some("https://evil.example/a?b#c"))),
            Err("Referer checking failed - https://evil.example/a?b#c does not match any trusted origins.".into())
        );
    }

    #[test]
    fn host_helpers() {
        assert_eq!(split_domain_port("Example.COM:8000"), ("example.com".into(), "8000".into()));
        assert_eq!(split_domain_port("example.com."), ("example.com".into(), "".into()));
        assert_eq!(split_domain_port("[::1]:80"), ("[::1]".into(), "80".into()));
        assert_eq!(split_domain_port("bad host"), ("".into(), "".into()));
        assert!(is_same_domain("a.example.com", ".example.com"));
        assert!(is_same_domain("example.com", ".example.com"));
        assert!(!is_same_domain("example.com", ""));
        let c = cfg();
        assert_eq!(get_host(&c, &hm(&[("host", "evil.com")])), None);
        assert_eq!(get_host(&c, &hm(&[("host", "localhost:8000")])).as_deref(), Some("localhost:8000"));
    }

    #[test]
    fn cookies() {
        let v = parse_cookie_str(r#"a=1; csrftoken="x\"y"; a=2; flag"#);
        assert_eq!(cookie(&v, "a"), Some("2"));
        assert_eq!(cookie(&v, "csrftoken"), Some("x\"y"));
        assert_eq!(cookie(&v, ""), Some("flag"));
    }

    /// S6: `csrfmiddlewaretoken` (form, POST only) takes precedence over the header, errors name
    /// the source "POST" like Django's `_bad_token_message`.
    #[test]
    fn form_token_like_django() {
        let cookie = format!("csrftoken={SECRET}");
        let base = hm(&[("host", "paperless.example"), ("cookie", &cookie)]);
        let info = RequestInfo { method: "POST", headers: &base };
        let c = cfg();
        assert_eq!(enforce_csrf_with_form(&c, &info, Some(MASKED)), Ok(()));
        assert_eq!(enforce_csrf_with_form(&c, &info, Some("abc")), Err("CSRF token from POST has incorrect length.".into()));
        let mut wrong = MASKED.to_string();
        wrong.replace_range(0..1, if MASKED.starts_with('a') { "b" } else { "a" });
        assert_eq!(enforce_csrf_with_form(&c, &info, Some(&wrong)), Err("CSRF token from POST incorrect.".into()));
        // empty form value -> header (missing)
        assert_eq!(enforce_csrf_with_form(&c, &info, Some("")), Err(REASON_CSRF_TOKEN_MISSING.into()));
        // not POST: the form value is ignored
        let put = RequestInfo { method: "PUT", headers: &base };
        assert_eq!(enforce_csrf_with_form(&c, &put, Some(MASKED)), Err(REASON_CSRF_TOKEN_MISSING.into()));
    }

    #[test]
    fn form_body_parsing() {
        assert_eq!(urlencoded_field(b"a=1&csrfmiddlewaretoken=x%2By+z&b=2", FORM_FIELD).as_deref(), Some("x+y z"));
        assert_eq!(urlencoded_field(b"csrfmiddlewaretoken=1&csrfmiddlewaretoken=2", FORM_FIELD).as_deref(), Some("2"));
        assert_eq!(urlencoded_field(b"a=1", FORM_FIELD), None);
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nT\r\n--XyZ\r\nContent-Disposition: form-data; name=\"csrfmiddlewaretoken\"\r\n\r\nTOKEN\r\n--XyZ\r\nContent-Disposition: form-data; name=\"document\"; filename=\"a.pdf\"\r\n\r\n%PDF\r\n--XyZ--\r\n";
        assert_eq!(multipart_field(body, "XyZ", FORM_FIELD), MultipartScan::Found("TOKEN".into()));
        assert_eq!(multipart_field(&body[..40], "XyZ", FORM_FIELD), MultipartScan::NeedMore);
        let none = b"--XyZ\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nT\r\n--XyZ--\r\n";
        assert_eq!(multipart_field(none, "XyZ", FORM_FIELD), MultipartScan::Absent);
        let mut h = HeaderMap::new();
        h.insert("content-type", HeaderValue::from_static("multipart/form-data; boundary=\"XyZ\""));
        assert_eq!(form_kind(&h), Some((FormKind::Multipart, Some("XyZ".into()))));
    }

    /// The middleware stores the token and the handler still sees the whole body.
    #[tokio::test]
    async fn middleware_keeps_body() {
        use tower::ServiceExt as _;
        let app = axum::Router::new()
            .route(
                "/f",
                axum::routing::post(|req: axum::extract::Request| async move {
                    let t = req.extensions().get::<FormCsrfToken>().map(|t| t.0.clone()).unwrap_or_default();
                    let b = axum::body::to_bytes(req.into_body(), 8 << 20).await.unwrap_or_default();
                    format!("{t}|{}", String::from_utf8_lossy(&b))
                }),
            )
            .layer(axum::middleware::from_fn(form_token_middleware));
        let req = axum::http::Request::post("/f")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", "sessionid=x")
            .body(axum::body::Body::from("a=1&csrfmiddlewaretoken=TOK"))
            .unwrap();
        let r = app.clone().oneshot(req).await.unwrap();
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&b[..], b"TOK|a=1&csrfmiddlewaretoken=TOK");
        let big = format!(
            "--B\r\nContent-Disposition: form-data; name=\"csrfmiddlewaretoken\"\r\n\r\nM\r\n--B\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\n{}\r\n--B--\r\n",
            "x".repeat(3 << 20)
        );
        let req = axum::http::Request::post("/f")
            .header("content-type", "multipart/form-data; boundary=B")
            .header("cookie", "sessionid=x")
            .body(axum::body::Body::from(big.clone()))
            .unwrap();
        let r = app.oneshot(req).await.unwrap();
        let b = axum::body::to_bytes(r.into_body(), 8 << 20).await.unwrap();
        assert_eq!(b.len(), 2 + big.len());
        assert!(b.starts_with(b"M|--B"));
    }
}
