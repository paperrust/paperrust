//! Request info (query, absolute URL, API version) and `StandardPagination`.
//!
//! upstream: src/paperless/views.py@v3.4.2 (StandardPagination), rest_framework/pagination.py@3.16.1
//! (PageNumberPagination, _positive_int), rest_framework/utils/urls.py (replace_query_param,
//! remove_query_param), rest_framework/versioning.py (AcceptHeaderVersioning),
//! django/core/paginator.py (Paginator.validate_number).
//! Local until pl-core provides pagination/versioning (CORE-REQUEST).

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use pl_core::AppState;
use serde_json::{Map, Value};

use crate::ctx;
use crate::error::ApiError;
use crate::pyval;

#[derive(Debug, Clone)]
pub struct ReqInfo {
    pub scheme: String,
    pub host: String,
    /// Path as received (already URL-encoded).
    pub path: String,
    pub query: Vec<(String, String)>,
    pub api_version: u32,
    /// `PAPERLESS_TIME_ZONE` (from the AppState).
    pub tz: chrono_tz::Tz,
}

/// `urllib.parse.parse_qs(qs, keep_blank_values=True)` as ordered pairs.
pub fn parse_qs(qs: &str) -> Vec<(String, String)> {
    let dec = |s: &str| {
        let s = s.replace('+', " ");
        percent_decode_str(&s).decode_utf8_lossy().into_owned()
    };
    qs.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (dec(k), dec(v)),
            None => (dec(p), String::new()),
        })
        .collect()
}

/// Characters that `quote_plus` does NOT encode: letters, digits, `_.-~`.
const QUOTE_PLUS: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');

fn quote_plus(s: &str) -> String {
    s.split(' ')
        .map(|part| utf8_percent_encode(part, QUOTE_PLUS).to_string())
        .collect::<Vec<_>>()
        .join("+")
}

/// `api_version` from the Accept header (AcceptHeaderVersioning, default 10).
pub fn parse_api_version(accept: Option<&str>) -> Result<u32, ApiError> {
    let Some(accept) = accept else { return Ok(10) };
    for part in accept.split(',') {
        let mut it = part.split(';');
        let mt = it.next().unwrap_or("").trim().to_ascii_lowercase();
        if !(mt == "application/json" || mt == "*/*" || mt == "application/*") {
            continue;
        }
        for p in it {
            if let Some((k, v)) = p.split_once('=')
                && k.trim() == "version"
            {
                {
                    let v = v.trim().trim_matches('"');
                    return match v {
                        "9" => Ok(9),
                        "10" => Ok(10),
                        _ => Err(ApiError::detail(
                            StatusCode::NOT_ACCEPTABLE,
                            "Invalid version in \"Accept\" header.",
                        )),
                    };
                }
            }
        }
        return Ok(10);
    }
    Ok(10)
}

impl ReqInfo {
    pub fn from_parts(parts: &Parts, core: &pl_core::Config) -> Result<Self, ApiError> {
        let tz = core.tz;
        let cfg = ctx::config(core);
        let headers = &parts.headers;
        let hv = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let use_xfh = core.use_x_forwarded_host;
        let host = if use_xfh { hv("x-forwarded-host") } else { None }
            .or_else(|| hv("host"))
            .or_else(|| parts.uri.authority().map(|a| a.to_string()))
            .unwrap_or_else(|| "localhost".into());
        let scheme = match &cfg.proxy_ssl_header {
            Some((name, value)) if hv(name).as_deref() == Some(value.as_str()) => "https",
            _ => "http",
        };
        Ok(ReqInfo {
            scheme: scheme.into(),
            host,
            path: parts.uri.path().to_string(),
            query: parts.uri.query().map(parse_qs).unwrap_or_default(),
            api_version: parse_api_version(hv("accept").as_deref())?,
            tz,
        })
    }

    /// `request.query_params.get(key)` (Django: last value).
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// `?full_perms=` → `get_boolean`.
    pub fn full_perms(&self) -> bool {
        self.param("full_perms")
            .map(|v| matches!(v.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true"))
            .unwrap_or(false)
    }

    fn url_with(&self, key: &str, value: Option<String>) -> String {
        // parse_qs → dict (order of values per key is kept), keys sorted
        let mut keys: Vec<String> = Vec::new();
        let mut vals: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for (k, v) in &self.query {
            if !vals.contains_key(k) {
                keys.push(k.clone());
            }
            vals.entry(k.clone()).or_default().push(v.clone());
        }
        match value {
            Some(v) => {
                if !vals.contains_key(key) {
                    keys.push(key.to_string());
                }
                vals.insert(key.to_string(), vec![v]);
            }
            None => {
                vals.remove(key);
                keys.retain(|k| k != key);
            }
        }
        keys.sort();
        let q: Vec<String> = keys
            .iter()
            .flat_map(|k| {
                vals.get(k)
                    .into_iter()
                    .flatten()
                    .map(move |v| format!("{}={}", quote_plus(k), quote_plus(v)))
            })
            .collect();
        let mut url = format!("{}://{}{}", self.scheme, self.host, self.path);
        if !q.is_empty() {
            url.push('?');
            url.push_str(&q.join("&"));
        }
        url
    }
}

impl FromRequestParts<AppState> for ReqInfo {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        ReqInfo::from_parts(parts, state.config())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub number: i64,
    pub size: i64,
    pub count: i64,
    pub num_pages: i64,
}

impl Page {
    pub fn offset(&self) -> i64 {
        (self.number - 1) * self.size
    }
}

const PAGE_SIZE: i64 = 25;
const MAX_PAGE_SIZE: i64 = 100_000;

/// `PageNumberPagination.get_page_size` + paginator check.
pub fn paginate(info: &ReqInfo, count: i64) -> Result<Page, ApiError> {
    let size = info
        .param("page_size")
        .and_then(pyval::parse_py_int)
        .filter(|n| *n > 0)
        .map(|n| n.min(MAX_PAGE_SIZE as i128) as i64)
        .unwrap_or(PAGE_SIZE);
    let num_pages = if count == 0 { 1 } else { (count.max(1) + size - 1) / size };
    let raw = info.param("page").filter(|s| !s.is_empty()).unwrap_or("1");
    let invalid = || ApiError::detail(StatusCode::NOT_FOUND, "Invalid page.");
    let number = if raw == "last" {
        num_pages
    } else {
        let n = pyval::parse_py_int(raw).ok_or_else(invalid)?;
        if n < 1 || n > num_pages as i128 {
            return Err(invalid());
        }
        n as i64
    };
    Ok(Page { number, size, count, num_pages })
}

/// `StandardPagination.get_paginated_response`.
pub fn paginated(info: &ReqInfo, page: &Page, results: Vec<Value>, all_ids: Option<Vec<i32>>) -> Value {
    let mut m = Map::new();
    m.insert("count".into(), Value::from(page.count));
    let next = if page.number < page.num_pages {
        Value::String(info.url_with("page", Some((page.number + 1).to_string())))
    } else {
        Value::Null
    };
    let previous = if page.number > 1 {
        let p = page.number - 1;
        Value::String(if p == 1 {
            info.url_with("page", None)
        } else {
            info.url_with("page", Some(p.to_string()))
        })
    } else {
        Value::Null
    };
    m.insert("next".into(), next);
    m.insert("previous".into(), previous);
    if info.api_version < 10 {
        m.insert(
            "all".into(),
            Value::Array(all_ids.unwrap_or_default().into_iter().map(Value::from).collect()),
        );
    }
    m.insert("results".into(), Value::Array(results));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(q: &str) -> ReqInfo {
        ReqInfo {
            scheme: "http".into(),
            host: "localhost:8000".into(),
            path: "/api/mail_rules/".into(),
            query: parse_qs(q),
            api_version: 10,
            tz: chrono_tz::UTC,
        }
    }

    #[test]
    fn pagination_links() {
        let i = info("page_size=2&b=x y&page=2");
        let p = paginate(&i, 5).unwrap();
        assert_eq!(p, Page { number: 2, size: 2, count: 5, num_pages: 3 });
        let v = paginated(&i, &p, vec![], None);
        assert_eq!(v["next"], "http://localhost:8000/api/mail_rules/?b=x+y&page=3&page_size=2");
        assert_eq!(v["previous"], "http://localhost:8000/api/mail_rules/?b=x+y&page_size=2");
        assert!(v.get("all").is_none());
        assert!(paginate(&info("page=4&page_size=2"), 5).is_err());
        assert!(paginate(&info("page=abc"), 5).is_err());
        assert_eq!(paginate(&info("page=last&page_size=2"), 5).unwrap().number, 3);
        assert_eq!(paginate(&info("page_size=0"), 5).unwrap().size, 25);
        assert_eq!(paginate(&info(""), 0).unwrap().num_pages, 1);
    }

    #[test]
    fn api_version() {
        assert_eq!(parse_api_version(None).unwrap(), 10);
        assert_eq!(parse_api_version(Some("application/json; version=9")).unwrap(), 9);
        assert_eq!(parse_api_version(Some("*/*")).unwrap(), 10);
        assert!(parse_api_version(Some("application/json; version=7")).is_err());
    }
}
