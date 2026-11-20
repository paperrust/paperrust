//! pl-auth-api – `/api/token/`, `/api/profile/*`, `/api/users/`, `/api/groups/`,
//! `/api/ui_settings/`. See docs/ARCHITECTURE.md and docs/contract/pl-auth-api.md (contract).
//!
//! Not ported here: `/api/profile/social_account_providers/` (W23: in pl-accounts), `DELETE /api/users/{id}/`,
//! `/api/auth/headless/*`, `/accounts/*`, OPTIONS.

/// Python-compatible JSON error texts (`pl_core::pyjson`, moved from pl-fields-views by W11).
pub use pl_core::pyjson;
pub mod db;
pub mod django_cache;
pub mod fields;
pub mod groups;
pub mod http;
pub mod password;
pub mod profile;
pub mod qr;
pub mod session;
pub mod social;
pub mod throttle;
pub mod token;
pub mod totp;
pub mod ui_settings;
pub mod users;

use axum::http::{HeaderValue, Method, header::ALLOW};
use axum::response::Response;
use axum::routing::{MethodFilter, MethodRouter, delete, get, on, patch, post, put};
use pl_core::route::{ALLOW_MODEL_DETAIL, ALLOW_MODEL_LIST, ALLOW_POST_ONLY, drf_view};
use pl_core::{ApiRoute, AppState};

const ALLOW_PROFILE: &str = "GET, PATCH, HEAD, OPTIONS";
const ALLOW_TOTP: &str = "GET, POST, DELETE, HEAD, OPTIONS";
const ALLOW_UI_SETTINGS: &str = "GET, POST, HEAD, OPTIONS";

/// Methods dispatched to `PaperlessObtainAuthTokenView` (all of Django's `http_method_names`;
/// HEAD is routed with GET).
const TOKEN_METHODS: MethodFilter = MethodFilter::GET
    .or(MethodFilter::POST)
    .or(MethodFilter::PUT)
    .or(MethodFilter::PATCH)
    .or(MethodFilter::DELETE)
    .or(MethodFilter::OPTIONS)
    .or(MethodFilter::TRACE);

/// `ObtainAuthToken` has a single renderer → DRF sets `Allow` but no `Vary: Accept`.
fn allow_only(handler: MethodRouter<AppState>, allow: &'static str) -> MethodRouter<AppState> {
    handler.layer(axum::middleware::map_response(move |mut resp: Response| async move {
        resp.headers_mut().insert(ALLOW, HeaderValue::from_static(allow));
        resp
    }))
}

/// All routes of this module. The server mounts only those enabled in `routes.toml`;
/// alles andere geht per Proxy an Django.
pub fn routes() -> Vec<ApiRoute> {
    // Every body is parsed by `http::parse_data` (DRF JSON/Form/MultiPart parsers).
    let routes = vec![
        ApiRoute::new(
            "auth.token",
            Method::POST,
            "/api/token/",
            "src/paperless/views.py::PaperlessObtainAuthTokenView",
            // Every method reaches the view (DRF: throttle in `initial()`, then 405/OPTIONS).
            allow_only(on(TOKEN_METHODS, token::obtain_token), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "profile.get",
            Method::GET,
            "/api/profile/",
            "src/paperless/views.py::ProfileView.get",
            drf_view(get(profile::get_profile), ALLOW_PROFILE),
        ),
        ApiRoute::new(
            "profile.patch",
            Method::PATCH,
            "/api/profile/",
            "src/paperless/views.py::ProfileView.patch",
            drf_view(patch(profile::patch_profile), ALLOW_PROFILE),
        ),
        ApiRoute::new(
            "profile.generate_auth_token",
            Method::POST,
            "/api/profile/generate_auth_token/",
            "src/paperless/views.py::GenerateAuthTokenView.post",
            drf_view(post(profile::generate_auth_token), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "profile.disconnect_social_account",
            Method::POST,
            "/api/profile/disconnect_social_account/",
            "src/paperless/views.py::DisconnectSocialAccountView.post",
            drf_view(post(profile::disconnect_social_account), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "profile.totp.get",
            Method::GET,
            "/api/profile/totp/",
            "src/paperless/views.py::TOTPView.get",
            drf_view(get(profile::totp_get), ALLOW_TOTP),
        ),
        ApiRoute::new(
            "profile.totp.post",
            Method::POST,
            "/api/profile/totp/",
            "src/paperless/views.py::TOTPView.post",
            drf_view(post(profile::totp_post), ALLOW_TOTP),
        ),
        ApiRoute::new(
            "profile.totp.delete",
            Method::DELETE,
            "/api/profile/totp/",
            "src/paperless/views.py::TOTPView.delete",
            drf_view(delete(profile::totp_delete), ALLOW_TOTP),
        ),
        ApiRoute::new(
            "users.list",
            Method::GET,
            "/api/users/",
            "src/paperless/views.py::UserViewSet.list",
            drf_view(get(users::list), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "users.create",
            Method::POST,
            "/api/users/",
            "src/paperless/views.py::UserViewSet.create",
            drf_view(post(users::create), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "users.retrieve",
            Method::GET,
            "/api/users/{id}/",
            "src/paperless/views.py::UserViewSet.retrieve",
            drf_view(get(users::retrieve), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "users.update",
            Method::PUT,
            "/api/users/{id}/",
            "src/paperless/views.py::UserViewSet.update",
            drf_view(put(users::update), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "users.partial_update",
            Method::PATCH,
            "/api/users/{id}/",
            "src/paperless/views.py::UserViewSet.partial_update",
            drf_view(patch(users::partial_update), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "users.destroy",
            Method::DELETE,
            "/api/users/{id}/",
            "src/paperless/views.py::UserViewSet.destroy",
            drf_view(delete(users::destroy), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "users.deactivate_totp",
            Method::POST,
            "/api/users/{id}/deactivate_totp/",
            "src/paperless/views.py::UserViewSet.deactivate_totp",
            drf_view(post(users::deactivate_totp), ALLOW_POST_ONLY),
        ),
        ApiRoute::new(
            "groups.list",
            Method::GET,
            "/api/groups/",
            "src/paperless/views.py::GroupViewSet.list",
            drf_view(get(groups::list), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "groups.create",
            Method::POST,
            "/api/groups/",
            "src/paperless/views.py::GroupViewSet.create",
            drf_view(post(groups::create), ALLOW_MODEL_LIST),
        ),
        ApiRoute::new(
            "groups.retrieve",
            Method::GET,
            "/api/groups/{id}/",
            "src/paperless/views.py::GroupViewSet.retrieve",
            drf_view(get(groups::retrieve), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "groups.update",
            Method::PUT,
            "/api/groups/{id}/",
            "src/paperless/views.py::GroupViewSet.update",
            drf_view(put(groups::update), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "groups.partial_update",
            Method::PATCH,
            "/api/groups/{id}/",
            "src/paperless/views.py::GroupViewSet.partial_update",
            drf_view(patch(groups::partial_update), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "groups.destroy",
            Method::DELETE,
            "/api/groups/{id}/",
            "src/paperless/views.py::GroupViewSet.destroy",
            drf_view(delete(groups::destroy), ALLOW_MODEL_DETAIL),
        ),
        ApiRoute::new(
            "ui_settings.get",
            Method::GET,
            "/api/ui_settings/",
            "src/documents/views.py::UiSettingsView.get",
            drf_view(get(ui_settings::get), ALLOW_UI_SETTINGS),
        ),
        ApiRoute::new(
            "ui_settings.post",
            Method::POST,
            "/api/ui_settings/",
            "src/documents/views.py::UiSettingsView.post",
            drf_view(post(ui_settings::post), ALLOW_UI_SETTINGS),
        ),
    ];
    routes.into_iter().map(ApiRoute::accepts_drf_parsers).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_ids_unique() {
        let r = super::routes();
        let mut ids: Vec<_> = r.iter().map(|r| r.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
