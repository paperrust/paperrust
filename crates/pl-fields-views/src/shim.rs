//! Adapter to `pl-core` / `pl-authz`: request context, user, errors, pagination, date.
//! Keeps the handlers of this crate lean; DRF behaviour comes from the core crates.
//!
//! upstream: rest_framework/views.py (`initial`, `finalize_response`), paperless/views.py
//! (StandardPagination), django.utils.crypto.get_random_string.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use axum::extract::FromRequestParts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_authz::{AuthError, AuthMethod, AuthUser};
use pl_core::pagination::{PageInfo, PageRequest, paginated_body};
use pl_core::{AppState, RequestContext};
use serde_json::{Map, Value, json};

// ---------------------------------------------------------------------------
// Settings that are not (yet) in pl_core::Config
// ---------------------------------------------------------------------------

/// `PAPERLESS_AUDIT_LOG_ENABLED` (default `true`, `get_bool_from_env`).
pub fn audit_log_enabled() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| {
        let v = pl_core::config::var("PAPERLESS_AUDIT_LOG_ENABLED").unwrap_or_else(|| "true".into());
        matches!(v.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true")
    })
}

static TZ: OnceLock<Tz> = OnceLock::new();

/// `TIME_ZONE` from `AppState` (set on the first request).
pub fn tz() -> Tz {
    TZ.get().copied().unwrap_or(chrono_tz::UTC)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Marker in the response extensions: the server is to pass the (buffered) request on unchanged to
/// Django. Only set before anything was written.
/// Shared mechanism from `pl_core::markers`.
pub use pl_core::DelegateToDjango;

#[derive(Debug)]
pub enum ApiError {
    /// Status + JSON body (DRF `Response(data, status)`)
    Json(StatusCode, Value),
    /// Errors from pl-core
    Core(pl_core::ApiError),
    /// Errors from pl-authz (401/403/404/405)
    Auth(AuthError),
    /// Django 500 without DEBUG
    ServerError,
}

impl ApiError {
    pub fn detail(status: StatusCode, msg: &str) -> Self {
        ApiError::Json(status, json!({ "detail": msg }))
    }
    pub fn forbidden() -> Self {
        Self::detail(
            StatusCode::FORBIDDEN,
            "You do not have permission to perform this action.",
        )
    }
    pub fn insufficient() -> Self {
        Self::detail(StatusCode::FORBIDDEN, "Insufficient permissions.")
    }
    /// `get_object_or_404` with a valid pk: `No <Model> matches the given query.`
    pub fn not_found(model: &str) -> Self {
        Self::detail(
            StatusCode::NOT_FOUND,
            &format!("No {model} matches the given query."),
        )
    }
    /// `Http404` without text (invalid pk, object permission): `Not found.`
    pub fn not_found_plain() -> Self {
        Self::detail(StatusCode::NOT_FOUND, "Not found.")
    }
    pub fn bad_request(v: Value) -> Self {
        ApiError::Json(StatusCode::BAD_REQUEST, v)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error in pl-fields-views");
        ApiError::ServerError
    }
}

impl From<pl_core::ApiError> for ApiError {
    fn from(e: pl_core::ApiError) -> Self {
        ApiError::Core(e)
    }
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        ApiError::Auth(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Json(status, v) => json_response(status, &v),
            ApiError::Core(e) => e.into_response(),
            ApiError::Auth(e) => e.into_response(),
            ApiError::ServerError => {
                pl_core::ApiError::internal("pl-fields-views server error").into_response()
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// DRF `JSONRenderer` (via pl-core: compact, Python float representation).
pub fn json_response(status: StatusCode, v: &Value) -> Response {
    pl_core::json::drf_response(status, v)
}

/// DRF `APIView.finalize_response`: `Allow` and `Vary: Accept` on every response.
pub fn finish(allow: &'static str, res: ApiResult<Response>) -> Response {
    let mut r = match res {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    // The Django 500 page itself (unhandled exception) arises outside DRF: no
    // Allow/Vary: Accept (W2 parity run 2026-11-08, w2.cf.create.validation / sl.create.validation)
    if r.extensions().get::<DelegateToDjango>().is_none()
        && r.extensions().get::<pl_core::DjangoHandlerResponse>().is_none()
    {
        r.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static(allow));
        r.headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Accept"));
    }
    r
}

pub fn no_content() -> Response {
    pl_core::json::drf_no_content()
}

// ---------------------------------------------------------------------------
// User (from pl-authz)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct User {
    pub inner: Arc<pl_authz::User>,
    pub id: i64,
    pub email: String,
    pub is_superuser: bool,
    pub is_active: bool,
    /// true on session login (only then does the AuditlogMiddleware set the actor).
    pub via_session: bool,
}

impl User {
    pub fn from_authz(u: &pl_authz::User, method: Option<AuthMethod>) -> Self {
        User {
            inner: Arc::new(u.clone()),
            id: u.id,
            email: u.email.clone(),
            is_superuser: u.is_superuser,
            is_active: u.is_active,
            via_session: matches!(method, Some(AuthMethod::Session)),
        }
    }
    /// Django `User.has_perm("documents.<codename>")`.
    pub fn has_perm(&self, perm: &str) -> bool {
        self.inner.has_perm(perm)
    }
}

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

pub struct Ctx {
    pub user: User,
    pub auth: AuthUser,
    pub rc: RequestContext,
    pub version: u32,
    pub headers: HeaderMap,
    /// Query parameters in order (Django `QueryDict.get` = last value)
    pub query: Vec<(String, String)>,
    pub remote_ip: Option<String>,
    pub share_link_bundle_dir: PathBuf,
}

impl Ctx {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.rc.query.get(name)
    }
    /// `PassUserMixin`: `full_perms` per `get_boolean`.
    pub fn full_perms(&self) -> bool {
        pl_authz::owned::parse_full_perms(self.param("full_perms"))
    }
    pub fn authz_user(&self) -> &pl_authz::User {
        self.auth.user()
    }
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // DRF `initial()`: negotiation/version before authentication/permissions
        let rc = RequestContext::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let auth = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let _ = TZ.set(state.inner.tz);
        let user = User::from_authz(auth.user(), auth.context().method);
        let remote_ip = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string());
        Ok(Ctx {
            user,
            version: u32::from(rc.version().as_u16()),
            headers: parts.headers.clone(),
            query: rc.query.pairs().to_vec(),
            rc,
            auth,
            remote_ip,
            share_link_bundle_dir: state.inner.config.share_link_bundle_dir.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Request body (DRF parser: JSON, form)
// ---------------------------------------------------------------------------

/// Returns the body as a JSON value. Form data becomes an object of strings
/// (repeated keys additionally as a list under `__list__<key>` for ListFields).
pub fn parse_body(headers: &HeaderMap, body: &[u8]) -> ApiResult<Value> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let media = ct.split(';').next().unwrap_or("").trim().to_lowercase();
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match media.as_str() {
        "application/json" => match serde_json::from_slice::<Value>(body) {
            Ok(v) => Ok(v),
            // Python's json message (W2 parity run: serde_json's texts differed)
            Err(e) => Err(ApiError::Core(pl_core::ApiError::json_parse_error(
                &crate::pyjson::decode_error(body).unwrap_or_else(|| e.to_string()),
            ))),
        },
        "application/x-www-form-urlencoded" => {
            let mut map = Map::new();
            let mut lists: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
            for (k, v) in url::form_urlencoded::parse(body) {
                lists
                    .entry(k.to_string())
                    .or_default()
                    .push(Value::String(v.to_string()));
                map.insert(k.into_owned(), Value::String(v.into_owned()));
            }
            for (k, v) in lists {
                map.insert(format!("__list__{k}"), Value::Array(v));
            }
            Ok(Value::Object(map))
        }
        _ => Err(ApiError::Core(pl_core::ApiError::unsupported_media_type(
            &ct,
        ))),
    }
}

/// DRF `Serializer.to_internal_value`: body must be an object.
pub fn require_object(v: Value) -> ApiResult<Map<String, Value>> {
    match v {
        Value::Object(m) => Ok(m),
        other => Err(ApiError::bad_request(json!({
            "non_field_errors": [format!(
                "Invalid data. Expected a dictionary, but got {}.",
                py_type_name(&other)
            )]
        }))),
    }
}

/// Python type name of a JSON value (for DRF error texts).
pub fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

// ---------------------------------------------------------------------------
// Date/time (pl-core)
// ---------------------------------------------------------------------------

pub fn fmt_dt(dt: &DateTime<Utc>) -> String {
    pl_core::datetime::format_datetime(dt, &tz())
}

pub fn fmt_dt_opt(dt: &Option<DateTime<Utc>>) -> Value {
    pl_core::datetime::format_datetime_opt(dt.as_ref(), &tz())
}

/// DRF `DateTimeField.to_internal_value` (ISO 8601).
pub fn parse_dt(s: &str) -> Result<DateTime<Utc>, String> {
    pl_core::datetime::drf_parse_datetime(s, &tz())
}

// ---------------------------------------------------------------------------
// Pagination (StandardPagination via pl-core)
// ---------------------------------------------------------------------------

pub fn page_request(ctx: &Ctx) -> PageRequest {
    PageRequest::from_query(&ctx.rc.query)
}

pub fn resolve_page(req: &PageRequest, count: i64) -> ApiResult<PageInfo> {
    Ok(req.resolve(count)?)
}

pub fn paginated(
    ctx: &Ctx,
    info: &PageInfo,
    all_ids: Option<Vec<i64>>,
    results: Vec<Value>,
) -> Response {
    let all = all_ids.map(|ids| ids.into_iter().map(Value::from).collect());
    json_response(
        StatusCode::OK,
        &paginated_body(&ctx.rc, info, results, all),
    )
}

// ---------------------------------------------------------------------------
// django.utils.crypto.get_random_string
// ---------------------------------------------------------------------------

pub const RANDOM_STRING_CHARS: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn get_random_string(len: usize) -> String {
    use rand::Rng;
    let mut rng = rand::rngs::OsRng;
    (0..len)
        .map(|_| RANDOM_STRING_CHARS[rng.gen_range(0..RANDOM_STRING_CHARS.len())] as char)
        .collect()
}

/// `pk` from the path: non-numeric → DRF `get_object_or_404` catches the ValueError →
/// `Http404` without text → `Not found.`
pub fn parse_pk(raw: &str) -> ApiResult<i64> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::not_found_plain())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_string_charset() {
        let s = get_random_string(50);
        assert_eq!(s.len(), 50);
        assert!(s.bytes().all(|b| RANDOM_STRING_CHARS.contains(&b)));
    }

    #[test]
    fn pk_parsing() {
        assert_eq!(parse_pk(" 5").ok(), Some(5));
        assert!(parse_pk("abc").is_err());
    }

    #[test]
    fn body_parsing() {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert_eq!(parse_body(&h, b"{\"a\":1}").ok(), Some(json!({"a": 1})));
        assert_eq!(parse_body(&h, b"").ok(), Some(json!({})));
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let v = parse_body(&h, b"document_ids=1&document_ids=2").unwrap_or_default();
        assert_eq!(v["__list__document_ids"], json!(["1", "2"]));
        assert!(require_object(json!([1])).is_err());
    }

    /// W2 parity run 2026-11-08: Django's 500 page carries neither `Allow` nor `Vary: Accept`.
    #[test]
    fn server_error_without_drf_headers() {
        let r = finish("GET, POST, HEAD, OPTIONS", Err(ApiError::ServerError));
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(r.headers().get(header::ALLOW).is_none());
        assert!(r.headers().get(header::VARY).is_none());
        let r = finish("GET, POST, HEAD, OPTIONS", Err(ApiError::forbidden()));
        assert_eq!(r.headers().get(header::ALLOW).map(|v| v.as_bytes()), Some(&b"GET, POST, HEAD, OPTIONS"[..]));
    }

    /// W2 parity run 2026-11-08 (`w2.cf.create.validation`): DRF's text uses Python's json message.
    #[tokio::test]
    async fn json_parse_error_like_python() {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let Err(e) = parse_body(&h, b"{bad") else { panic!("must fail") };
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        assert_eq!(
            v,
            json!({"detail": "JSON parse error - Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"})
        );
    }
}
