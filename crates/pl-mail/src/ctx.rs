//! Configuration and signed-in user for pl-mail.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (TIME_ZONE, EMAIL_*, *_OAUTH_*).
//! Values from `pl_core::Config` (no separate environment evaluation any more).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono_tz::Tz;
use pl_core::AppState;

use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct MailConfig {
    pub time_zone: Tz,
    pub allow_internal_hosts: bool,
    pub certificate_file: Option<PathBuf>,
    pub gmail_client_id: Option<String>,
    pub gmail_client_secret: Option<String>,
    pub outlook_client_id: Option<String>,
    pub outlook_client_secret: Option<String>,
    /// Token endpoints (httpx-oauth). Overridable for tests only.
    pub gmail_token_url: String,
    pub outlook_token_url: String,
    /// No upstream counterpart (imaplib waits indefinitely) – see Known differences.
    pub imap_timeout: Duration,
    /// `SECURE_PROXY_SSL_HEADER` (header name in lower case, value).
    pub proxy_ssl_header: Option<(String, String)>,
}

impl MailConfig {
    /// From `pl_core::Config` (same `PAPERLESS_*` evaluation as Django, once centrally).
    pub fn from_core(core: &pl_core::Config) -> Self {
        // Django: META key like HTTP_X_FORWARDED_PROTO → header name
        let proxy_ssl_header = core.secure_proxy_ssl_header.as_ref().map(|(name, value)| {
            let header = name.strip_prefix("HTTP_").unwrap_or(name).replace('_', "-").to_lowercase();
            (header, value.clone())
        });
        MailConfig {
            time_zone: core.tz,
            allow_internal_hosts: core.email_allow_internal_hosts,
            certificate_file: core.email_certificate_file.clone(),
            gmail_client_id: core.gmail_oauth_client_id.clone(),
            gmail_client_secret: core.gmail_oauth_client_secret.clone(),
            outlook_client_id: core.outlook_oauth_client_id.clone(),
            outlook_client_secret: core.outlook_oauth_client_secret.clone(),
            gmail_token_url: "https://oauth2.googleapis.com/token".to_string(),
            outlook_token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
            imap_timeout: Duration::from_secs(60),
            proxy_ssl_header,
        }
    }
}

static CONFIG: OnceLock<MailConfig> = OnceLock::new();

/// Process-wide configuration, built from `pl_core::Config` on first use. Tests can call
/// `set_config` beforehand.
pub fn config(core: &pl_core::Config) -> &'static MailConfig {
    CONFIG.get_or_init(|| MailConfig::from_core(core))
}

/// Sets the configuration (possible only once; for tests and server start).
pub fn set_config(cfg: MailConfig) -> bool {
    CONFIG.set(cfg).is_ok()
}

/// Signed-in user as the permission check needs it.
///
/// Normal case: from `pl_authz::AuthUser` (DRF authentication + `IsAuthenticated`; 401/403
/// come from pl-authz). Tests can put `MailUser` or `AuthUserId` directly into the request
/// extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailUser {
    pub id: i32,
    pub is_active: bool,
    pub is_superuser: bool,
    /// ModelBackend permissions `"app_label.codename"`; `None` → load from the DB.
    pub perms: Option<Arc<HashSet<String>>>,
}

/// Minimal variant: only the user ID; flags/permissions are loaded from the DB.
#[derive(Debug, Clone, Copy)]
pub struct AuthUserId(pub i32);

impl MailUser {
    pub fn is_active_superuser(&self) -> bool {
        self.is_active && self.is_superuser
    }

    pub fn from_authz(u: &pl_authz::User) -> Option<Self> {
        Some(MailUser {
            id: i32::try_from(u.id).ok()?,
            is_active: u.is_active,
            is_superuser: u.is_superuser,
            perms: Some(Arc::new(u.permissions.clone())),
        })
    }
}

impl FromRequestParts<AppState> for MailUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(u) = parts.extensions.get::<MailUser>() {
            return Ok(u.clone());
        }
        if let Some(AuthUserId(id)) = parts.extensions.get::<AuthUserId>().copied() {
            let row: Option<(bool, bool)> =
                sqlx::query_as("SELECT is_active, is_superuser FROM auth_user WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&state.inner.db)
                    .await?;
            return match row {
                Some((is_active, is_superuser)) => Ok(MailUser { id, is_active, is_superuser, perms: None }),
                None => Err(ApiError::NotAuthenticated),
            };
        }
        let au = pl_authz::AuthUser::from_request_parts(parts, state).await?;
        MailUser::from_authz(au.user()).ok_or_else(|| ApiError::internal("user id out of range"))
    }
}
