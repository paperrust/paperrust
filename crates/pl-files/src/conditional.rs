// upstream: django/views/decorators/http.py (condition), django/utils/cache.py
//           (get_conditional_response, _if_*_passes, _not_modified, _precondition_failed),
//           django/utils/http.py (parse_etags, quote_etag, parse_http_date, http_date),
//           django/views/decorators/cache.py (cache_control(no_cache=True))

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// `ETAG_MATCH`: `(?:W/)?"[^"]*"` over the whole (stripped) string.
fn etag_match(s: &str) -> bool {
    let rest = s.strip_prefix("W/").unwrap_or(s);
    rest.len() >= 2 && rest.starts_with('"') && rest.ends_with('"') && !rest[1..rest.len() - 1].contains('"')
}

pub fn quote_etag(s: &str) -> String {
    if etag_match(s) { s.to_string() } else { format!("\"{s}\"") }
}

pub fn parse_etags(s: &str) -> Vec<String> {
    if s.trim() == "*" {
        return vec!["*".to_string()];
    }
    s.split(',').map(|p| p.trim()).filter(|p| etag_match(p)).map(|p| p.to_string()).collect()
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn parse_time(t: &str) -> Option<(u32, u32, u32)> {
    let b: Vec<&str> = t.split(':').collect();
    if b.len() != 3 || b.iter().any(|x| x.len() != 2 || !digits(x)) {
        return None;
    }
    Some((b[0].parse().ok()?, b[1].parse().ok()?, b[2].parse().ok()?))
}

/// `parse_http_date_safe` – RFC 1123, RFC 850 and asctime. Returns epoch seconds.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let (year, mon, day, time) = if let Some(rest) = s.strip_suffix(" GMT") {
        // RFC1123: "\w{3}, DD Mon YYYY HH:MM:SS"  RFC850: "\w{6,9}, DD-Mon-YY HH:MM:SS"
        let (wd, rest) = rest.split_once(", ")?;
        if !wd.chars().all(is_word) {
            return None;
        }
        let wl = wd.chars().count();
        let parts: Vec<&str> = rest.split(' ').collect();
        if wl == 3 && parts.len() == 4 {
            let (d, m, y) = (parts[0], parts[1], parts[2]);
            if d.len() != 2 || !digits(d) || y.len() != 4 || !digits(y) || m.chars().count() != 3 {
                return None;
            }
            (y.parse::<i32>().ok()?, m.to_string(), d.parse::<u32>().ok()?, parse_time(parts[3])?)
        } else if (6..=9).contains(&wl) && parts.len() == 2 {
            let dmy: Vec<&str> = parts[0].split('-').collect();
            if dmy.len() != 3 {
                return None;
            }
            let (d, m, y) = (dmy[0], dmy[1], dmy[2]);
            if d.len() != 2 || !digits(d) || y.len() != 2 || !digits(y) || m.chars().count() != 3 {
                return None;
            }
            let mut year: i32 = y.parse().ok()?;
            let current_year = Utc::now().year();
            let current_century = current_year - current_year % 100;
            if year - (current_year % 100) > 50 {
                year += current_century - 100;
            } else {
                year += current_century;
            }
            (year, m.to_string(), d.parse::<u32>().ok()?, parse_time(parts[1])?)
        } else {
            return None;
        }
    } else {
        // asctime: "\w{3} Mon [ D]D HH:MM:SS YYYY"
        if s.len() < 24 || !s.is_ascii() {
            return None;
        }
        let wd = &s[0..3];
        let m = &s[4..7];
        let d = &s[8..10];
        let t = &s[11..19];
        let y = &s[20..];
        if !wd.chars().all(is_word)
            || &s[3..4] != " "
            || &s[7..8] != " "
            || &s[10..11] != " "
            || &s[19..20] != " "
            || y.len() != 4
            || !digits(y)
        {
            return None;
        }
        let d0 = d.as_bytes()[0];
        if !(d0 == b' ' || d0.is_ascii_digit()) || !d.as_bytes()[1].is_ascii_digit() {
            return None;
        }
        (y.parse().ok()?, m.to_string(), d.trim().parse().ok()?, parse_time(t)?)
    };
    let month = MONTHS.iter().position(|x| *x == mon.to_lowercase())? as u32 + 1;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let t = NaiveTime::from_hms_opt(time.0, time.1, time.2)?;
    Some(Utc.from_utc_datetime(&NaiveDateTime::new(date, t)).timestamp())
}

/// `http_date(epoch_seconds)` – RFC 1123 in GMT.
pub fn http_date(ts: i64) -> String {
    match DateTime::<Utc>::from_timestamp(ts, 0) {
        Some(d) => d.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        None => String::new(),
    }
}

/// Outcome of `get_conditional_response` before the view ran.
#[derive(Debug, PartialEq, Eq)]
pub enum Precondition {
    Proceed,
    NotModified,
    Failed,
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> &'a str {
    h.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
}

pub fn evaluate(method: &Method, h: &HeaderMap, etag: Option<&str>, last_modified: Option<i64>) -> Precondition {
    let safe = method == Method::GET || method == Method::HEAD;
    let if_match = parse_etags(header_str(h, "if-match"));
    let ius = h.get("if-unmodified-since").and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()).and_then(parse_http_date);
    let if_none_match = parse_etags(header_str(h, "if-none-match"));
    let ims = h.get("if-modified-since").and_then(|v| v.to_str().ok()).filter(|s| !s.is_empty()).and_then(parse_http_date);

    // Step 1: If-Match
    if !if_match.is_empty() {
        let passes = match etag {
            None | Some("") => false,
            Some(_) if if_match == ["*"] => true,
            Some(t) if t.starts_with("W/") => false,
            Some(t) => if_match.iter().any(|e| e == t),
        };
        if !passes {
            return Precondition::Failed;
        }
    }
    // Step 2: If-Unmodified-Since
    if if_match.is_empty()
        && let Some(ius) = ius
        && ius != 0
    {
        let passes = matches!(last_modified, Some(lm) if lm != 0 && lm <= ius);
        if !passes {
            return Precondition::Failed;
        }
    }
    // Step 3: If-None-Match
    if !if_none_match.is_empty() {
        let passes = match etag {
            None | Some("") => true,
            Some(_) if if_none_match == ["*"] => false,
            Some(t) => {
                let t = t.strip_prefix("W/").unwrap_or(t);
                !if_none_match.iter().any(|e| e.strip_prefix("W/").unwrap_or(e) == t)
            }
        };
        if !passes {
            return if safe { Precondition::NotModified } else { Precondition::Failed };
        }
    }
    // Step 4: If-Modified-Since
    if if_none_match.is_empty()
        && safe
        && let Some(ims) = ims
        && ims != 0
    {
        let passes = match last_modified {
            None | Some(0) => true,
            Some(lm) => lm > ims,
        };
        if !passes {
            return Precondition::NotModified;
        }
    }
    Precondition::Proceed
}

pub fn precondition_response(p: &Precondition) -> Option<Response> {
    match p {
        Precondition::Proceed => None,
        Precondition::NotModified => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::NOT_MODIFIED;
            Some(r)
        }
        Precondition::Failed => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::PRECONDITION_FAILED;
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(crate::compat::HTML));
            Some(r)
        }
    }
}

/// `condition()` post-processing plus `cache_control(no_cache=True)`.
///
/// Unlike Django, ETag/Last-Modified are only attached to 2xx and 304 responses: error
/// responses (403, 404, 412, ...) never carry validators (contract pl-files, Known difference 7).
pub fn finish(mut r: Response, method: &Method, etag: Option<&str>, last_modified: Option<i64>) -> Response {
    let status = r.status();
    let attach_validators = status.is_success() || status == StatusCode::NOT_MODIFIED;
    let h = r.headers_mut();
    if attach_validators && (method == Method::GET || method == Method::HEAD) {
        if let Some(lm) = last_modified.filter(|v| *v != 0)
            && !h.contains_key(header::LAST_MODIFIED)
            && let Ok(v) = HeaderValue::from_str(&http_date(lm))
        {
            h.insert(header::LAST_MODIFIED, v);
        }
        if let Some(e) = etag.filter(|e| !e.is_empty())
            && !h.contains_key(header::ETAG)
            && let Ok(v) = HeaderValue::from_str(e)
        {
            h.insert(header::ETAG, v);
        }
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etags() {
        assert_eq!(quote_etag("abc"), "\"abc\"");
        assert_eq!(quote_etag("\"abc\""), "\"abc\"");
        assert_eq!(quote_etag(""), "\"\"");
        assert_eq!(parse_etags(" \"a\", W/\"b\", c"), vec!["\"a\"", "W/\"b\""]);
        assert_eq!(parse_etags("*"), vec!["*"]);
    }

    #[test]
    fn dates() {
        let ts = parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(http_date(ts), "Wed, 21 Oct 2015 07:28:00 GMT");
        assert_eq!(parse_http_date("Wednesday, 21-Oct-15 07:28:00 GMT"), Some(ts));
        assert_eq!(parse_http_date("Wed Oct 21 07:28:00 2015"), Some(ts));
        assert_eq!(parse_http_date("garbage"), None);
    }

    #[test]
    fn conditions() {
        let mut h = HeaderMap::new();
        h.insert("if-none-match", HeaderValue::from_static("\"v2\""));
        assert_eq!(evaluate(&Method::GET, &h, Some("\"v2\""), None), Precondition::NotModified);
        assert_eq!(evaluate(&Method::GET, &h, Some("\"v1\""), None), Precondition::Proceed);
        assert_eq!(evaluate(&Method::GET, &h, None, None), Precondition::Proceed);
        let mut h = HeaderMap::new();
        h.insert("if-modified-since", HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
        let ts = parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(evaluate(&Method::GET, &h, None, Some(ts)), Precondition::NotModified);
        assert_eq!(evaluate(&Method::GET, &h, None, Some(ts + 1)), Precondition::Proceed);
        let mut h = HeaderMap::new();
        h.insert("if-match", HeaderValue::from_static("\"x\""));
        assert_eq!(evaluate(&Method::GET, &h, Some("\"y\""), None), Precondition::Failed);
    }

    /// Regression: validators must never be attached to error responses.
    #[test]
    fn finish_skips_validators_on_errors() {
        for status in [StatusCode::FORBIDDEN, StatusCode::NOT_FOUND, StatusCode::PRECONDITION_FAILED, StatusCode::INTERNAL_SERVER_ERROR] {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = status;
            let r = finish(r, &Method::GET, Some("\"abc\""), Some(1_600_000_000));
            assert!(r.headers().get(header::ETAG).is_none(), "{status}");
            assert!(r.headers().get(header::LAST_MODIFIED).is_none(), "{status}");
            assert_eq!(r.headers().get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()), Some("no-cache"));
        }
        for status in [StatusCode::OK, StatusCode::NOT_MODIFIED] {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = status;
            let r = finish(r, &Method::GET, Some("\"abc\""), Some(1_600_000_000));
            assert!(r.headers().get(header::ETAG).is_some(), "{status}");
            assert!(r.headers().get(header::LAST_MODIFIED).is_some(), "{status}");
        }
    }
}
