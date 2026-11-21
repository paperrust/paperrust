// upstream: rest_framework/views.py (exception_handler: Http404 -> NotFound),
//           django/views/defaults.py (ERROR_PAGE_TEMPLATE for 404/500),
//           django/http/response.py (HttpResponseForbidden, HttpResponseRedirect)
//
// Small response helpers for the shapes the file routes need. DRF auth/permission errors come
// from pl-authz, Allow/Vary from `pl_core::route::drf_view`.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub const HTML: &str = "text/html; charset=utf-8";

/// DRF `Response({"detail": ...}, status)` rendered by `JSONRenderer` (compact).
pub fn drf_detail(status: StatusCode, detail: &str) -> Response {
    drf_json(status, &serde_json::json!({ "detail": detail }))
}

pub fn drf_json(status: StatusCode, value: &serde_json::Value) -> Response {
    pl_core::ApiError::json(status, value.clone()).into_response()
}

pub fn drf_not_found() -> Response {
    drf_detail(StatusCode::NOT_FOUND, "Not found.")
}

/// Plain Django `HttpResponse(text, status=...)` with the default content type.
pub fn plain(status: StatusCode, text: &'static str) -> Response {
    let mut r = Response::new(Body::from(text));
    *r.status_mut() = status;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(HTML));
    r
}

/// `HttpResponseForbidden("Insufficient permissions")`.
pub fn insufficient_permissions() -> Response {
    plain(StatusCode::FORBIDDEN, "Insufficient permissions")
}

/// `HttpResponseRedirect(location)` – 302, empty body.
pub fn redirect(location: &str) -> Response {
    let mut r = plain(StatusCode::FOUND, "");
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn error_page(title: &str, details: &str) -> String {
    format!(
        "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>{title}</title>\n</head>\n<body>\n  <h1>{title}</h1><p>{details}</p>\n</body>\n</html>\n"
    )
}

/// Django `page_not_found` without a 404.html template (DEBUG=False).
pub fn django_404() -> Response {
    let mut r = Response::new(Body::from(error_page("Not Found", "The requested resource was not found on this server.")));
    *r.status_mut() = StatusCode::NOT_FOUND;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(HTML));
    r
}

/// Log an internal error (never into the response) and answer like Django (generic 500).
pub fn internal<E: std::fmt::Display>(context: &str, e: E) -> Response {
    pl_core::ApiError::internal(format!("pl_files {context}: {e}")).into_response()
}
