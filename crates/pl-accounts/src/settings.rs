//! Settings of the account pages, from the same environment variables as upstream.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (ACCOUNT_*, SOCIALACCOUNT_*, MFA_*,
//! SESSION_*, EMAIL_*, DISABLE_REGULAR_LOGIN, REDIRECT_LOGIN_TO_SSO, LOGIN_URL/…_REDIRECT_URL,
//! ALLAUTH_TRUSTED_*), src/paperless/settings/custom.py@v3.4.2 (parse_hosting_settings),
//! allauth 65.20.1 account/app_settings.py (defaults, RATE_LIMITS, LOGIN_TIMEOUT).

use pl_authz::AuthConfig;
use pl_core::Config;
use serde_json::Value;

/// CSRF cookie independent of Django `SESSION_COOKIE_AGE`: `CSRF_COOKIE_AGE` = 1 year.
pub const CSRF_COOKIE_AGE: i64 = 60 * 60 * 24 * 7 * 52;
/// allauth `ACCOUNT_LOGIN_TIMEOUT` (15 min).
pub const LOGIN_TIMEOUT: f64 = 15.0 * 60.0;
/// allauth `ACCOUNT_PASSWORD_RESET_TIMEOUT` = Django `PASSWORD_RESET_TIMEOUT` (3 days).
pub const PASSWORD_RESET_TIMEOUT: i64 = 60 * 60 * 24 * 3;

/// Everything the account pages need in terms of settings.
#[derive(Debug, Clone)]
pub struct Settings {
    pub auth: AuthConfig,
    pub secret_key: String,
    pub csrf_cookie_name: String,
    pub session_cookie_name: String,
    pub language_cookie_name: String,
    pub session_cookie_age: i64,
    pub session_expire_at_browser_close: bool,
    pub base_url: String,
    pub login_url: String,
    pub login_redirect_url: String,
    pub logout_redirect_url: String,
    pub static_url: String,
    pub paperless_url: Option<String>,
    pub allowed_hosts: Vec<String>,
    pub email_enabled: bool,
    pub disable_regular_login: bool,
    pub redirect_login_to_sso: bool,
    pub account_allow_signups: bool,
    pub account_default_groups: Vec<String>,
    pub socialaccount_allow_signups: bool,
    pub socialaccount_auto_signup: bool,
    pub social_account_default_groups: Vec<String>,
    pub social_account_sync_groups: bool,
    pub social_account_sync_groups_claim: String,
    pub social_account_sync_superuser_group: Option<String>,
    pub social_account_sync_staff_group: Option<String>,
    pub social_account_mfa_trusted_amr: Vec<String>,
    pub account_email_verification: String,
    pub account_email_unknown_accounts: bool,
    pub account_default_http_protocol: String,
    pub trusted_proxy_count: i64,
    pub trusted_client_ip_header: Option<String>,
    pub app_title: Option<String>,
    pub app_logo: Option<String>,
    /// Product name in page titles and mails (`pl_core::Config::product_name`, otherwise `PaperRust`).
    pub site_name: String,
    pub auto_login_username: Option<String>,
    pub enable_http_remote_user: bool,
    pub http_remote_user_header_name: String,
    pub email_host: String,
    pub email_port: u16,
    pub email_host_user: String,
    pub email_host_password: String,
    pub default_from_email: String,
    pub email_use_tls: bool,
    pub email_use_ssl: bool,
    /// `SOCIALACCOUNT_PROVIDERS` (JSON).
    pub socialaccount_providers: Value,
    /// `PAPERLESS_APPS` (for installed provider apps).
    pub installed_provider_apps: Vec<String>,
}

/// `SOCIAL_ACCOUNT_MFA_TRUSTED_AMR = get_list_from_env("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR",
/// default=["mfa"])` (upstream fix cb3513c6f, after v3.4.2).
fn list_default_mfa(env: &pl_core::config::Env) -> Vec<String> {
    env.list("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", &["mfa"])
}

impl Settings {
    pub fn from_core(c: &Config) -> Self {
        let env = &c.env;
        let list = |k: &str| env.list(k, &[]);
        let providers = env.json("PAPERLESS_SOCIALACCOUNT_PROVIDERS").ok().flatten().unwrap_or(Value::Object(Default::default()));
        let apps = list("PAPERLESS_APPS");
        Self {
            auth: AuthConfig::from_core(c),
            secret_key: c.secret_key.clone(),
            csrf_cookie_name: c.csrf_cookie_name.clone(),
            session_cookie_name: c.session_cookie_name.clone(),
            language_cookie_name: c.language_cookie_name.clone(),
            session_cookie_age: c.session_cookie_age,
            session_expire_at_browser_close: c.session_expire_at_browser_close,
            base_url: c.base_url.clone(),
            login_url: c.login_url.clone(),
            login_redirect_url: c.login_redirect_url.clone(),
            logout_redirect_url: c.logout_redirect_url.clone(),
            static_url: c.static_url.clone(),
            paperless_url: c.paperless_url.clone(),
            allowed_hosts: c.allowed_hosts.clone(),
            email_enabled: c.email_enabled,
            disable_regular_login: c.disable_regular_login,
            redirect_login_to_sso: c.redirect_login_to_sso,
            account_allow_signups: c.account_allow_signups,
            account_default_groups: c.account_default_groups.clone(),
            socialaccount_allow_signups: c.socialaccount_allow_signups,
            socialaccount_auto_signup: c.socialaccount_auto_signup,
            social_account_default_groups: c.social_account_default_groups.clone(),
            social_account_sync_groups: c.social_account_sync_groups,
            social_account_sync_groups_claim: env.get_or("PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM", "groups"),
            social_account_sync_superuser_group: env.get("PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP").map(String::from),
            social_account_sync_staff_group: env.get("PAPERLESS_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP").map(String::from),
            // upstream fix cb3513c6f (#14693, after v3.4.2): get_list_from_env(key, default=["mfa"]).
            // v3.4.2 passed "mfa" as the *separator* (so the default was the empty list).
            social_account_mfa_trusted_amr: list_default_mfa(env),
            account_email_verification: c.account_email_verification.clone(),
            account_email_unknown_accounts: env.bool("PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS", "True").unwrap_or(true),
            account_default_http_protocol: c.account_default_http_protocol.clone(),
            trusted_proxy_count: c.allauth_trusted_proxy_count,
            trusted_client_ip_header: c.allauth_trusted_client_ip_header.clone(),
            app_title: c.app_title.clone(),
            app_logo: c.app_logo.clone(),
            site_name: c.product_name.clone().unwrap_or_else(|| pl_core::brand::PRODUCT_NAME.to_string()),
            auto_login_username: c.auto_login_username.clone(),
            enable_http_remote_user: c.enable_http_remote_user || c.enable_http_remote_user_api,
            http_remote_user_header_name: c.http_remote_user_header_name.clone(),
            email_host: c.email_host.clone(),
            email_port: u16::try_from(c.email_port).unwrap_or(25),
            email_host_user: c.email_host_user.clone(),
            email_host_password: c.email_host_password.clone(),
            default_from_email: c.default_from_email.clone(),
            email_use_tls: c.email_use_tls,
            email_use_ssl: c.email_use_ssl,
            socialaccount_providers: providers,
            installed_provider_apps: apps,
        }
    }

    /// allauth `ACCOUNT_RATE_LIMITS[action]` (defaults, paperless sets nothing).
    pub fn rate_limits(action: &str) -> &'static str {
        match action {
            "login" => "30/m/ip",
            "login_failed" => "10/m/ip,5/300s/key",
            "signup" => "20/m/ip",
            "reset_password" => "20/m/ip,5/m/key",
            "reset_password_from_key" => "20/m/ip",
            "confirm_email" => "1/180s/key",
            _ => "",
        }
    }

    /// `AUTHENTICATION_BACKENDS` with remote-user backend (when active).
    pub fn has_remote_user_backend(&self) -> bool {
        self.enable_http_remote_user
    }
}

#[cfg(test)]
mod tests {
    use pl_core::config::Env;

    /// upstream fix cb3513c6f: default `["mfa"]`, set values comma-separated.
    #[test]
    fn mfa_trusted_amr_default_and_list() {
        assert_eq!(super::list_default_mfa(&Env::from_pairs::<&str, &str>([])), vec!["mfa".to_string()]);
        assert_eq!(super::list_default_mfa(&Env::from_pairs([("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", "")])), vec!["mfa".to_string()]);
        assert_eq!(
            super::list_default_mfa(&Env::from_pairs([("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", "pwd, hwk ,")])),
            vec!["pwd".to_string(), "hwk".to_string()]
        );
        // v3.4.2 split on "mfa": "otpmfahwk" gave ["otp", "hwk"]; now a single value
        assert_eq!(
            super::list_default_mfa(&Env::from_pairs([("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", "otpmfahwk")])),
            vec!["otpmfahwk".to_string()]
        );
    }
}
