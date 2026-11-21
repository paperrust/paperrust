// upstream: src/paperless/urls.py@v3.4.2 (api/search/, api/search/autocomplete/),
//           src/documents/views.py (SearchAutoCompleteView, GlobalSearchView)
//
// Mounting of the two pl-search routes. GET /api/documents/?query=... is NOT
// mounted here: it is the document list route of pl-documents, which calls
// `views::document_search` (see docs/contract/pl-search.md).

use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pl_authz::AuthUser;
use pl_authz::perms::VIEW_DOCUMENTS_PERMISSIONS;
use pl_core::route::{ALLOW_READ_ONLY, drf_view};
use pl_core::{ApiError, ApiRoute, AppState, QueryParams, RequestContext};

use crate::backend::{SearchBackend, SearchSettings};
use crate::db::RequestUser;
use crate::views::{self, GlobalSearchProviders, ViewError};

static BACKEND: OnceLock<Arc<SearchBackend>> = OnceLock::new();
static PROVIDERS: OnceLock<GlobalSearchProviders> = OnceLock::new();

/// The process-wide read-only backend (one reader per process, like upstream's
/// `get_backend()` singleton). All settings from pl-core's config.
pub fn backend(state: &AppState) -> Arc<SearchBackend> {
    BACKEND.get_or_init(|| Arc::new(SearchBackend::new(settings_from_core(state.config())))).clone()
}

/// [`SearchSettings`] from `pl_core::Config` (index dir, `SEARCH_LANGUAGE`,
/// `ADVANCED_FUZZY_SEARCH_THRESHOLD`, `TIME_ZONE`) – no own environment parsing.
pub fn settings_from_core(cfg: &pl_core::Config) -> SearchSettings {
    SearchSettings {
        index_dir: cfg.index_dir.clone(),
        language: cfg.search_language.clone(),
        fuzzy_threshold: cfg.advanced_fuzzy_search_threshold,
        time_zone: cfg.tz,
    }
}

/// Register the object sections of `/api/search/` (call once at start-up).
/// Returns false when already set.
pub fn set_global_search_providers(p: GlobalSearchProviders) -> bool {
    PROVIDERS.set(p).is_ok()
}

/// `pl_authz::User` -> local request user.
pub fn request_user(user: &pl_authz::User) -> RequestUser {
    RequestUser {
        id: user.id,
        is_superuser: user.is_superuser,
        is_active: user.is_active,
        group_ids: user.groups.clone(),
        perms: user.permissions.clone(),
    }
}

fn view_error(e: ViewError) -> Response {
    ApiError::internal(e).into_response()
}

/// GET /api/search/autocomplete/ – `SearchAutoCompleteView.get`
async fn autocomplete(State(state): State<AppState>, auth: AuthUser, params: QueryParams) -> Response {
    // (IsAuthenticated, ViewDocumentsPermissions)
    if let Err(e) = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS) {
        return e.into_response();
    }
    let user = request_user(auth.user());
    match views::autocomplete_view(backend(&state), &user, params.pairs()).await {
        Ok(r) => r.into_response(),
        Err(e) => view_error(e),
    }
}

/// GET /api/search/ – `GlobalSearchView.get`
async fn global_search(State(state): State<AppState>, auth: AuthUser, rc: RequestContext, params: QueryParams) -> Response {
    let user = request_user(auth.user());
    // Registered providers win (tests / other crates); otherwise the built-in sections.
    let builtin;
    let providers = match PROVIDERS.get() {
        Some(p) => p,
        None => {
            builtin = crate::global::builtin_providers(crate::global::Ctx {
                db: state.db().clone(),
                user: auth.user().clone(),
                tz: *state.tz(),
                api_version: rc.negotiation.version.as_u16(),
                request: crate::global::children_context(rc),
            });
            &builtin
        }
    };
    match views::global_search_view(state.db(), backend(&state), &user, params.pairs(), providers).await {
        Ok(r) => r.into_response(),
        Err(e) => view_error(e),
    }
}

pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute {
            id: "search.autocomplete",
            method: Method::GET,
            path: "/api/search/autocomplete/",
            upstream: "src/documents/views.py::SearchAutoCompleteView.get",
            handler: drf_view(get(autocomplete), ALLOW_READ_ONLY),
        },
        ApiRoute {
            id: "search.global",
            method: Method::GET,
            path: "/api/search/",
            upstream: "src/documents/views.py::GlobalSearchView.get",
            handler: drf_view(get(global_search), ALLOW_READ_ONLY),
        },
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_come_from_core_config() {
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([
            ("PAPERLESS_SECRET_KEY", "pl-search-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
            ("PAPERLESS_OCR_LANGUAGE", "deu+eng"),
            ("PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD", "0.5"),
            ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ]))
        .unwrap();
        let s = super::settings_from_core(&cfg);
        assert_eq!(s.language.as_deref(), Some("de"));
        assert_eq!(s.fuzzy_threshold, Some(0.5));
        assert_eq!(s.time_zone, chrono_tz::Europe::Zurich);
        assert_eq!(s.index_dir, cfg.index_dir);
    }
}
