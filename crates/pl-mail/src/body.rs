//! Request body like DRF `request.data` (JSON only, see Known differences).
//!
//! upstream: rest_framework/request.py@3.16.1 (Request._parse), rest_framework/parsers.py (JSONParser).

use axum::http::{HeaderMap, StatusCode, header};
use serde_json::{Map, Value};

use crate::error::ApiError;

/// `request.data`: empty body → `{}`; JSON → value; other content type → 415.
pub fn parse_body(headers: &HeaderMap, body: &[u8]) -> Result<Value, ApiError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let mt = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if mt != "application/json" {
        let shown = if ct.is_empty() { "".to_string() } else { ct.clone() };
        return Err(ApiError::detail(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("Unsupported media type \"{shown}\" in request."),
        ));
    }
    serde_json::from_slice::<Value>(body).map_err(|e| {
        ApiError::detail(StatusCode::BAD_REQUEST, format!("JSON parse error - {e}"))
    })
}

/// Body as object, otherwise DRF `non_field_errors` (Serializer.to_internal_value).
pub fn as_object(v: Value) -> Result<Map<String, Value>, ApiError> {
    match v {
        Value::Object(m) => Ok(m),
        other => Err(ApiError::bad_request(crate::drf::non_dict_error(&other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn body_rules() {
        let mut h = HeaderMap::new();
        assert_eq!(parse_body(&h, b"").unwrap(), serde_json::json!({}));
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
        assert_eq!(parse_body(&h, b"{\"a\":1}").unwrap(), serde_json::json!({"a": 1}));
        assert!(parse_body(&h, b"{").is_err());
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        match parse_body(&h, b"x") {
            Err(ApiError::Json(s, v)) => {
                assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
                assert_eq!(v["detail"], "Unsupported media type \"text/plain\" in request.");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
