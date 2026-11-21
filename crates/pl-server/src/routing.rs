//! `routes.toml` and route mounting.
//!
//! - Every route is served by Rust (since 2026-11-14 there is no Django backend any more).
//! - `routes.toml` (optional): `[routes]` table `id = "off"` switches a route off (it then
//!   answers like a path without a route: 404). `"rust"`/`"on"` = on (default for ids not
//!   listed). The obsolete value `"django"` is accepted with a warning and means on.
//!   Unknown ids, unknown keys or values are a startup error.
//! - Route ids must be unique, `(method, path)` must be unique (paths compared with
//!   parameter names normalised, `{id}` == `{pk}`), and all routes of one path pattern
//!   must spell it identically (axum/matchit requirement).
//! - Active routes with the same path from different crates are merged into one
//!   `MethodRouter`; other methods on such a path get DRF's 401/403/405 below `/api/` and the
//!   404 page elsewhere (docs/STANDALONE-GAPS.md §5).
//! - Every mounted handler sits behind [`crate::body::gate`] (body buffering and limits).
//!
//! upstream: src/paperless/urls.py@v3.4.2 (URL layout)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use axum::Router;
use axum::extract::Request;
use axum::http::Method;
use axum::routing::MethodRouter;
use pl_core::{ApiRoute, AppState};
use serde::Deserialize;

use crate::body::{BODY_BUFFER_LIMIT, Gate};

/// Value of a route id in `routes.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Served by Rust (also `"on"`; default for ids not listed).
    #[serde(alias = "on")]
    Rust,
    /// Switched off: the route is not mounted.
    Off,
    /// Obsolete (forwarding to Django was removed): read as [`Backend::Rust`] with a warning.
    Django,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutesFile {
    #[serde(default)]
    routes: BTreeMap<String, Backend>,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutesError {
    #[error("routes.toml: {0}")]
    Parse(String),
    #[error("routes.toml: cannot read {path}: {error}")]
    Read { path: String, error: String },
    #[error("routes.toml: unknown route id(s): {}", .0.join(", "))]
    UnknownIds(Vec<String>),
    #[error("route id {0:?} is registered more than once")]
    DuplicateId(String),
    #[error("route {method} {path} is registered twice (ids {first:?} and {second:?})")]
    DuplicateRoute { method: String, path: String, first: String, second: String },
    #[error("routes {first:?} ({first_path}) and {second:?} ({second_path}) use the same path with different parameter names")]
    ParamNameConflict { first: String, first_path: String, second: String, second_path: String },
    #[error("route {id:?}: invalid path {path:?} ({reason})")]
    InvalidPath { id: String, path: String, reason: &'static str },
    #[error("cannot mount path {path:?}: {reason}")]
    Mount { path: String, reason: String },
}

/// Parse the contents of `routes.toml`.
pub fn parse_routes_toml(text: &str) -> Result<BTreeMap<String, Backend>, RoutesError> {
    toml::from_str::<RoutesFile>(text).map(|f| f.routes).map_err(|e| RoutesError::Parse(e.to_string()))
}

/// Read `routes.toml`; a missing file is only fine when it was not requested explicitly.
pub fn load_routes_file(path: &std::path::Path, required: bool) -> Result<BTreeMap<String, Backend>, RoutesError> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse_routes_toml(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => {
            tracing::debug!(path = %path.display(), "routes.toml not found: every route is on");
            Ok(BTreeMap::new())
        }
        Err(e) => Err(RoutesError::Read { path: path.display().to_string(), error: e.to_string() }),
    }
}

/// Public description of a route (for logs and the health endpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInfo {
    pub id: &'static str,
    pub method: Method,
    pub path: &'static str,
    pub upstream: &'static str,
}

impl From<&ApiRoute> for RouteInfo {
    fn from(r: &ApiRoute) -> Self {
        Self { id: r.id, method: r.method.clone(), path: r.path, upstream: r.upstream }
    }
}

/// Validated set of routes to mount.
pub struct MountPlan {
    pub active: Vec<ApiRoute>,
    pub total: usize,
}

impl MountPlan {
    pub fn active_info(&self) -> Vec<RouteInfo> {
        self.active.iter().map(RouteInfo::from).collect()
    }
}

/// `/a/{id}/b/` -> `/a/{}/b/` (also `{*rest}` -> `{*}`).
fn normalize(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.starts_with("{*") && seg.ends_with('}') {
                "{*}".to_owned()
            } else if seg.starts_with('{') && seg.ends_with('}') {
                "{}".to_owned()
            } else {
                seg.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn validate_path(r: &ApiRoute) -> Result<(), RoutesError> {
    let err = |reason| RoutesError::InvalidPath { id: r.id.to_owned(), path: r.path.to_owned(), reason };
    if !r.path.starts_with('/') {
        return Err(err("must start with '/'"));
    }
    if r.path.contains(['?', '#', '%']) {
        return Err(err("must not contain '?', '#' or '%'"));
    }
    if r.path.starts_with(crate::frontline::INTERNAL_PREFIX) {
        return Err(err("reserved internal prefix"));
    }
    Ok(())
}

/// Validate all routes against each other and against `routes.toml`, keep the active ones.
pub fn plan(all: Vec<ApiRoute>, config: &BTreeMap<String, Backend>) -> Result<MountPlan, RoutesError> {
    let mut ids = HashSet::new();
    let mut by_method_path: HashMap<(Method, String), &'static str> = HashMap::new();
    let mut spelling: HashMap<String, (&'static str, &'static str)> = HashMap::new();
    for r in &all {
        if r.id.is_empty() {
            return Err(RoutesError::InvalidPath { id: String::new(), path: r.path.to_owned(), reason: "empty route id" });
        }
        if !ids.insert(r.id) {
            return Err(RoutesError::DuplicateId(r.id.to_owned()));
        }
        validate_path(r)?;
        let norm = normalize(r.path);
        if let Some(first) = by_method_path.insert((r.method.clone(), norm.clone()), r.id) {
            return Err(RoutesError::DuplicateRoute {
                method: r.method.to_string(),
                path: r.path.to_owned(),
                first: first.to_owned(),
                second: r.id.to_owned(),
            });
        }
        match spelling.get(&norm) {
            Some((id, p)) if *p != r.path => {
                return Err(RoutesError::ParamNameConflict {
                    first: (*id).to_owned(),
                    first_path: (*p).to_owned(),
                    second: r.id.to_owned(),
                    second_path: r.path.to_owned(),
                });
            }
            Some(_) => {}
            None => {
                spelling.insert(norm, (r.id, r.path));
            }
        }
    }
    let unknown: Vec<String> = config.keys().filter(|k| !ids.contains(k.as_str())).cloned().collect();
    if !unknown.is_empty() {
        return Err(RoutesError::UnknownIds(unknown));
    }
    let obsolete: Vec<&str> = config.iter().filter(|(_, v)| **v == Backend::Django).map(|(k, _)| k.as_str()).collect();
    if !obsolete.is_empty() {
        tracing::warn!(
            count = obsolete.len(),
            ids = %obsolete.join(", "),
            "routes.toml: \"django\" is obsolete (there is no Django backend any more); these routes are served by Rust. Use \"off\" to switch a route off."
        );
    }
    let total = all.len();
    let active = all.into_iter().filter(|r| config.get(r.id) != Some(&Backend::Off)).collect();
    Ok(MountPlan { active, total })
}

/// Fallback for a path without a route (or a method no route serves): the 404 page / DRF JSON.
/// axum's `MethodRouter` adds `Allow` to fallback responses that lack one; the 404 must stay
/// without it, so the response is marked and [`strip_injected_allow`] removes axum's header.
async fn not_found_fallback(req: Request) -> axum::response::Response {
    let mut res = crate::frontend::not_found(req.uri().path());
    if !res.headers().contains_key(axum::http::header::ALLOW) {
        res.extensions_mut().insert(NoUpstreamAllow);
    }
    res
}

/// Paths below `/api/` whose views do not require authentication (or are not DRF views): a wrong
/// method there is not answered by [`method_not_allowed`].
const NO_AUTH_API_PATHS: &[&str] = &["/api/", "/api/token/", "/api/auth/login/", "/api/auth/logout/", "/api/schema/", "/api/schema/view/"];

/// docs/STANDALONE-GAPS.md §5: a method that no Rust route serves on an `/api/` path gets DRF's
/// answer instead of a 404 – authentication first (`APIView.initial`:
/// 401/403 like the view), then `{"detail": "Method \"X\" not allowed."}` (405) with the view's
/// `Allow`. Approximation: view-specific permission checks before the 405 (e.g. model
/// permissions of a `ModelViewSet`) are not evaluated. `OPTIONS` gets DRF's metadata
/// (`SimpleMetadata`, [`crate::drf_options`]) after the same authentication.
async fn method_not_allowed(state: AppState, allow: &str, meta: &serde_json::Value, req: Request) -> axum::response::Response {
    use axum::extract::FromRequestParts as _;
    use axum::response::IntoResponse as _;
    let (mut parts, _body) = req.into_parts();
    let mut resp = match pl_authz::AuthUser::from_request_parts(&mut parts, &state).await {
        Err(e) => e.into_response(),
        Ok(_) if parts.method == Method::OPTIONS => match pl_core::RequestContext::from_request_parts(&mut parts, &state).await {
            Ok(ctx) => pl_core::Drf::ok(meta).indent(ctx.negotiation.indent).into_response(),
            Err(e) => e.into_response(),
        },
        Ok(_) => pl_core::ApiError::method_not_allowed(parts.method.as_str()).into_response(),
    };
    pl_core::route::apply_drf_view_headers(resp.headers_mut(), allow);
    resp
}

/// Marker: the fallback 404 had no `Allow` header.
#[derive(Clone, Copy)]
struct NoUpstreamAllow;

/// Parity fix (run 2026-11-08, `files.bulk_download.archive`): a wrong method on a path served by
/// Rust must not get axum's `Allow: GET,HEAD` on the 404 page.
pub(crate) fn strip_injected_allow(mut res: axum::response::Response) -> axum::response::Response {
    if res.extensions().get::<NoUpstreamAllow>().is_some() {
        res.headers_mut().remove(axum::http::header::ALLOW);
    }
    res
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "unknown error".to_owned())
}

/// Build the router with the active routes; other paths and other methods on mounted paths get
/// the 404 (or DRF's 401/403/405 below `/api/`).
pub fn build_router(plan: MountPlan) -> Result<Router<AppState>, RoutesError> {
    let ui = crate::frontend::UiFlags::from_ids(plan.active.iter().map(|r| r.id));
    let mut grouped: BTreeMap<&'static str, Vec<ApiRoute>> = BTreeMap::new();
    for r in plan.active {
        grouped.entry(r.path).or_default().push(r);
    }
    let mut router: Router<AppState> = Router::new();
    for (path, routes) in grouped {
        let methods: Vec<Method> = routes.iter().map(|r| r.method.clone()).collect();
        let drf_405 = path.starts_with("/api/") && !NO_AUTH_API_PATHS.contains(&path);
        let allow = pl_core::route::allow_value(&methods);
        let meta = std::sync::Arc::new(crate::drf_options::view_metadata(path).to_json());
        let merged = catch_unwind(AssertUnwindSafe(|| {
            let mut mr: MethodRouter<AppState> = MethodRouter::new();
            for r in routes {
                let gate = Gate {
                    flags: r.flags(),
                    limit: BODY_BUFFER_LIMIT,
                    streaming: crate::upload::STREAMING_ROUTES.contains(&r.id),
                    allow: drf_405.then(|| std::sync::Arc::<str>::from(allow.as_str())),
                };
                mr = mr.merge(r.handler.layer(axum::middleware::from_fn_with_state(gate, crate::body::gate)));
            }
            if drf_405 {
                let allow = allow.clone();
                let meta = meta.clone();
                mr.fallback(move |axum::extract::State(st): axum::extract::State<AppState>, req: Request| {
                    let allow = allow.clone();
                    let meta = meta.clone();
                    async move { method_not_allowed(st, &allow, &meta, req).await }
                })
            } else {
                mr.fallback(not_found_fallback)
            }
        }))
        .map_err(|p| RoutesError::Mount { path: path.to_owned(), reason: format!("handlers overlap: {}", panic_text(p)) })?;
        let r = std::mem::take(&mut router);
        router = catch_unwind(AssertUnwindSafe(move || r.route(path, merged)))
            .map_err(|p| RoutesError::Mount { path: path.to_owned(), reason: panic_text(p) })?;
    }
    if ui.index {
        // `ui.index`: Django's catch-all `re_path(r".*", IndexView)` for paths without a route.
        Ok(router.fallback(|axum::extract::State(st): axum::extract::State<AppState>, req: Request| async move {
            crate::frontend::fallback(st, req).await
        }))
    } else {
        Ok(router.fallback(not_found_fallback))
    }
}
