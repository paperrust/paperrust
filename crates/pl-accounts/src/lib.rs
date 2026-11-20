//! pl-accounts – account pages of django-allauth 65.20.1, as paperless-ngx v3.4.2 uses them
//! (`/accounts/*`, `/api/auth/login|logout/`), with Django-compatible sessions.
//! Contract: docs/contract/pl-accounts.md.
//!
//! OIDC login (`/accounts/oidc/<id>/login/`, `.../callback/`), OAuth2 providers GitHub/GitLab/
//! Google/Microsoft/Nextcloud/Discord/Gitea (`/accounts/<id>/login/`, `.../callback/`) and `/accounts/3rdparty/*`:
//! see `socialaccount.rs`/`oauth2.rs`; email verification `none|optional|mandatory`: see
//! `flow.rs`/`emailaddr.rs`. Not reimplemented: other social providers (skipped with a note in
//! the log), headless flows (`/api/auth/headless/*`, not used by the frontend),
//! Passkeys/WebAuthn.

pub mod api;
pub mod auth;
pub mod cache;
/// Automatic login in desktop mode (`/rs/desktop/login`, PaperRust extension).
pub mod desktop;
pub mod emailaddr;
pub mod flow;
pub mod i18n;
pub mod mail;
pub mod oauth2;
pub mod oidc;
pub mod messages;
pub mod page;
pub mod ratelimit;
pub mod reset;
pub mod session;
pub mod settings;
pub mod signup;
pub mod social;
pub mod socialaccount;
pub mod tpl;
pub mod views;
pub mod web;

use axum::http::Method;
use axum::routing::{get, post};
use pl_core::ApiRoute;

/// All routes of this crate (ids for `routes.toml`).
pub fn routes() -> Vec<ApiRoute> {
    let r = |id, m: Method, path, up, h| ApiRoute::new(id, m, path, up, h);
    vec![
        r("accounts.login.get", Method::GET, "/accounts/login/", "allauth/account/views.py::LoginView.get", get(views::login)),
        r("accounts.login.post", Method::POST, "/accounts/login/", "allauth/account/views.py::LoginView.post", post(views::login)).accepts_form().delegatable(),
        r("accounts.logout.get", Method::GET, "/accounts/logout/", "allauth/account/views.py::LogoutView.get", get(views::logout)),
        r("accounts.logout.post", Method::POST, "/accounts/logout/", "allauth/account/views.py::LogoutView.post", post(views::logout)).accepts_form(),
        r("api_auth.login.get", Method::GET, "/api/auth/login/", "src/paperless/urls.py (rest_framework:login → allauth LoginView)", get(views::login)),
        r("api_auth.login.post", Method::POST, "/api/auth/login/", "src/paperless/urls.py (rest_framework:login → allauth LoginView)", post(views::login)).accepts_form().delegatable(),
        r("api_auth.logout.get", Method::GET, "/api/auth/logout/", "src/paperless/urls.py (rest_framework:logout → allauth LogoutView)", get(views::logout)),
        r("api_auth.logout.post", Method::POST, "/api/auth/logout/", "src/paperless/urls.py (rest_framework:logout → allauth LogoutView)", post(views::logout)).accepts_form(),
        // PaperRust: automatic login in desktop mode (loopback only, only with supervisor key).
        r(desktop::ROUTE_ID, Method::GET, desktop::PATH, "PaperRust extension (desktop auto-login, docs/DESKTOP.md)", get(desktop::desktop_login)),
        r("accounts.account_inactive", Method::GET, "/accounts/account_inactive/", "allauth/account/views.py::AccountInactiveView", get(views::account_inactive)),
        r("accounts.mfa_authenticate.get", Method::GET, "/accounts/2fa/authenticate/", "allauth/mfa/base/views.py::AuthenticateView.get", get(views::mfa_authenticate)),
        r("accounts.mfa_authenticate.post", Method::POST, "/accounts/2fa/authenticate/", "allauth/mfa/base/views.py::AuthenticateView.post", post(views::mfa_authenticate)).accepts_form(),
        r("accounts.signup.get", Method::GET, "/accounts/signup/", "allauth/account/views.py::SignupView.get", get(signup::signup)).delegatable(),
        r("accounts.signup.post", Method::POST, "/accounts/signup/", "allauth/account/views.py::SignupView.post", post(signup::signup)).accepts_form().delegatable(),
        r("accounts.password_reset.get", Method::GET, "/accounts/password/reset/", "allauth/account/views.py::PasswordResetView.get", get(reset::password_reset)),
        r("accounts.password_reset.post", Method::POST, "/accounts/password/reset/", "allauth/account/views.py::PasswordResetView.post", post(reset::password_reset)).accepts_form(),
        r("accounts.password_reset_done", Method::GET, "/accounts/password/reset/done/", "allauth/account/views.py::PasswordResetDoneView", get(reset::password_reset_done)),
        r("accounts.password_reset_from_key.get", Method::GET, "/accounts/password/reset/key/{key}/", "allauth/account/views.py::PasswordResetFromKeyView.get", get(reset::from_key)),
        r("accounts.password_reset_from_key.post", Method::POST, "/accounts/password/reset/key/{key}/", "allauth/account/views.py::PasswordResetFromKeyView.post", post(reset::from_key)).accepts_form(),
        r("accounts.password_reset_from_key_done", Method::GET, "/accounts/password/reset/key/done/", "allauth/account/views.py::PasswordResetFromKeyDoneView", get(reset::from_key_done)),
        r("accounts.oidc_login.get", Method::GET, "/accounts/oidc/{provider_id}/login/", "allauth/socialaccount/providers/openid_connect/views.py::login (OAuth2LoginView, LOGIN_ON_GET=False)", get(socialaccount::oidc_login)).delegatable(),
        r("accounts.oidc_login.post", Method::POST, "/accounts/oidc/{provider_id}/login/", "allauth/socialaccount/providers/openid_connect/views.py::login (redirect_from_request)", post(socialaccount::oidc_login)).accepts_form().delegatable(),
        r("accounts.oidc_callback.get", Method::GET, "/accounts/oidc/{provider_id}/login/callback/", "allauth/socialaccount/providers/openid_connect/views.py::callback (OAuth2CallbackView)", get(socialaccount::oidc_callback)).delegatable(),
        r("accounts.oidc_callback.post", Method::POST, "/accounts/oidc/{provider_id}/login/callback/", "allauth/socialaccount/providers/openid_connect/views.py::callback (OAuth2CallbackView)", post(socialaccount::oidc_callback)).accepts_form().delegatable(),
        r("accounts.provider_login.get", Method::GET, "/accounts/{provider}/login/", "allauth/socialaccount/providers/oauth2/views.py::OAuth2LoginView (github, gitlab, google, microsoft, nextcloud)", get(socialaccount::provider_login)),
        r("accounts.provider_login.post", Method::POST, "/accounts/{provider}/login/", "allauth/socialaccount/providers/oauth2/views.py::OAuth2LoginView (redirect_from_request)", post(socialaccount::provider_login)).accepts_form(),
        r("accounts.provider_callback.get", Method::GET, "/accounts/{provider}/login/callback/", "allauth/socialaccount/providers/oauth2/views.py::OAuth2CallbackView", get(socialaccount::provider_callback)),
        r("accounts.provider_callback.post", Method::POST, "/accounts/{provider}/login/callback/", "allauth/socialaccount/providers/oauth2/views.py::OAuth2CallbackView", post(socialaccount::provider_callback)).accepts_form(),
        r("accounts.social_signup.get", Method::GET, "/accounts/3rdparty/signup/", "allauth/socialaccount/views.py::SignupView.get", get(socialaccount::signup)).delegatable(),
        r("accounts.social_signup.post", Method::POST, "/accounts/3rdparty/signup/", "allauth/socialaccount/views.py::SignupView.post", post(socialaccount::signup)).accepts_form().delegatable(),
        r("accounts.social_login_cancelled", Method::GET, "/accounts/3rdparty/login/cancelled/", "allauth/socialaccount/views.py::LoginCancelledView", get(socialaccount::login_cancelled)),
        r("accounts.email_verification_sent", Method::GET, "/accounts/confirm-email/", "allauth/account/views.py::email_verification_sent (EmailVerificationSentView)", get(emailaddr::verification_sent)),
        r("accounts.confirm_email.get", Method::GET, "/accounts/confirm-email/{key}/", "allauth/account/views.py::ConfirmEmailView.get", get(emailaddr::confirm_email)).delegatable(),
        r("accounts.confirm_email.post", Method::POST, "/accounts/confirm-email/{key}/", "allauth/account/views.py::ConfirmEmailView.post", post(emailaddr::confirm_email)).accepts_form().delegatable(),
        ApiRoute::new(
            "profile.social_account_providers",
            Method::GET,
            "/api/profile/social_account_providers/",
            "src/paperless/views.py::SocialAccountProvidersView.get",
            pl_core::route::drf_view(get(api::social_account_providers), pl_core::route::ALLOW_READ_ONLY),
        )
        .delegatable(),
        r("accounts.social_login_error", Method::GET, "/accounts/3rdparty/login/error/", "allauth/socialaccount/views.py::LoginErrorView", get(socialaccount::login_error)),
    ]
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

    /// The OAuth2 provider paths (`/accounts/{provider}/login/`) coexist with the fixed
    /// account paths and the OIDC pattern in the axum router (no conflict panic).
    #[test]
    fn routes_mount_without_conflict() {
        let mut by_path: std::collections::BTreeMap<&str, axum::routing::MethodRouter<pl_core::AppState>> = std::collections::BTreeMap::new();
        for r in super::routes() {
            let h = r.handler;
            let merged = match by_path.remove(r.path) {
                Some(prev) => prev.merge(h),
                None => h,
            };
            by_path.insert(r.path, merged);
        }
        let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
        for (p, h) in by_path {
            router = router.route(p, h);
        }
        let _ = router;
    }
}
