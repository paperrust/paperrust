//! The Angular frontend without Django: `IndexView` (`/` and the catch-all), WhiteNoise for
//! `/static/`, the `RedirectView`s of the URL configuration (`/api/`, `/assets/…`, `/fetch/…`).
//!
//! Route ids (activate in `routes.toml`):
//! * `ui.index` – `GET /`, and the catch-all: every GET/HEAD that matches no Rust route and no
//!   specific upstream view ([`urlconf::is_upstream_view`]) renders the index like Django's
//!   `re_path(r".*", login_required(ensure_csrf_cookie(IndexView.as_view())))`.
//! * `static.files` – `/static/{*path}` (WhiteNoise, see [`statics`]); unknown files fall through
//!   to the index like in Django (WhiteNoise passes them on to the URL configuration).
//! * `ui.api_root`, `ui.assets`, `ui.fetch_doc`, `ui.fetch_thumb`, `ui.fetch_preview` – redirects.
//!
//! upstream: src/paperless/urls.py@v3.4.2, src/documents/views.py@v3.4.2 (`IndexView`),
//! src/documents/templates/index.html@v3.4.2, django@5.2 contrib/auth/decorators.py
//! (`login_required` → `redirect_to_login`), contrib/auth/views.py (`redirect_to_login`),
//! views/decorators/csrf.py (`ensure_csrf_cookie` → `get_token`), views/generic/base.py
//! (`RedirectView`, `permanent=False`, `query_string=False`).

pub mod index_i18n;
pub mod media_types;
pub mod statics;
pub mod urlconf;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, get, on};
use pl_core::{ApiRoute, AppState};

use pl_accounts::web::{Ctx, html, redirect, server_error};

/// Every method: WhiteNoise answers 405 itself (not `any()`: axum implements it as the
/// `MethodRouter` fallback, which `routing::build_router` replaces by the Django proxy).
const ALL_METHODS: MethodFilter = MethodFilter::GET
    .or(MethodFilter::HEAD)
    .or(MethodFilter::POST)
    .or(MethodFilter::PUT)
    .or(MethodFilter::PATCH)
    .or(MethodFilter::DELETE)
    .or(MethodFilter::OPTIONS)
    .or(MethodFilter::TRACE);

/// `static.swagger_ui_dist` (pl-cli) serves a subset of `/static/` with fixed headers; with
/// `static.files` active WhiteNoise's behaviour for those files wins (collectstatic contains them).
pub const SUPERSEDED_BY_STATIC: &[&str] = &["static.swagger_ui_dist"];

/// Ids of this module (for [`UiFlags`]).
pub const INDEX_ID: &str = "ui.index";
pub const STATIC_ID: &str = "static.files";

/// Which frontend routes are active (set by `build_app`, read by the handlers and the fallback).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiFlags {
    pub index: bool,
    pub static_files: bool,
}

impl UiFlags {
    pub fn from_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut f = Self::default();
        for id in ids {
            match id {
                INDEX_ID => f.index = true,
                STATIC_ID => f.static_files = true,
                _ => {}
            }
        }
        f
    }
}

pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new(INDEX_ID, Method::GET, "/", "src/paperless/urls.py::IndexView (re_path .*)", get(index_root)),
        ApiRoute::new(
            STATIC_ID,
            Method::GET,
            "/static/{*path}",
            "whitenoise.middleware.WhiteNoiseMiddleware (STATIC_ROOT)",
            on(ALL_METHODS, static_files),
        ),
        ApiRoute::new("ui.api_root", Method::GET, "/api/", "src/paperless/urls.py::RedirectView(url=\"schema/view/\")", get(api_root)),
        ApiRoute::new(
            "ui.assets",
            Method::GET,
            "/assets/{*path}",
            "src/paperless/urls.py::RedirectView(STATIC_URL + frontend/en-US/assets/)",
            get(assets),
        ),
        ApiRoute::new(
            "ui.fetch_doc",
            Method::GET,
            "/fetch/doc/{num}",
            "src/paperless/urls.py::RedirectView(api/documents/<pk>/download/)",
            get(fetch_doc),
        ),
        ApiRoute::new(
            "ui.fetch_thumb",
            Method::GET,
            "/fetch/thumb/{num}",
            "src/paperless/urls.py::RedirectView(api/documents/<pk>/thumb/)",
            get(fetch_thumb),
        ),
        ApiRoute::new(
            "ui.fetch_preview",
            Method::GET,
            "/fetch/preview/{num}",
            "src/paperless/urls.py::RedirectView(api/documents/<pk>/preview/)",
            get(fetch_preview),
        ),
    ]
}

fn flags(st: &AppState) -> UiFlags {
    st.service::<UiFlags>().unwrap_or_default()
}

/// `request.path_info` (decoded like Django).
fn decoded_path(req: &Request) -> String {
    pl_core::urlenc::decode_request_path(req.uri().path())
}

/// Should a GET/HEAD that no Rust route serves render the index (instead of the 404)?
///
/// Never below `/api/` (parity run 2026-11-20, F5): an API client gets DRF's JSON 404
/// `{"detail": "Not found."}` instead of a redirect into the web UI. Upstream renders the
/// Angular index there (catch-all `re_path(r".*")`); Known difference, docs/contract/pl-server.md.
pub fn index_handles(flags: UiFlags, method: &Method, decoded: &str) -> bool {
    flags.index
        && (*method == Method::GET || *method == Method::HEAD)
        && !is_api_path(decoded)
        && !urlconf::is_upstream_view(decoded)
        && !(decoded.starts_with(statics::PREFIX) && !flags.static_files)
}

/// Router fallback (no Rust route matched): index or the 404.
pub async fn fallback(st: AppState, req: Request) -> Response {
    let path = decoded_path(&req);
    // Websocket upgrades (Channels, not the HTTP URL configuration) are not the index.
    let upgrade = req.headers().contains_key(axum::http::header::UPGRADE);
    if !upgrade && index_handles(flags(&st), req.method(), &path) { index_view(st, req).await } else { not_found(req.uri().path()) }
}

/// Inside a mounted route: index if active and Django would render it, else the 404 of a path
/// without a route (Django's URL resolver finds no other view).
async fn index_or_delegate(st: AppState, req: Request) -> Response {
    let path = decoded_path(&req);
    if index_handles(flags(&st), req.method(), &path) { index_view(st, req).await } else { not_found(req.uri().path()) }
}

async fn index_root(State(st): State<AppState>, req: Request) -> Response {
    index_view(st, req).await
}

async fn static_files(State(st): State<AppState>, req: Request) -> Response {
    let files = match st.service::<Arc<statics::StaticFiles>>() {
        Some(f) => f,
        None => match st.service::<statics::StaticScan>() {
            Some(scan) => scan.files().await,
            // `static.files` without a scanned STATIC_ROOT (build_app always sets one): 404.
            None => return not_found(req.uri().path()),
        },
    };
    let path = decoded_path(&req);
    match files.get(&path) {
        Some(f) => f.respond(req.method(), req.headers()).await,
        None => index_or_delegate(st, req).await,
    }
}

/// `HttpResponseRedirect(url)` of a `RedirectView`, through the session/auth middleware like in
/// Django (`ApiVersionMiddleware` adds `X-Api-Version` for a logged-in session user).
async fn redirect_view(st: AppState, req: Request, url: &str) -> Response {
    let mut ctx = match Ctx::new(st, req).await {
        Ok(c) => c,
        Err(e) => return server_error(&e),
    };
    if let Err(e) = pl_accounts::auth::resolve_user(&mut ctx).await {
        return server_error(&e);
    }
    ctx.finish(redirect(url)).await
}

async fn api_root(State(st): State<AppState>, req: Request) -> Response {
    redirect_view(st, req, "schema/view/").await
}

async fn assets(State(st): State<AppState>, req: Request) -> Response {
    let path = decoded_path(&req);
    match path.strip_prefix("/assets/").filter(|p| !p.is_empty()) {
        Some(rest) => {
            let url = format!("{}frontend/en-US/assets/{rest}", st.config().static_url);
            redirect_view(st, req, &url).await
        }
        None => index_or_delegate(st, req).await,
    }
}

async fn fetch(st: AppState, req: Request, kind: &str, target: &str) -> Response {
    let path = decoded_path(&req);
    let prefix = format!("/fetch/{kind}/");
    match path.strip_prefix(&prefix).filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        Some(pk) => {
            let url = format!("{}api/documents/{pk}/{target}/", st.config().base_url);
            redirect_view(st, req, &url).await
        }
        None => index_or_delegate(st, req).await,
    }
}

async fn fetch_doc(State(st): State<AppState>, req: Request) -> Response {
    fetch(st, req, "doc", "download").await
}

async fn fetch_thumb(State(st): State<AppState>, req: Request) -> Response {
    fetch(st, req, "thumb", "thumb").await
}

async fn fetch_preview(State(st): State<AppState>, req: Request) -> Response {
    fetch(st, req, "preview", "preview").await
}

/// `IndexView.get_frontend_language` (Django `en-us` → Angular `en-US`).
pub fn frontend_language(lang: &str) -> String {
    match lang.find('-') {
        Some(i) => format!("{}-{}", &lang[..i], lang[i + 1..].to_uppercase()),
        None => lang.to_owned(),
    }
}

/// `{% static name %}` with `CompressedStaticFilesStorage`: `urljoin(STATIC_URL,
/// filepath_to_uri(name).lstrip("/"))`, HTML-escaped (autoescape).
fn static_url(base: &str, name: &str) -> String {
    let uri = pl_core::urlenc::quote(&name.replace('\\', "/"), "/~!*()'");
    pl_accounts::tpl::escape(&format!("{base}{}", uri.trim_start_matches('/')))
}

/// `gettext` for the three texts of the loading screen.
fn tr(lang: &str, i: usize) -> &'static str {
    index_i18n::CATALOG
        .iter()
        .find(|(l, _)| *l == lang)
        .and_then(|(_, rows)| rows.iter().find(|(n, _)| *n as usize == i))
        .map(|(_, t)| *t)
        .unwrap_or(index_i18n::MSGIDS[i])
}

const TEMPLATE: &str = include_str!("index.html");
/// Same template with PaperRust's name, author, logo and docs link (from `frontend/server/index.html`,
/// R2), used when branding is on (`pl_core::Config::product_name`). Placeholders are identical.
const TEMPLATE_PAPERRUST: &str = include_str!("index_paperrust.html");

/// Values of the template (all already escaped as Django would).
pub struct IndexContext {
    pub base: String,
    pub cookie_prefix: String,
    pub static_base: String,
    pub frontend_lang: String,
    pub ui_lang: String,
    /// `(level_tag, text)`.
    pub messages: Vec<(String, String)>,
    /// `Some(name)` with branding: PaperRust template, `Paperless-ngx` -> `name` in the loading-screen
    /// texts of every language. `None`: upstream template byte for byte (parity).
    pub product_name: Option<String>,
    /// Desktop app only (`PAPERRUST_SHARE_MODE`, see [`share_meta`]): `<meta>` tags for the share
    /// dialogs (frontend patch 09), added to the PaperRust template only.
    pub share_meta: String,
}

/// `<meta>` tags telling the UI how far share links reach (frontend patch 09,
/// `paperrustShareMode()`): `PAPERRUST_SHARE_MODE=local` (desktop app, this computer only) or
/// `lan` (desktop app with network sharing; `PAPERRUST_SHARE_URL`, default `PAPERLESS_URL`, is
/// the address other devices use). Unset (server operation): nothing, the UI behaves as upstream.
pub fn share_meta(env: &pl_core::config::Env, paperless_url: Option<&str>) -> String {
    let esc = pl_accounts::tpl::escape;
    match env.get("PAPERRUST_SHARE_MODE").map(str::trim) {
        Some("local") => "<meta name=\"paperrust_share_mode\" content=\"local\">".into(),
        Some("lan") => {
            let url = env.get("PAPERRUST_SHARE_URL").or(paperless_url).unwrap_or_default();
            format!(
                "<meta name=\"paperrust_share_mode\" content=\"lan\">
	<meta name=\"paperrust_share_url\" content=\"{}\">",
                esc(url)
            )
        }
        _ => String::new(),
    }
}

/// Render `documents/templates/index.html`.
pub fn render_index(c: &IndexContext) -> String {
    let fl = &c.frontend_lang;
    let st = |file: &str| static_url(&c.static_base, &format!("frontend/{fl}/{file}"));
    let mut messages = String::new();
    for (level, text) in &c.messages {
        messages.push_str(&format!(
            "\n\t\t\t\t\t{{ level: \"{}\", message: \"{}\" }},\n\t\t\t\t",
            pl_cli::openapi::escapejs(level),
            pl_cli::openapi::escapejs(text)
        ));
    }
    let text = |i: usize| -> String {
        let t = tr(&c.ui_lang, i);
        match &c.product_name {
            Some(p) => pl_accounts::tpl::with_site_name(t, p),
            None => t.to_owned(),
        }
    };
    let value = |name: &str| -> String {
        match name {
            "base" => pl_accounts::tpl::escape(&c.base),
            "cookie_prefix" => pl_accounts::tpl::escape(&c.cookie_prefix),
            "webmanifest" => st("manifest.webmanifest"),
            "styles_css" => st("styles.css"),
            "apple_touch_icon" => st("apple-touch-icon.png"),
            "polyfills_js" => st("polyfills.js"),
            "main_js" => st("main.js"),
            "t0" => text(0),
            "t1" => text(1),
            "t2" => text(2),
            "messages" => messages.clone(),
            _ => String::new(),
        }
    };
    let template = if c.product_name.is_some() { TEMPLATE_PAPERRUST } else { TEMPLATE };
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                out.push_str(&value(&after[..j]));
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    if c.product_name.is_some()
        && !c.share_meta.is_empty()
        && let Some(i) = out.find("</head>")
    {
        out.insert_str(
            i,
            &format!(
                "	{}
",
                c.share_meta
            ),
        );
    }
    out
}

/// `redirect_to_login(request.get_full_path(), LOGIN_URL)`.
pub fn login_redirect_location(login_url: &str, raw_path: &str, raw_query: Option<&str>) -> String {
    let path = pl_core::urlenc::escape_uri_path(&pl_core::urlenc::decode_request_path(raw_path));
    let full = match raw_query {
        Some(q) if !q.is_empty() => format!("{path}?{}", pl_core::urlenc::iri_to_uri(q)),
        _ => path,
    };
    let next = format!("next={}", pl_core::urlenc::quote(&full, "/"));
    match login_url.split_once('?') {
        Some((base, q)) if !q.is_empty() => format!("{base}?{q}&{next}"),
        Some((base, _)) => format!("{base}?{next}"),
        None => format!("{login_url}?{next}"),
    }
}

/// `IndexView` (`/` and every old Angular SPA path). PaperRust: the Angular UI is gone – always
/// `302` to the same path below `/rs/app/` (the light UI logs in by itself; without a light UI
/// build it shows a notice, [`crate::light_ui::missing_response`]). Test log 2026-11-16 D-100:
/// before, without the light UI the old Angular index was served and never finished loading.
/// [`render_index`] stays for reference/tests only.
pub async fn index_view(_st: AppState, req: Request) -> Response {
    crate::light_ui::legacy_redirect(req.uri().path(), req.uri().query())
}

/// `/api` or below.
pub fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// Response for a path without a route (or a case PaperRust does not support): Django's 404 page
/// (`DEBUG=False`, no 404.html), or DRF's `{"detail": "Not found."}` below `/api/`.
pub fn not_found(path: &str) -> Response {
    if is_api_path(path) {
        pl_core::ApiError::not_found().into_response()
    } else {
        html(
            axum::http::StatusCode::NOT_FOUND,
            "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Not Found</title>\n</head>\n<body>\n  <h1>Not Found</h1><p>The requested resource was not found on this server.</p>\n</body>\n</html>\n".into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages() {
        assert_eq!(frontend_language("en-us"), "en-US");
        assert_eq!(frontend_language("de-de"), "de-DE");
        assert_eq!(frontend_language("sr-cs"), "sr-CS");
        assert_eq!(frontend_language("de"), "de");
    }

    #[test]
    fn login_redirect() {
        assert_eq!(login_redirect_location("/accounts/login/", "/", None), "/accounts/login/?next=/");
        assert_eq!(
            login_redirect_location("/accounts/login/", "/documents", Some("a=1&b=%C3%A4")),
            "/accounts/login/?next=/documents%3Fa%3D1%26b%3D%25C3%25A4"
        );
        assert_eq!(login_redirect_location("/accounts/login/", "/a%20b", None), "/accounts/login/?next=/a%2520b");
    }

    #[test]
    fn template_placeholders_only_ours() {
        let names = |t: &'static str| -> Vec<&'static str> {
            t.match_indices("{{")
                .map(|(i, _)| {
                    let rest = &t[i + 2..];
                    &rest[..rest.find("}}").unwrap_or(0)]
                })
                .collect()
        };
        assert_eq!(names(TEMPLATE).len(), 11, "{:?}", names(TEMPLATE));
        assert_eq!(names(TEMPLATE_PAPERRUST), names(TEMPLATE));
    }

    #[test]
    fn render_like_django() {
        let c = IndexContext {
            base: "/".into(),
            cookie_prefix: "".into(),
            static_base: "/static/".into(),
            frontend_lang: "en-US".into(),
            ui_lang: "de-de".into(),
            messages: vec![("success".into(), "Successfully signed in as admin.".into())],
            product_name: None,
            share_meta: String::new(),
        };
        let out = render_index(&c);
        assert!(out.starts_with("<!doctype html>\n\n\n\n<html lang=\"en\" data-bs-theme=\"auto\">"));
        assert!(
            out.contains("<link rel=\"manifest\" href=\"/static/frontend/en-US/manifest.webmanifest\" crossorigin=\"use-credentials\">")
        );
        assert!(out.contains("<h6 class=\"m-auto\">Paperless-ngx wird geladen...</h6>"));
        assert!(out.contains("window.DJANGO_MESSAGES = [\n\t\t\t\t\n\t\t\t\t\t{ level: \"success\", message: \"Successfully signed in as admin.\" },\n\t\t\t\t\n\t\t\t]"));
        let empty = render_index(&IndexContext { messages: vec![], ..c });
        assert!(empty.contains("window.DJANGO_MESSAGES = [\n\t\t\t\t\n\t\t\t]"));
        assert!(!empty.contains("{{"));
    }

    #[test]
    fn render_branded() {
        let c = IndexContext {
            base: "/".into(),
            cookie_prefix: "".into(),
            static_base: "/static/".into(),
            frontend_lang: "en-US".into(),
            ui_lang: "de-de".into(),
            messages: vec![],
            product_name: Some("PaperRust".into()),
            share_meta: String::new(),
        };
        let out = render_index(&c);
        assert!(!out.contains("paperrust_share"), "server operation: no share meta");
        assert!(out.contains("<title>PaperRust</title>"));
        assert!(out.contains("<h6 class=\"m-auto\">PaperRust wird geladen...</h6>"));
        assert!(!out.contains("Paperless-ngx wird") && !out.contains("docs.paperless-ngx.com"));
        assert!(
            out.contains("<link rel=\"manifest\" href=\"/static/frontend/en-US/manifest.webmanifest\" crossorigin=\"use-credentials\">")
        );
        assert!(!out.contains("{{"));
    }

    #[test]
    fn share_meta_only_in_desktop_mode() {
        use pl_core::config::Env;
        assert_eq!(share_meta(&Env::from_pairs(Vec::<(String, String)>::new()), Some("http://x")), "");
        let local = share_meta(&Env::from_pairs([("PAPERRUST_SHARE_MODE", "local")]), Some("http://127.0.0.1:58080"));
        assert_eq!(local, "<meta name=\"paperrust_share_mode\" content=\"local\">");
        let lan = share_meta(&Env::from_pairs([("PAPERRUST_SHARE_MODE", "lan")]), Some("http://192.168.1.20:58080"));
        assert!(
            lan.contains("content=\"lan\"") && lan.contains("<meta name=\"paperrust_share_url\" content=\"http://192.168.1.20:58080\">")
        );
        let base = IndexContext {
            base: "/".into(),
            cookie_prefix: "".into(),
            static_base: "/static/".into(),
            frontend_lang: "en-US".into(),
            ui_lang: "en-us".into(),
            messages: vec![],
            product_name: Some("PaperRust".into()),
            share_meta: local.clone(),
        };
        let out = render_index(&base);
        assert!(
            out.contains(
                "	<meta name=\"paperrust_share_mode\" content=\"local\">
</head>"
            ),
            "{out}"
        );
        // upstream template (branding off) stays byte for byte
        let plain = render_index(&IndexContext { product_name: None, ..base });
        assert!(!plain.contains("paperrust_share"));
    }

    #[test]
    fn index_decision() {
        let on = UiFlags { index: true, static_files: true };
        assert!(index_handles(on, &Method::GET, "/dashboard"));
        assert!(index_handles(on, &Method::HEAD, "/documents/1/details"));
        assert!(!index_handles(on, &Method::POST, "/dashboard"));
        assert!(!index_handles(on, &Method::GET, "/admin/"));
        assert!(index_handles(on, &Method::GET, "/static/missing.js"));
        assert!(!index_handles(UiFlags { index: true, static_files: false }, &Method::GET, "/static/x.js"));
        assert!(!index_handles(UiFlags::default(), &Method::GET, "/dashboard"));
        // F5: unknown API paths get the JSON 404, not the web UI
        assert!(!index_handles(on, &Method::GET, "/api/foo/"));
        assert!(!index_handles(on, &Method::GET, "/api/users/2"));
        assert!(!index_handles(on, &Method::GET, "/api"));
        assert!(index_handles(on, &Method::GET, "/apix/"));
    }
}

#[cfg(test)]
mod matchit_tests {
    use axum::Router;
    use axum::body::Body;
    use axum::routing::{any, get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn static_catch_all_next_to_sidecar_route() {
        let app: Router = Router::new()
            .route("/static/drf_spectacular_sidecar/swagger-ui-dist/{file}", get(|| async { "sidecar" }))
            .route("/static/{*path}", any(|| async { "static" }))
            .route("/", get(|| async { "root" }))
            .fallback(|| async { "fallback" });
        for (p, want) in [
            ("/static/frontend/de-DE/main.js", "static"),
            ("/static/base.css", "static"),
            ("/static/drf_spectacular_sidecar/swagger-ui-dist/x.css", "sidecar"),
            ("/dashboard", "fallback"),
        ] {
            let r = app.clone().oneshot(axum::http::Request::get(p).body(Body::empty()).unwrap()).await.unwrap();
            let b = http_body_util::BodyExt::collect(r.into_body()).await.unwrap().to_bytes();
            assert_eq!(&b[..], want.as_bytes(), "{p}");
        }
    }
}
