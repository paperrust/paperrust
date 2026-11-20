//! URL encoding exactly like Python `urllib.parse` and DRF `rest_framework.utils.urls`.
//!
//! upstream: CPython 3.12 `urllib/parse.py` (quote, quote_plus, unquote, parse_qsl, urlencode,
//! urlsplit/urlunsplit), rest_framework/utils/urls.py@3.18.3 (replace_query_param,
//! remove_query_param), django/utils/encoding.py@5.2 (escape_uri_path, iri_to_uri).

/// Always-safe characters for Python `quote` (`_ALWAYS_SAFE`).
fn always_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~')
}

/// Python `urllib.parse.quote_from_bytes(bytes, safe)`.
pub fn quote_bytes(input: &[u8], safe: &str) -> String {
    let safe = safe.as_bytes();
    let mut out = String::with_capacity(input.len());
    for &b in input {
        if always_safe(b) || (b.is_ascii() && safe.contains(&b)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Python `urllib.parse.quote(str, safe)` (UTF-8).
pub fn quote(input: &str, safe: &str) -> String {
    quote_bytes(input.as_bytes(), safe)
}

/// Python `urllib.parse.quote_plus(str, safe)`: space → `+`.
pub fn quote_plus(input: &str, safe: &str) -> String {
    if input.contains(' ') {
        let mut s = String::from(safe);
        s.push(' ');
        quote(input, &s).replace(' ', "+")
    } else {
        quote(input, safe)
    }
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Python `unquote_to_bytes`: only valid `%XX` are decoded, everything else stays.
pub fn unquote_to_bytes(input: &str) -> Vec<u8> {
    let b = input.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hexval(b[i + 1]), hexval(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Python `unquote(str, encoding="utf-8", errors="replace")`.
pub fn unquote(input: &str) -> String {
    if !input.contains('%') {
        return input.to_string();
    }
    String::from_utf8_lossy(&unquote_to_bytes(input)).into_owned()
}

/// Python `parse_qsl(qs, keep_blank_values=...)` with separator `&` (Python ≥ 3.10).
pub fn parse_qsl(qs: &str, keep_blank_values: bool) -> Vec<(String, String)> {
    let mut r = Vec::new();
    if qs.is_empty() {
        return r;
    }
    for nv in qs.split('&') {
        if nv.is_empty() {
            continue;
        }
        let (name, value) = match nv.split_once('=') {
            Some((n, v)) => (n, v),
            None => {
                if keep_blank_values {
                    (nv, "")
                } else {
                    continue;
                }
            }
        };
        if !value.is_empty() || keep_blank_values {
            r.push((unquote(&name.replace('+', " ")), unquote(&value.replace('+', " "))));
        }
    }
    r
}

/// Python `parse_qs(qs, keep_blank_values=True)`: keys in order of first
/// occurrence, values collected.
pub fn parse_qs(qs: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (k, v) in parse_qsl(qs, true) {
        if let Some(e) = out.iter_mut().find(|(ek, _)| *ek == k) {
            e.1.push(v);
        } else {
            out.push((k, vec![v]));
        }
    }
    out
}

/// Python `urlencode(items, doseq=True)` with `quote_plus` and `safe=''`.
pub fn urlencode_doseq(items: &[(String, Vec<String>)]) -> String {
    let mut parts = Vec::new();
    for (k, vs) in items {
        let k = quote_plus(k, "");
        for v in vs {
            parts.push(format!("{k}={}", quote_plus(v, "")));
        }
    }
    parts.join("&")
}

/// Result of [`urlsplit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

fn valid_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// Simplified Python `urlsplit` (without stripping control characters).
pub fn urlsplit(url: &str) -> SplitUrl {
    let mut rest = url;
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let cand = &rest[..i];
        if !cand.is_empty()
            && cand.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && cand.chars().all(valid_scheme_char)
        {
            scheme = cand.to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        netloc = r[..end].to_string();
        rest = &r[end..];
    }
    let (rest, fragment) = match rest.split_once('#') {
        Some((a, f)) => (a, f.to_string()),
        None => (rest, String::new()),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (rest.to_string(), String::new()),
    };
    SplitUrl { scheme, netloc, path, query, fragment }
}

/// Python `urlunsplit`.
pub fn urlunsplit(u: &SplitUrl) -> String {
    let mut url = u.path.clone();
    if !u.netloc.is_empty() {
        if !url.is_empty() && !url.starts_with('/') {
            url.insert(0, '/');
        }
        url = format!("//{}{}", u.netloc, url);
    } else if url.starts_with("//")
        || (matches!(u.scheme.as_str(), "http" | "https" | "ftp" | "ws" | "wss")
            && (url.is_empty() || url.starts_with('/')))
    {
        url = format!("//{url}");
    }
    if !u.scheme.is_empty() {
        url = format!("{}:{}", u.scheme, url);
    }
    if !u.query.is_empty() {
        url.push('?');
        url.push_str(&u.query);
    }
    if !u.fragment.is_empty() {
        url.push('#');
        url.push_str(&u.fragment);
    }
    url
}

fn rewrite_query(url: &str, f: impl FnOnce(&mut Vec<(String, Vec<String>)>)) -> String {
    let mut parts = urlsplit(url);
    let mut q = parse_qs(&parts.query);
    f(&mut q);
    // Python: sorted(query_dict.items()) – keys are unique, so sorted by key.
    q.sort_by(|a, b| a.0.cmp(&b.0));
    parts.query = urlencode_doseq(&q);
    urlunsplit(&parts)
}

/// DRF `replace_query_param(url, key, val)`: sets `key=val`, sorts all parameters by
/// name and re-encodes them (`quote_plus`).
pub fn replace_query_param(url: &str, key: &str, val: &str) -> String {
    rewrite_query(url, |q| {
        if let Some(e) = q.iter_mut().find(|(k, _)| k == key) {
            e.1 = vec![val.to_string()];
        } else {
            q.push((key.to_string(), vec![val.to_string()]));
        }
    })
}

/// DRF `remove_query_param(url, key)`.
pub fn remove_query_param(url: &str, key: &str) -> String {
    rewrite_query(url, |q| q.retain(|(k, _)| k != key))
}

/// Django `escape_uri_path(path)` on an already decoded path.
pub fn escape_uri_path(path: &str) -> String {
    quote(path, "/:@&+$,-_.!~*'()")
}

/// Django `iri_to_uri(iri)`.
pub fn iri_to_uri(iri: &str) -> String {
    quote(iri, "/#%[]=:;$&()+,!?*@'~")
}

/// Django `repercent_broken_unicode` + decoding: turns the raw (percent-encoded)
/// path bytes of a request into the string Django sees as `request.path`.
/// Invalid UTF-8 bytes are replaced by `%XX` (like Django).
pub fn decode_request_path(raw_path: &str) -> String {
    let bytes = unquote_to_bytes(raw_path);
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            let mut out = String::new();
            let mut rest: &[u8] = &bytes;
            loop {
                match std::str::from_utf8(rest) {
                    Ok(s) => {
                        out.push_str(s);
                        break;
                    }
                    Err(err) => {
                        let (valid, after) = rest.split_at(err.valid_up_to());
                        out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                        let bad = err.error_len().unwrap_or(after.len());
                        for b in &after[..bad] {
                            out.push_str(&format!("%{b:02X}"));
                        }
                        rest = &after[bad..];
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_variants() {
        assert_eq!(quote_plus("a b/c+ä", ""), "a+b%2Fc%2B%C3%A4");
        assert_eq!(quote("a b~", ""), "a%20b~");
        assert_eq!(escape_uri_path("/api/tags/ä b/"), "/api/tags/%C3%A4%20b/");
        assert_eq!(iri_to_uri("a=1&b=%20x y"), "a=1&b=%20x%20y");
    }

    #[test]
    fn unquote_edge() {
        assert_eq!(unquote("%41%zz%"), "A%zz%");
        assert_eq!(unquote("%C3%A4"), "ä");
        assert_eq!(unquote("%FF"), "\u{FFFD}");
    }

    #[test]
    fn parse_qsl_like_python() {
        assert_eq!(
            parse_qsl("a=1&&b&c=x+y&a=2", true),
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "".into()),
                ("c".into(), "x y".into()),
                ("a".into(), "2".into())
            ]
        );
        assert_eq!(parse_qsl("b&c=", false), vec![]);
    }

    #[test]
    fn replace_and_remove() {
        // Sort by key, re-encode with quote_plus.
        let url = "http://h/api/documents/?title__icontains=a%20b&page=2&ordering=-created";
        assert_eq!(
            replace_query_param(url, "page", "3"),
            "http://h/api/documents/?ordering=-created&page=3&title__icontains=a+b"
        );
        assert_eq!(
            remove_query_param(url, "page"),
            "http://h/api/documents/?ordering=-created&title__icontains=a+b"
        );
        assert_eq!(remove_query_param("http://h/x/?page=2", "page"), "http://h/x/");
        assert_eq!(replace_query_param("http://h/x/", "page", "2"), "http://h/x/?page=2");
        // Multiple values stay in order.
        assert_eq!(
            replace_query_param("http://h/x/?tags__id__all=1&tags__id__all=2", "page", "2"),
            "http://h/x/?page=2&tags__id__all=1&tags__id__all=2"
        );
        // Blank values stay (keep_blank_values=True).
        assert_eq!(replace_query_param("http://h/x/?a&b=", "page", "2"), "http://h/x/?a=&b=&page=2");
    }

    #[test]
    fn broken_unicode_path() {
        assert_eq!(decode_request_path("/a%C3%A4"), "/aä");
        assert_eq!(decode_request_path("/a%FFb"), "/a%FFb");
        assert_eq!(escape_uri_path(&decode_request_path("/a%FFb")), "/a%25FFb");
    }
}
