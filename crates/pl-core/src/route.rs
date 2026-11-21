//! Route type and DRF view headers.
//!
//! upstream: rest_framework/views.py@3.18.3 (APIView.default_response_headers, finalize_response)
//!
//! DRF sets on **every** response of an APIView (also on error responses 401/403/404/406):
//! * `Allow: <methods of the view>` – order by `http_method_names`
//!   (`GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS`), only those present. ViewSets always have
//!   `OPTIONS`, and `HEAD` as soon as `GET` is mapped.
//! * `Vary: Accept`, because paperless has two renderers (JSON + Browsable API).
//!
//! [`drf_view`] adds both via a layer. The Django middlewares afterwards add further
//! `Vary` values and headers (see `docs/contract/pl-core.md`, section "Header"); that is
//! the job of `pl-server`.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use axum::http::header::{ALLOW, VARY};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::response::Response;
use axum::routing::MethodRouter;

use crate::AppState;

/// An API route that a module implements in Rust.
///
/// `id` is stable and is used in `routes.toml` for switching on/off, e.g. `"tags.list"`.
/// `path` in axum syntax incl. trailing slash like Django, e.g. `"/api/tags/{id}/"`.
pub struct ApiRoute {
    /// Stable ID, e.g. `"tags.list"`.
    pub id: &'static str,
    /// HTTP method of this route.
    pub method: Method,
    /// Path in axum syntax (`/api/tags/{id}/`).
    pub path: &'static str,
    /// Upstream source, e.g. `"src/documents/views.py::TagViewSet.list"`.
    pub upstream: &'static str,
    /// Handler (typically `drf_view(get(handler), ALLOW_MODEL_LIST)`).
    pub handler: MethodRouter<AppState>,
}

impl ApiRoute {
    /// Convenient constructor (equivalent to the struct literal).
    pub fn new(id: &'static str, method: Method, path: &'static str, upstream: &'static str, handler: MethodRouter<AppState>) -> Self {
        Self { id, method, path, upstream, handler }
    }

    /// Route may delegate to Django ([`crate::markers::delegate_to_django`]). `pl-server`
    /// then buffers the request body (up to the limit) and sends requests with a larger body
    /// directly to Django.
    pub fn delegatable(self) -> Self {
        update_flags(self.id, |f| f.delegatable = true);
        self
    }

    /// Handler understands `application/x-www-form-urlencoded` (DRF `FormParser`). Without this
    /// marker, POST/PUT/PATCH with this content type go directly to Django.
    pub fn accepts_form(self) -> Self {
        update_flags(self.id, |f| f.accepts_form = true);
        self
    }

    /// Handler understands `multipart/form-data` (DRF `MultiPartParser`). Without this marker,
    /// POST/PUT/PATCH with this content type go directly to Django.
    pub fn accepts_multipart(self) -> Self {
        update_flags(self.id, |f| f.accepts_multipart = true);
        self
    }

    /// All DRF default parsers (JSON, form, multipart).
    pub fn accepts_drf_parsers(self) -> Self {
        self.accepts_form().accepts_multipart()
    }

    /// Markers of this route.
    pub fn flags(&self) -> RouteFlags {
        route_flags(self.id)
    }
}

/// Properties of a route that `pl-server` must know **before** the handler.
///
/// Stored in a process-wide table by route id (ids are unique, see the
/// test in pl-server), so that [`ApiRoute`] stays stable as a struct literal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteFlags {
    /// Can answer with [`crate::markers::DelegateToDjango`].
    pub delegatable: bool,
    /// Accepts form bodies.
    pub accepts_form: bool,
    /// Accepts multipart bodies.
    pub accepts_multipart: bool,
}

static ROUTE_FLAGS: LazyLock<RwLock<HashMap<&'static str, RouteFlags>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

fn update_flags(id: &'static str, f: impl FnOnce(&mut RouteFlags)) {
    let mut m = match ROUTE_FLAGS.write() {
        Ok(m) => m,
        Err(p) => p.into_inner(),
    };
    f(m.entry(id).or_default());
}

/// Markers of the route `id` (default: none).
pub fn route_flags(id: &str) -> RouteFlags {
    let m = match ROUTE_FLAGS.read() {
        Ok(m) => m,
        Err(p) => p.into_inner(),
    };
    m.get(id).copied().unwrap_or_default()
}

/// `Allow` of a `ModelViewSet` list route (`list` + `create`).
pub const ALLOW_MODEL_LIST: &str = "GET, POST, HEAD, OPTIONS";
/// `Allow` of a `ModelViewSet` detail route (`retrieve`, `update`, `partial_update`, `destroy`).
pub const ALLOW_MODEL_DETAIL: &str = "GET, PUT, PATCH, DELETE, HEAD, OPTIONS";
/// `Allow` of a `ReadOnlyModelViewSet` list or detail route or of a GET-only view.
pub const ALLOW_READ_ONLY: &str = "GET, HEAD, OPTIONS";
/// `Allow` of a POST-only view/action.
pub const ALLOW_POST_ONLY: &str = "POST, OPTIONS";

/// Builds the `Allow` value like DRF from a set of methods: order by Django's
/// `http_method_names`, `HEAD` automatically with `GET`, `OPTIONS` always.
pub fn allow_value(methods: &[Method]) -> String {
    let order = [Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::HEAD, Method::OPTIONS, Method::TRACE];
    let has = |m: &Method| methods.contains(m) || (*m == Method::HEAD && methods.contains(&Method::GET)) || *m == Method::OPTIONS;
    order.iter().filter(|m| has(m)).map(|m| m.as_str()).collect::<Vec<_>>().join(", ")
}

/// Adds `Allow: <allow>` and `Vary: Accept` to every response of the handler (merged like
/// Django's `patch_vary_headers`). The same applies to DRF error responses – exactly like DRF; not for
/// Django's own 400/500 pages ([`crate::markers::DjangoHandlerResponse`]).
pub fn drf_view(handler: MethodRouter<AppState>, allow: &'static str) -> MethodRouter<AppState> {
    handler.layer(axum::middleware::map_response(move |mut resp: Response| async move {
        finalize_drf_response(&mut resp, allow);
        resp
    }))
}

/// DRF `finalize_response` (Allow/Vary) – except for Django's own 400/500 pages, which arise outside
/// of DRF ([`crate::markers::DjangoHandlerResponse`]).
pub fn finalize_drf_response(resp: &mut Response, allow: &str) {
    if resp.extensions().get::<crate::markers::DjangoHandlerResponse>().is_none() {
        apply_drf_view_headers(resp.headers_mut(), allow);
    }
}

/// Sets `Allow` and adds `Vary: Accept` on an existing header map.
pub fn apply_drf_view_headers(headers: &mut HeaderMap, allow: &str) {
    if let Ok(v) = HeaderValue::from_str(allow) {
        headers.insert(ALLOW, v);
    }
    patch_vary(headers, &["Accept"]);
}

/// Like `django.utils.cache.patch_vary_headers`: keep existing values (order),
/// append new ones, comparison case-insensitive, `*` wins.
pub fn patch_vary(headers: &mut HeaderMap, new: &[&str]) {
    let mut values: Vec<String> = headers
        .get_all(VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for n in new {
        if !values.iter().any(|v| v.eq_ignore_ascii_case(n)) {
            values.push((*n).to_string());
        }
    }
    let joined = if values.iter().any(|v| v == "*") { "*".to_string() } else { values.join(", ") };
    if let Ok(v) = HeaderValue::from_str(&joined) {
        headers.insert(VARY, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity run 2026-11-08 (`correspondents.list.ordering±last_correspondence`): Django's 500 page
    /// has `text/html; charset=utf-8`, no `Allow`, no `Vary: Accept`.
    #[test]
    fn django_500_page_has_no_drf_headers() {
        use axum::response::IntoResponse;
        let mut r = crate::ApiError::internal("boom").into_response();
        finalize_drf_response(&mut r, ALLOW_MODEL_LIST);
        assert!(r.headers().get(ALLOW).is_none());
        assert!(r.headers().get(VARY).is_none());
        assert_eq!(r.headers()[axum::http::header::CONTENT_TYPE], "text/html; charset=utf-8");
        let mut r = crate::ApiError::django_bad_request().into_response();
        finalize_drf_response(&mut r, ALLOW_MODEL_LIST);
        assert!(r.headers().get(ALLOW).is_none());
        let mut r = crate::ApiError::not_found().into_response();
        finalize_drf_response(&mut r, ALLOW_MODEL_LIST);
        assert_eq!(r.headers()[ALLOW], ALLOW_MODEL_LIST);
        assert_eq!(r.headers()[VARY], "Accept");
    }

    #[test]
    fn allow_like_drf() {
        assert_eq!(allow_value(&[Method::GET, Method::POST]), ALLOW_MODEL_LIST);
        assert_eq!(allow_value(&[Method::DELETE, Method::PATCH, Method::PUT, Method::GET]), ALLOW_MODEL_DETAIL);
        assert_eq!(allow_value(&[Method::GET]), ALLOW_READ_ONLY);
        assert_eq!(allow_value(&[Method::POST]), ALLOW_POST_ONLY);
    }

    #[test]
    fn flags_registry() {
        let r = ApiRoute::new("test.flags", Method::POST, "/x/", "", MethodRouter::new()).delegatable().accepts_form();
        assert_eq!(r.flags(), RouteFlags { delegatable: true, accepts_form: true, accepts_multipart: false });
        assert_eq!(route_flags("test.unknown"), RouteFlags::default());
    }

    #[test]
    fn vary_patch() {
        let mut h = HeaderMap::new();
        patch_vary(&mut h, &["Accept"]);
        patch_vary(&mut h, &["Accept-Language", "accept"]);
        patch_vary(&mut h, &["Cookie"]);
        assert_eq!(h.get(VARY).unwrap(), "Accept, Accept-Language, Cookie");
    }
}
