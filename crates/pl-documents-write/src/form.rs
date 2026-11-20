// upstream: rest_framework 3.18.3 parsers.py (FormParser, MultiPartParser), request.py
//           (Request.data: QueryDict), fields.py (Field.get_value, ListField.get_value),
//           utils/html.py (is_html_input, parse_html_list)
//
//! Form bodies (`application/x-www-form-urlencoded`, `multipart/form-data`) as DRF's default
//! parsers deliver them in `request.data` (a `QueryDict`). Text fields only: file parts are
//! skipped (none of the views that use this module reads files).

use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Multipart};
use axum::http::{HeaderMap, Request, header};
use serde_json::{Map, Value};

use crate::error::{ApiError, ApiResult};

/// `QueryDict` of a form body: key/value pairs in body order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormData(pub Vec<(String, String)>);

impl FormData {
    /// `QueryDict.__getitem__` / `.get`: the **last** value of the key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// `QueryDict.getlist`.
    pub fn get_list(&self, key: &str) -> Vec<&str> {
        self.0.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    /// `ListField.get_value` for HTML input: `getlist(name)`, else `parse_html_list(prefix=name)`;
    /// `None` = `empty` (field not given).
    pub fn list_value(&self, name: &str) -> Option<Value> {
        let direct = self.get_list(name);
        if !direct.is_empty() {
            return Some(Value::Array(direct.into_iter().map(|s| Value::String(s.to_owned())).collect()));
        }
        parse_html_list(self, name)
    }
}

/// Media type (lowercase, without parameters) of the request.
pub fn media_type(headers: &HeaderMap) -> String {
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Is the body a form body (DRF `FormParser`/`MultiPartParser`)?
pub fn is_form(headers: &HeaderMap) -> bool {
    matches!(media_type(headers).as_str(), "application/x-www-form-urlencoded" | "multipart/form-data")
}

/// Parse a form body like DRF. `Ok(None)`: not a form content type (caller parses JSON).
/// An empty body with a form type is an empty `QueryDict` (`Request._parse` without stream).
pub async fn parse(headers: &HeaderMap, body: &Bytes) -> ApiResult<Option<FormData>> {
    match media_type(headers).as_str() {
        "application/x-www-form-urlencoded" => {
            Ok(Some(FormData(pl_core::QueryParams::parse(&String::from_utf8_lossy(body)).pairs().to_vec())))
        }
        "multipart/form-data" => {
            if body.is_empty() {
                return Ok(Some(FormData::default()));
            }
            let mut req = Request::new(Body::from(body.clone()));
            *req.headers_mut() = headers.clone();
            let mut mp = Multipart::from_request(req, &()).await.map_err(|_| {
                ApiError::Core(pl_core::ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))
            })?;
            let mut out = Vec::new();
            loop {
                match mp.next_field().await {
                    Ok(Some(f)) => {
                        let name = f.name().unwrap_or("").to_string();
                        if f.file_name().is_some() {
                            continue;
                        }
                        let text = f.text().await.unwrap_or_default();
                        out.push((name, text));
                    }
                    Ok(None) => break,
                    Err(_) => {
                        return Err(ApiError::Core(pl_core::ApiError::parse_error(Some(
                            "Multipart form parse error - Invalid multipart body",
                        ))));
                    }
                }
            }
            Ok(Some(FormData(out)))
        }
        _ => Ok(None),
    }
}

/// `rest_framework.utils.html.parse_html_list(dictionary, prefix)`: `prefix[0]=a&prefix[1]=b` →
/// `["a", "b"]` (sorted by index, last value per key); `prefix[0]key=v` → `{"key": "v"}`.
/// `None` if no key matches (`default=empty`).
pub fn parse_html_list(data: &FormData, prefix: &str) -> Option<Value> {
    let mut items: std::collections::BTreeMap<u64, Value> = std::collections::BTreeMap::new();
    // QueryDict.items(): every key once (first position), last value
    let mut seen: Vec<&str> = Vec::new();
    for (k, _) in &data.0 {
        if seen.contains(&k.as_str()) {
            continue;
        }
        seen.push(k.as_str());
        let Some(rest) = k.strip_prefix(prefix).and_then(|r| r.strip_prefix('[')) else { continue };
        let Some((idx, key)) = rest.split_once(']') else { continue };
        if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(index) = idx.parse::<u64>() else { continue };
        let value = Value::String(data.get(k).unwrap_or_default().to_owned());
        if key.is_empty() {
            items.insert(index, value);
        } else {
            match items.get_mut(&index) {
                Some(Value::Object(m)) => {
                    m.insert(key.to_owned(), value);
                }
                _ => {
                    let mut m = Map::new();
                    m.insert(key.to_owned(), value);
                    items.insert(index, Value::Object(m));
                }
            }
        }
    }
    if items.is_empty() { None } else { Some(Value::Array(items.into_values().collect())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn fd(pairs: &[(&str, &str)]) -> FormData {
        FormData(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }

    #[test]
    fn querydict_semantics() {
        let f = fd(&[("a", "1"), ("b", "x"), ("a", "2")]);
        assert_eq!(f.get("a"), Some("2"));
        assert_eq!(f.get_list("a"), vec!["1", "2"]);
        assert_eq!(f.get("c"), None);
        assert_eq!(f.list_value("a"), Some(json!(["1", "2"])));
        assert_eq!(f.list_value("c"), None);
    }

    #[test]
    fn html_list() {
        let f = fd(&[("documents[1]", "5"), ("documents[0]", "3"), ("documents[x]", "9"), ("other", "1")]);
        assert_eq!(f.list_value("documents"), Some(json!(["3", "5"])));
        let f = fd(&[("cf[0]field", "1"), ("cf[0]value", "a")]);
        assert_eq!(parse_html_list(&f, "cf"), Some(json!([{"field": "1", "value": "a"}])));
        assert_eq!(parse_html_list(&f, "documents"), None);
    }

    #[tokio::test]
    async fn parses_urlencoded_and_multipart() {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-www-form-urlencoded"));
        let f = parse(&h, &Bytes::from_static(b"note=hello+world&x=%C3%A4")).await.ok().flatten();
        assert_eq!(f, Some(fd(&[("note", "hello world"), ("x", "ä")])));

        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("multipart/form-data; boundary=XX"));
        let body = "--XX\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\nrestore\r\n--XX\r\nContent-Disposition: form-data; name=\"f\"; filename=\"a.txt\"\r\n\r\nfile\r\n--XX--\r\n";
        let f = parse(&h, &Bytes::from(body)).await.ok().flatten();
        assert_eq!(f, Some(fd(&[("action", "restore")])));
        assert_eq!(parse(&h, &Bytes::new()).await.ok().flatten(), Some(FormData::default()));

        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert_eq!(parse(&h, &Bytes::from_static(b"{}")).await.ok().flatten(), None);
    }
}
