//! Thin glue to pl-core / pl-authz: error conversion, JSON request bodies, responses.
//!
//! upstream: rest_framework 3.18.3 (parsers.JSONParser, views.exception_handler)

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pl_authz::authn::AuthMethod;
use pl_authz::{AuthError, AuthUser};
use pl_core::client::{RequestUserSource, audit_actor};
use pl_core::{ApiError, Drf, RequestContext};
use serde_json::{Map, Value, json};

pub type HResult = Result<Response, ApiError>;

/// `AuthError` raised inside a handler (model/object permission checks) as DRF response.
pub fn auth(e: AuthError) -> ApiError {
    let status = e.status();
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        return ApiError::internal(e.detail());
    }
    if status == StatusCode::METHOD_NOT_ALLOWED {
        return ApiError::with_detail(status, e.detail());
    }
    ApiError::with_detail(status, e.detail())
}

/// 400 with a JSON body.
pub fn validation(v: Value) -> ApiError {
    ApiError::validation(v)
}

/// `{"non_field_errors": ["Invalid data. Expected a dictionary, but got <type>."]}`.
pub fn not_a_dict(v: &Value) -> ApiError {
    ApiError::validation(json!({
        "non_field_errors": [format!("Invalid data. Expected a dictionary, but got {}.", crate::text::py_type_name(v))]
    }))
}

pub fn ok(v: Value) -> Response {
    Drf::ok(v).into_response()
}

pub fn created(v: Value) -> Response {
    Drf::created(v).into_response()
}

pub fn no_content() -> Response {
    pl_core::json::drf_no_content()
}

/// `QueryDict.get` on the request.
pub fn param<'a>(ctx: &'a RequestContext, key: &str) -> Option<&'a str> {
    ctx.query.get(key)
}

/// `PassUserMixin`: `full_perms` query parameter.
pub fn full_perms(ctx: &RequestContext) -> bool {
    pl_authz::owned::parse_full_perms(ctx.query.get("full_perms"))
}

/// DRF `JSONParser` (the views only declare / effectively receive JSON).
/// Empty body → empty dict (DRF `request.data` of an empty request).
pub fn parse_json_body(ctx: &RequestContext, body: &[u8]) -> Result<Value, ApiError> {
    let ct = ctx.headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if body.is_empty() && main.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    if main != "application/json" {
        return Err(ApiError::unsupported_media_type(ct));
    }
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(body).map_err(|e| ApiError::json_body_parse_error(body, e))
}

/// Client address for django-auditlog (`REMOTE_ADDR`), if the server provides `ConnectInfo`.
#[derive(Clone, Debug, Default)]
pub struct ClientAddr(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientAddr(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0.ip().to_string()),
        ))
    }
}

/// Request data django-auditlog's middleware records (`remote_addr`/`remote_port` already
/// validated by `pl_core::client`: only a valid IP / port 0..=65535, else NULL).
#[derive(Clone, Debug, Default)]
pub struct AuditMeta {
    pub remote_addr: Option<String>,
    pub remote_port: Option<i32>,
    pub correlation_id: Option<String>,
    /// `actor_id`/`actor_email`: only a user Django's middlewares already know (session,
    /// auto-login, remote-user middleware); token/basic auth → `NULL` (`pl_core::client::audit_actor`).
    pub actor_id: Option<i64>,
    pub actor_email: Option<String>,
}

/// Which authenticator set the user, seen from `AuditlogMiddleware` (runs before DRF).
pub fn request_user_source(method: Option<AuthMethod>) -> RequestUserSource {
    if method.is_some_and(AuthMethod::is_django_request_user) {
        RequestUserSource::DjangoMiddleware
    } else {
        RequestUserSource::DrfAuthentication
    }
}

impl AuditMeta {
    pub fn new(ctx: &RequestContext, client: &ClientAddr, auth: &AuthUser) -> Self {
        let h = |n: &str| ctx.headers.get(n).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let u = auth.user();
        let (actor_id, actor_email) = audit_actor(u.id, &u.email, request_user_source(auth.context().method));
        let peer = client.0.as_deref().and_then(|a| a.parse::<std::net::IpAddr>().ok());
        AuditMeta {
            remote_addr: pl_core::client::audit_remote_addr(&ctx.headers, peer).map(|a| a.to_string()),
            remote_port: pl_core::client::audit_remote_port(&ctx.headers),
            correlation_id: h("x-correlation-id"),
            actor_id,
            actor_email,
        }
    }
}

/// Minimal multipart reader for text fields (file parts are ignored by these serializers).
fn multipart_text_fields(content_type: &str, body: &[u8]) -> Result<Vec<(String, String)>, ApiError> {
    let Some(boundary) = content_type
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_string())
    else {
        return Err(ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")));
    };
    let text = String::from_utf8_lossy(body);
    let delim = format!("--{boundary}");
    let mut out = Vec::new();
    for part in text.split(delim.as_str()).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let part = part.strip_prefix("\r\n").unwrap_or(part);
        let Some((head, value)) = part.split_once("\r\n\r\n") else { continue };
        let value = value.strip_suffix("\r\n").unwrap_or(value);
        let Some(disp) = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-disposition:")) else { continue };
        if disp.contains("filename=") {
            continue;
        }
        if let Some(n) = disp.split(';').map(str::trim).find_map(|p| p.strip_prefix("name=")) {
            out.push((n.trim_matches('"').to_string(), value.to_string()));
        }
    }
    Ok(out)
}

/// `request.data` for the object serializers with DRF's default parsers (JSON, form, multipart)
/// – form/multipart already reduced to what `Field.get_value` yields for HTML input
/// (docs/STANDALONE-GAPS.md §4.4):
/// * last value per key (`QueryDict.__getitem__`),
/// * `BooleanField` missing on create/PUT → `false` (`default_empty_html`), `""` → skipped,
/// * `""` for `ChoiceField`/`CharField` without `allow_blank` and not required → skipped,
/// * `""` for `allow_null` relations (`owner`, `parent`) → `null`.
pub fn parse_body(ctx: &RequestContext, body: &[u8], kind: crate::model::Kind, partial: bool) -> Result<Value, ApiError> {
    let ct = ctx.headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let pairs: Vec<(String, String)> = match main.as_str() {
        "application/x-www-form-urlencoded" => pl_core::QueryParams::parse(&String::from_utf8_lossy(body)).pairs().to_vec(),
        "multipart/form-data" => {
            if body.is_empty() {
                Vec::new()
            } else {
                multipart_text_fields(ct, body)?
            }
        }
        _ => return parse_json_body(ctx, body),
    };
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k, Value::String(v));
    }
    let bools: &[&str] = if kind == crate::model::Kind::Tag { &["is_insensitive", "is_inbox_tag"] } else { &["is_insensitive"] };
    for b in bools {
        match m.get(*b) {
            None if !partial => {
                m.insert((*b).to_string(), Value::Bool(false));
            }
            Some(Value::String(s)) if s.is_empty() => {
                m.remove(*b);
            }
            _ => {}
        }
    }
    for k in ["matching_algorithm", "color"] {
        if m.get(k) == Some(&Value::String(String::new())) {
            m.remove(k);
        }
    }
    for k in ["owner", "parent"] {
        if m.get(k) == Some(&Value::String(String::new())) {
            m.insert(k.to_string(), Value::Null);
        }
    }
    Ok(Value::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AuditlogMiddleware` sees only users set by Django middlewares (auditlog/middleware.py).
    #[test]
    fn audit_actor_source() {
        use RequestUserSource::*;
        assert_eq!(request_user_source(Some(AuthMethod::Session)), DjangoMiddleware);
        assert_eq!(request_user_source(Some(AuthMethod::AutoLogin)), DjangoMiddleware);
        assert_eq!(request_user_source(Some(AuthMethod::RemoteUser)), DjangoMiddleware);
        assert_eq!(request_user_source(Some(AuthMethod::Token)), DrfAuthentication);
        assert_eq!(request_user_source(Some(AuthMethod::Basic)), DrfAuthentication);
        assert_eq!(request_user_source(Some(AuthMethod::DebugAngular)), DrfAuthentication);
        assert_eq!(request_user_source(None), DrfAuthentication);
    }
}
