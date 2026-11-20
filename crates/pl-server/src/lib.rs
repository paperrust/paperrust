//! PaperRust server: mounts every Rust route (switched off ones in `routes.toml` excepted) behind a
//! Django-like middleware layer. There is no Django backend any more (since 2026-11-14).
//! Owner: server agent (wave 0). Contract: docs/contract/pl-server.md.
//!
//! upstream: src/paperless/urls.py@v3.4.2, src/paperless/middleware.py@v3.4.2,
//! src/paperless/settings/__init__.py@v3.4.2 (MIDDLEWARE)

pub mod api_v2;
pub mod body;
pub mod branding;
pub mod cli;
pub mod compress;
pub mod config;
/// Start checks: database reachable and migrated (test log 2026-11-16).
pub mod dbcheck;
pub mod detach;
pub mod django_mw;
pub mod docs;
/// `OPTIONS` metadata of DRF views (`SimpleMetadata`).
pub mod drf_options;
pub mod frontend;
pub mod frontline;
pub mod light_ui;
pub mod logfile;
/// Regularly return free heap (base memory after ingestion).
pub mod memtrim;
pub mod routing;
pub mod search_sync;
pub mod serve;
/// Settings from the UI in the running process (docs/SETTINGS.md WP4).
pub mod settings_live;
pub mod shutdown;
pub mod upload;
pub mod worker;
pub mod ws;

/// Global allocator with feature `mimalloc` (O1 measurement: docs/contract/pl-server.md).
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Language negotiation lives in pl-core (one implementation for server and handlers).
pub use pl_core::locale;

use std::sync::Arc;

use axum::Router;
use pl_core::{ApiRoute, AppState};

use crate::config::ServerConfig;
use crate::frontline::Frontline;
use crate::routing::{MountPlan, RoutesError};

/// Routes of all domain crates (`routes()` of each crate, see docs/ARCHITECTURE.md).
pub fn all_routes() -> Vec<ApiRoute> {
    let mut v = Vec::new();
    v.extend(pl_authz::routes());
    v.extend(pl_auth_api::routes());
    v.extend(pl_objects::routes());
    v.extend(pl_documents::routes());
    v.extend(pl_docfilter::routes());
    v.extend(pl_files::routes());
    v.extend(pl_documents_write::routes());
    v.extend(pl_fields_views::routes());
    v.extend(pl_workflows::routes());
    v.extend(pl_search::routes());
    v.extend(pl_system::routes());
    v.extend(pl_mail::routes());
    // Account pages (allauth replacement, W3): login, logout, MFA, password reset, signup.
    v.extend(pl_accounts::routes());
    #[cfg(feature = "ai")]
    v.extend(pl_ai::routes());
    // OpenAPI schema + Swagger UI (pl-cli, W7): schema.get, schema.swagger_ui, static.swagger_ui_dist.
    v.extend(pl_cli::routes());
    // Upload needs the worker's task queue (PgQueue), so it lives here (W1).
    v.extend(upload::routes());
    // Scanner integration (eSCL scanning, devices for SMTP intake): /api/x-scan/…
    v.extend(pl_scan::routes());
    // Ransomware protection (pl-guard): status/banner, events, lift read-only mode, backups.
    v.extend(pl_guard::routes());
    // Angular frontend without Django (W22): ui.index, static.files, RedirectViews.
    v.extend(frontend::routes());
    v.extend(docs::routes());
    // pl-templating, pl-matching, pl-queue, pl-consumer, pl-indexer, pl-classifier, pl-parsers,
    // pl-ocr: libraries without routes.
    v
}

/// Assemble the complete application: active Rust routes (404 for the rest) + Django-like
/// middleware + internal health endpoint.
pub fn build_app(
    cfg: &ServerConfig,
    mut plan: MountPlan,
    state: AppState,
) -> Result<Router, RoutesError> {
    if frontend::UiFlags::from_ids(plan.active.iter().map(|r| r.id)).static_files {
        plan.active.retain(|r| {
            let drop = frontend::SUPERSEDED_BY_STATIC.contains(&r.id);
            if drop {
                tracing::info!(id = r.id, "route superseded by static.files (WhiteNoise for all of /static/)");
            }
            !drop
        });
    }
    let rust_routes = plan.active.len();
    let ui = frontend::UiFlags::from_ids(plan.active.iter().map(|r| r.id));
    state.set_service(ui);
    if ui.static_files
        && state.service::<Arc<frontend::statics::StaticFiles>>().is_none()
        && state.service::<frontend::statics::StaticScan>().is_none()
    {
        let c = state.config();
        // WHITENOISE_MAX_AGE: 0 with DEBUG, else 60
        let max_age = if c.debug { 0 } else { 60 };
        if tokio::runtime::Handle::try_current().is_ok() {
            // O1: in the background – the server listens meanwhile, `/static/…` waits for it.
            state.set_service(frontend::statics::StaticScan::spawn(c.static_root.clone(), Some(max_age), c.tz));
        } else {
            let t0 = std::time::Instant::now();
            let files = frontend::statics::StaticFiles::scan(&c.static_root, Some(max_age), &c.tz);
            tracing::info!(dir = %files.root.display(), files = files.len(), ms = t0.elapsed().as_millis() as u64, "static files (STATIC_ROOT) scanned");
            state.set_service(Arc::new(files));
        }
    }
    let mut router = routing::build_router(plan)?;
    // Native API v2 (`/rs/api/v2/`, P3): fixed mount before the fallback, not in
    // routes.toml; `PAPERRUST_API_V2=off` switches it off.
    if cfg.api_v2 {
        // "Automation" area (pl-automation) before the v2 router: the more specific prefix wins.
        router = router.nest(pl_automation::PREFIX, pl_automation::router());
        router = router.nest(pl_api_v2::PREFIX, api_v2::router(&state));
    }
    // Attach the AI condition of the workflows (pl-consumer workflow_ext) to pl-ai (without `ai`: none).
    pl_automation::ai::install(&state);
    // Light-UI (`/rs/app/`, P4/P5): fixed mount like API v2; `PAPERRUST_LIGHT_UI=off` switches it
    // off, `PAPERRUST_LIGHT_UI_DIR` (default `<STATICDIR>/../light`) is the Vite build.
    // `/` and the old Angular paths always redirect to `/rs/app/…` (D-100); without a build, `/rs/app/`
    // answers with a clear page instead of the old Angular index that never finished loading.
    let mut mounted = false;
    match light_ui::configured_dir(pl_core::config::var, &state.config().static_root) {
        Some(dir) => match light_ui::LightUi::scan(&dir) {
            Some(mut ui) => {
                let c = state.config();
                ui.inject_head(&frontend::share_meta(&c.env, c.paperless_url.as_deref()));
                tracing::info!(dir = %ui.root.display(), files = ui.len(), "light UI mounted at /rs/app/");
                router = router.merge(light_ui::router(Arc::new(ui)));
                state.set_service(light_ui::Mounted);
                mounted = true;
            }
            None => tracing::warn!(
                dir = %dir.display(),
                "no web UI build (index.html missing): /rs/app/ shows a notice; set PAPERRUST_LIGHT_UI_DIR"
            ),
        },
        None => tracing::info!("web UI switched off (PAPERRUST_LIGHT_UI=off): /rs/app/ shows a notice"),
    }
    if !mounted {
        router = router.merge(light_ui::missing_router());
    }
    // DRF authentication for the handlers (pl_authz::AuthUser / MaybeUser).
    let authz = pl_authz::Authz::new(
        state.db().clone(),
        pl_authz::AuthConfig::from_core(state.config()),
    );
    let bus = state.service::<pl_queue::EventBus>();
    let router = router
        .with_state(state)
        // FINDINGS B10: writing requests (POST/PUT/PATCH/DELETE) run to completion as their own task,
        // even if the client closes the connection (workflows, search index, WebSocket after the commit).
        .layer(axum::middleware::from_fn(crate::detach::middleware))
        .layer(axum::Extension(authz.clone()))
        // Handlers never see more than Django's DATA_UPLOAD_MAX_MEMORY_SIZE (body::gate answers
        // larger bodies with 400); axum's own extractor limit must not be lower.
        .layer(axum::extract::DefaultBodyLimit::max(
            pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE,
        ))
        // S6: Django's CsrfViewMiddleware also accepts the `csrfmiddlewaretoken` form field. Reads
        // it from form POSTs without `X-CSRFToken` and hands it to the authentication extractor
        // (pl_authz::csrf::FormCsrfToken), which runs the CSRF check inside the handlers.
        .layer(axum::middleware::from_fn(pl_authz::csrf::form_token_middleware));
    // Native `/ws/status/` (events come from the Rust worker over the event bus).
    let status_ws = bus.map(|bus| frontline::StatusSocket { authz: authz.clone(), bus });
    let front = Arc::new(Frontline {
        settings: cfg.settings.clone(),
        rust_routes,
        branding: cfg.branding.clone(),
        status_ws,
    });
    let inner = router
        .layer(axum::middleware::from_fn_with_state(
            front,
            frontline::frontline,
        ))
        // Read-only mode after a ransomware alarm (pl-guard): writing API requests → 423, also
        // for paths without a route.
        .layer(axum::middleware::from_fn(pl_guard::readonly::middleware))
        // tests/chaos C1: database unreachable → API immediately 503 instead of waiting 30 s and 500.
        .layer(axum::middleware::from_fn(crate::dbcheck::unavailable_middleware));
    // Outermost: axum's MethodRouter inserts `Allow` into fallback (404) responses after all
    // per-route layers ran, so the cleanup must wrap the whole router as one service.
    Ok(Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .map_response(routing::strip_injected_allow)
            // `re_path` views without `$` (e.g. `^statistics/`) also answer longer paths – route
            // them to the Rust view (docs/STANDALONE-GAPS.md §5).
            .map_request(crate::frontend::urlconf::rewrite_prefix_view)
            // DRF format suffixes of router routes (`/api/tags/5.json/`, `/api/tags/1.5/`).
            .map_request(crate::frontend::urlconf::rewrite_format_suffix)
            .service(inner),
    ))
}
