//! Glue to pl-core: settings from [`pl_core::Config`], errors into [`pl_core::ApiError`].

use crate::config::AuthConfig;
use crate::error::AuthError;
use crate::owned::FieldError;
use pl_core::ApiError;

impl AuthConfig {
    /// Take the auth settings from pl-core's parsed configuration (same env parsing).
    pub fn from_core(c: &pl_core::Config) -> Self {
        Self {
            secret_key: c.secret_key.clone(),
            cookie_prefix: c.cookie_prefix.clone(),
            debug: c.debug,
            remote_user_enabled: c.enable_http_remote_user,
            remote_user_api: c.enable_http_remote_user_api,
            remote_user_header: c.http_remote_user_header_name.clone(),
            auto_login_username: c.auto_login_username.clone().filter(|s| !s.is_empty()),
            csrf_trusted_origins: c.csrf_trusted_origins.clone(),
            allowed_hosts: c.allowed_hosts.clone(),
            use_x_forwarded_host: c.use_x_forwarded_host,
            use_x_forwarded_port: c.use_x_forwarded_port,
            secure_proxy_ssl_header: c.secure_proxy_ssl_header.clone(),
            server_scheme_https: false,
            trusted_proxies: c.trusted_proxies.clone(),
            server_name: "localhost".into(),
        }
    }
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::NotAuthenticated { www_authenticate } => ApiError::not_authenticated(www_authenticate.as_deref()),
            AuthError::AuthenticationFailed { detail, www_authenticate } => {
                ApiError::authentication_failed(Some(&detail), www_authenticate.as_deref())
            }
            AuthError::PermissionDenied { detail } => ApiError::permission_denied_msg(detail),
            AuthError::NotFound { detail } => ApiError::with_detail(axum::http::StatusCode::NOT_FOUND, detail),
            AuthError::MethodNotAllowed { method } => ApiError::method_not_allowed(&method),
            AuthError::Internal { message } => ApiError::internal(message),
            AuthError::Unavailable { message } => {
                tracing::warn!(target: "paperless.auth", "database not reachable: {message}");
                ApiError::db_unavailable()
            }
        }
    }
}

impl FieldError {
    /// `{"<field>": <detail>}` as a DRF 400 validation error.
    pub fn into_api_error(self, field: &str) -> ApiError {
        let mut m = serde_json::Map::new();
        m.insert(field.to_string(), self.0);
        ApiError::validation(serde_json::Value::Object(m))
    }
}
