//! Extractors and helpers: login (shared with v1 via `pl-authz`), JSON body, `If-Match`.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_authz::{AuthError, MaybeUser};
use pl_domain::{DomainError, Principal};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::V2Error;

/// `pl-authz` error → v2 (CSRF errors and blocked users → 403, wrong credentials → 401).
pub fn auth_error(e: AuthError) -> V2Error {
    match e.status() {
        StatusCode::UNAUTHORIZED => V2Error(DomainError::Unauthenticated),
        StatusCode::FORBIDDEN => V2Error(DomainError::Forbidden(e.detail())),
        _ => V2Error(DomainError::Internal(e.detail())),
    }
}

fn peer(parts: &Parts) -> Option<SocketAddr> {
    parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0)
}

/// Logged-in user (session+CSRF, `Token`, `Bearer prt_…`, Basic, remote user – like v1).
pub struct Caller(pub Principal);

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = V2Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let m = MaybeUser::from_request_parts(parts, state).await.map_err(auth_error)?;
        pl_store::principal(m.0.clone(), peer(parts)).map(Caller).ok_or(V2Error(DomainError::Unauthenticated))
    }
}

/// Optionally logged in (`/server`).
pub struct OptCaller(pub Option<Principal>);

impl<S: Send + Sync> FromRequestParts<S> for OptCaller {
    type Rejection = V2Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let m = MaybeUser::from_request_parts(parts, state).await.map_err(auth_error)?;
        Ok(OptCaller(pl_store::principal(m.0.clone(), peer(parts))))
    }
}

/// JSON body with v2 errors.
pub struct V2Json<T>(pub T);

/// Largest accepted JSON body (1 MiB).
const MAX_JSON: usize = 1024 * 1024;

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for V2Json<T> {
    type Rejection = V2Error;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let ct = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        if !(main == "application/json" || main.ends_with("+json")) {
            return Err(V2Error(DomainError::invalid("Content-Type must be application/json.")));
        }
        let body = Bytes::from_request(req, state)
            .await
            .map_err(|_| V2Error(DomainError::invalid("Could not read the request body.")))?;
        if body.len() > MAX_JSON {
            return Err(V2Error(DomainError::invalid("Request body too large.")));
        }
        serde_json::from_slice(&body)
            .map(V2Json)
            .map_err(|e| V2Error(DomainError::invalid(format!("Invalid JSON: {e}"))))
    }
}

/// `If-Match` → expected `rev` (`None` = no header or `*`).
pub fn if_match(h: &HeaderMap) -> Result<Option<i64>, V2Error> {
    let Some(v) = h.get(header::IF_MATCH) else { return Ok(None) };
    let s = v.to_str().unwrap_or("").trim();
    if s == "*" {
        return Ok(None);
    }
    let s = s.strip_prefix("W/").unwrap_or(s).trim_matches('"');
    s.parse::<i64>()
        .map(Some)
        .map_err(|_| V2Error(DomainError::field("If-Match", "Expected the object's rev, e.g. \"7\".")))
}

/// JSON response with optional ETag (`"<rev>"`).
pub fn json_with_rev<T: Serialize>(status: StatusCode, v: &T, rev: Option<i64>) -> Response {
    let mut r = (status, axum::Json(v)).into_response();
    if let Some(rev) = rev
        && let Ok(h) = HeaderValue::from_str(&format!("\"{rev}\""))
    {
        r.headers_mut().insert(header::ETAG, h);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    r
}

pub fn json<T: Serialize>(v: &T) -> Response {
    json_with_rev(StatusCode::OK, v, None)
}

/// `true`/`1`/`yes`.
pub fn truthy(s: Option<&str>) -> bool {
    matches!(s.map(|x| x.trim().to_ascii_lowercase()).as_deref(), Some("true" | "1" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_match_forms() {
        let mut h = HeaderMap::new();
        assert_eq!(if_match(&h).unwrap(), None);
        h.insert(header::IF_MATCH, HeaderValue::from_static("\"7\""));
        assert_eq!(if_match(&h).unwrap(), Some(7));
        h.insert(header::IF_MATCH, HeaderValue::from_static("W/\"8\""));
        assert_eq!(if_match(&h).unwrap(), Some(8));
        h.insert(header::IF_MATCH, HeaderValue::from_static("*"));
        assert_eq!(if_match(&h).unwrap(), None);
        h.insert(header::IF_MATCH, HeaderValue::from_static("\"x\""));
        assert!(if_match(&h).is_err());
    }
}
