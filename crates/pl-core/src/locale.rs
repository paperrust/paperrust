//! Language negotiation exactly like Django's `LocaleMiddleware` (no i18n URL patterns).
//!
//! upstream: django/utils/translation/trans_real.py@5.2.18 (get_language_from_request,
//! get_supported_language_variant, parse_accept_lang_header, check_for_language)
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (LANGUAGE_CODE, LANGUAGES)
//!
//! Single implementation for the whole workspace (checked against the reference:
//! `de-CH` -> `de-de`, cookie `fr` -> `fr-fr`). Used by `pl-server` (`Content-Language`) and
//! by [`crate::RequestContext::language`].

use std::sync::LazyLock;

use regex::Regex;

pub const LANGUAGE_CODE: &str = "en-us";

/// `settings.LANGUAGES` keys (lowercase, as `get_languages()` returns them), in order.
pub const LANGUAGES: &[&str] = &[
    "en-us", "ar-ar", "af-za", "be-by", "bg-bg", "ca-es", "cs-cz", "da-dk", "de-de", "el-gr", "en-gb", "es-es", "fa-ir", "fi-fi", "fr-fr",
    "hu-hu", "id-id", "it-it", "ja-jp", "ko-kr", "lb-lu", "no-no", "nl-nl", "pl-pl", "pt-br", "pt-pt", "ro-ro", "ru-ru", "sk-sk", "sl-si",
    "sr-cs", "sv-se", "tr-tr", "uk-ua", "vi-vn", "zh-cn", "zh-tw",
];

const LANGUAGE_CODE_MAX_LENGTH: usize = 500;

#[allow(clippy::expect_used)]
static ACCEPT_LANGUAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([A-Za-z]{1,8}(?:-[A-Za-z0-9]{1,8})*|\*)(?:\s*;\s*q=(0(?:\.[0-9]{0,3})?|1(?:\.0{0,3})?))?(?:\s*,\s*|$)")
        .expect("static regex")
});

#[allow(clippy::expect_used)]
static LANGUAGE_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z]{1,8}(?:-[a-z0-9]{1,8})*(?:@[a-z0-9]{1,20})?$").expect("static regex"));

/// `check_for_language`: assumes a catalog exists for every entry of `LANGUAGES`
/// (Django ships catalogs for all of them except possibly `no`; see contract).
fn check_for_language(code: &str) -> bool {
    code.len() <= LANGUAGE_CODE_MAX_LENGTH && LANGUAGE_CODE_RE.is_match(code) && LANGUAGES.contains(&code.to_lowercase().as_str())
}

fn supported_variant_inner(lang_code: &str, strict: bool) -> Option<String> {
    // LANG_INFO fallbacks are omitted: none of them is in LANGUAGES, and they never
    // change the generic code (the last '-' prefix), so the result is identical.
    let mut possible = vec![lang_code.to_owned()];
    let mut end = lang_code.len();
    while let Some(i) = lang_code[..end].rfind('-') {
        possible.push(lang_code[..i].to_owned());
        end = i;
    }
    let generic = possible.last().cloned().unwrap_or_default();
    for code in &possible {
        if LANGUAGES.contains(&code.to_lowercase().as_str()) && check_for_language(code) {
            return Some(code.clone());
        }
    }
    if !strict {
        let prefix = format!("{generic}-");
        if let Some(s) = LANGUAGES.iter().find(|s| s.starts_with(&prefix)) {
            return Some((*s).to_owned());
        }
    }
    None
}

/// `get_supported_language_variant(lang_code, strict=False)`
pub fn get_supported_language_variant(lang_code: &str) -> Option<String> {
    if lang_code.is_empty() {
        return None;
    }
    if lang_code.len() <= LANGUAGE_CODE_MAX_LENGTH {
        return supported_variant_inner(lang_code, false);
    }
    let cut = floor_char_boundary(lang_code, LANGUAGE_CODE_MAX_LENGTH);
    match lang_code[..cut].rfind('-') {
        Some(i) if i > 0 => supported_variant_inner(&lang_code[..i], false),
        _ => None,
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `_parse_accept_lang_header`: emulates `re.split` (text between matches must be empty).
fn parse_accept_lang_inner(lang_string: &str) -> Vec<(String, f64)> {
    let lower = lang_string.to_lowercase();
    let mut result = Vec::new();
    let mut pos = 0;
    for caps in ACCEPT_LANGUAGE_RE.captures_iter(&lower) {
        let Some(m) = caps.get(0) else { continue };
        if m.start() != pos {
            return Vec::new();
        }
        pos = m.end();
        let lang = caps.get(1).map(|g| g.as_str()).unwrap_or("").to_owned();
        let q = match caps.get(2).map(|g| g.as_str()) {
            Some(p) if !p.is_empty() => p.parse::<f64>().unwrap_or(1.0),
            _ => 1.0,
        };
        result.push((lang, q));
        if m.end() == m.start() {
            break;
        }
    }
    if pos != lower.len() {
        return Vec::new();
    }
    // Python's sort(reverse=True) is stable.
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result
}

/// `parse_accept_lang_header` (with the 500-character truncation rule).
pub fn parse_accept_lang_header(lang_string: &str) -> Vec<(String, f64)> {
    if lang_string.len() <= LANGUAGE_CODE_MAX_LENGTH {
        return parse_accept_lang_inner(lang_string);
    }
    let cut = floor_char_boundary(lang_string, LANGUAGE_CODE_MAX_LENGTH);
    match lang_string[..cut].rfind(',') {
        Some(i) if i > 0 => parse_accept_lang_inner(&lang_string[..i]),
        _ => Vec::new(),
    }
}

/// Django `parse_cookie` + `http.cookies._unquote` (simplified: backslash escapes).
pub fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    let mut found = None;
    for chunk in cookie_header.split(';') {
        let (k, v) = match chunk.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => ("", chunk.trim()),
        };
        if (!k.is_empty() || !v.is_empty()) && k == name {
            found = Some(unquote(v));
        }
    }
    found
}

fn unquote(v: &str) -> String {
    if v.len() < 2 || !v.starts_with('"') || !v.ends_with('"') {
        return v.to_owned();
    }
    let inner = &v[1..v.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `get_language_from_request(request, check_path=False)` followed by
/// `translation.get_language()` (which lowercases via `to_language`).
pub fn language_from_request(cookie: Option<&str>, accept_language: &str) -> String {
    let code = negotiate(cookie, accept_language);
    to_language(&code)
}

fn negotiate(cookie: Option<&str>, accept_language: &str) -> String {
    if let Some(c) = cookie {
        if LANGUAGES.contains(&c) && check_for_language(c) {
            return c.to_owned();
        }
        if let Some(v) = get_supported_language_variant(c) {
            return v;
        }
    }
    for (lang, _) in parse_accept_lang_header(accept_language) {
        if lang == "*" {
            break;
        }
        if !LANGUAGE_CODE_RE.is_match(&lang) {
            continue;
        }
        if let Some(v) = get_supported_language_variant(&lang) {
            return v;
        }
    }
    get_supported_language_variant(LANGUAGE_CODE).unwrap_or_else(|| LANGUAGE_CODE.to_owned())
}

/// Header-based variant: `Cookie` (all values, joined with `; ` like Django's ASGI
/// handler) and `Accept-Language` (latin-1, joined with `,`). Result is always an entry of
/// [`LANGUAGES`].
pub fn language_from_headers(headers: &axum::http::HeaderMap, language_cookie_name: &str) -> &'static str {
    let latin1 = |v: &axum::http::HeaderValue| v.as_bytes().iter().map(|&b| b as char).collect::<String>();
    let cookie = headers.get_all(axum::http::header::COOKIE).iter().map(latin1).collect::<Vec<_>>().join("; ");
    let accept = headers.get_all(axum::http::header::ACCEPT_LANGUAGE).iter().map(latin1).collect::<Vec<_>>().join(",");
    let lang = language_from_request(cookie_value(&cookie, language_cookie_name).as_deref(), &accept);
    static_language(&lang)
}

/// The `LANGUAGES` entry equal to `code` (fallback `LANGUAGE_CODE`).
pub fn static_language(code: &str) -> &'static str {
    LANGUAGES.iter().find(|l| **l == code).copied().unwrap_or(LANGUAGE_CODE)
}

/// `django.utils.translation.to_language`
fn to_language(locale: &str) -> String {
    match locale.find('_') {
        Some(p) => format!("{}-{}", locale[..p].to_lowercase(), locale[p + 1..].to_lowercase()),
        None => locale.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_language_variants() {
        assert_eq!(language_from_request(None, ""), "en-us");
        assert_eq!(language_from_request(None, "de-DE,de;q=0.9"), "de-de");
        assert_eq!(language_from_request(None, "de-CH"), "de-de");
        assert_eq!(language_from_request(None, "de"), "de-de");
        assert_eq!(language_from_request(None, "en"), "en-us");
        assert_eq!(language_from_request(None, "en-GB"), "en-gb");
        assert_eq!(language_from_request(None, "pt"), "pt-br");
        assert_eq!(language_from_request(None, "xx, fr;q=0.5"), "fr-fr");
        assert_eq!(language_from_request(None, "fr;q=0.5, it"), "it-it");
        assert_eq!(language_from_request(None, "*, de"), "en-us");
        assert_eq!(language_from_request(None, "zh-hk"), "zh-cn");
        // malformed header -> whole header ignored
        assert_eq!(language_from_request(None, "de;q=2"), "en-us");
        assert_eq!(language_from_request(None, "de,,fr"), "en-us");
    }

    #[test]
    fn cookie_wins() {
        assert_eq!(language_from_request(Some("fr-fr"), "de"), "fr-fr");
        assert_eq!(language_from_request(Some("FR-FR"), "de"), "fr-fr");
        assert_eq!(language_from_request(Some("fr"), "de"), "fr-fr");
        assert_eq!(language_from_request(Some("xx"), "de"), "de-de");
    }

    #[test]
    fn parse_header() {
        let p = parse_accept_lang_header("en;q=0.5, de-DE, fr;q=0.8");
        let langs: Vec<_> = p.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(langs, ["de-de", "fr", "en"]);
        assert!(parse_accept_lang_header("en-us;q=1.0001").is_empty());
    }

    #[test]
    fn same_languages_as_formats() {
        assert_eq!(LANGUAGES, crate::locale_formats::LANGUAGES);
    }

    #[test]
    fn from_headers() {
        use axum::http::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert("accept-language", HeaderValue::from_static("de-CH,de;q=0.9"));
        assert_eq!(language_from_headers(&h, "django_language"), "de-de");
        h.insert("cookie", HeaderValue::from_static("x=1; django_language=fr"));
        assert_eq!(language_from_headers(&h, "django_language"), "fr-fr");
    }

    #[test]
    fn cookies() {
        assert_eq!(cookie_value("a=1; django_language=de-de; b=2", "django_language").as_deref(), Some("de-de"));
        assert_eq!(cookie_value("x=\"q\\\"v\"", "x").as_deref(), Some("q\"v"));
        assert_eq!(cookie_value("a=1", "django_language"), None);
    }
}
