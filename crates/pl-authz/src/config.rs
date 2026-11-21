//! Auth-relevant settings, parsed from the environment exactly like paperless.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (REST_FRAMEWORK, AUTHENTICATION_BACKENDS,
//!           _parse_remote_user_settings, CSRF_TRUSTED_ORIGINS, ALLOWED_HOSTS,
//!           _parse_paperless_url, USE_X_FORWARDED_HOST/PORT, SECURE_PROXY_SSL_HEADER,
//!           SECRET_KEY, COOKIE_PREFIX, AUTO_LOGIN_USERNAME, DEBUG),
//!           src/paperless/settings/parsers.py@v3.4.2 (get_bool_from_env, get_list_from_env).
//!
//! Until pl-core exposes these values (CORE-REQUEST), pl-authz parses them itself.

/// Settings that drive authentication, CSRF and host validation.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// `SECRET_KEY` (`PAPERLESS_SECRET_KEY`).
    pub secret_key: String,
    /// `COOKIE_PREFIX` (`PAPERLESS_COOKIE_PREFIX`, default "").
    pub cookie_prefix: String,
    /// `DEBUG` (`PAPERLESS_DEBUG`).
    pub debug: bool,
    /// `PAPERLESS_ENABLE_HTTP_REMOTE_USER` (frontend middleware + RemoteUserBackend).
    pub remote_user_enabled: bool,
    /// `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API` (adds PaperlessRemoteUserAuthentication first).
    pub remote_user_api: bool,
    /// `HTTP_REMOTE_USER_HEADER_NAME` as META key (default `HTTP_REMOTE_USER`).
    pub remote_user_header: String,
    /// `AUTO_LOGIN_USERNAME`.
    pub auto_login_username: Option<String>,
    /// `CSRF_TRUSTED_ORIGINS` (incl. `PAPERLESS_URL` and DEBUG's `http://localhost:4200`).
    pub csrf_trusted_origins: Vec<String>,
    /// `ALLOWED_HOSTS` (incl. `localhost` and the `PAPERLESS_URL` hostname).
    pub allowed_hosts: Vec<String>,
    /// `USE_X_FORWARDED_HOST`.
    pub use_x_forwarded_host: bool,
    /// `USE_X_FORWARDED_PORT`.
    pub use_x_forwarded_port: bool,
    /// `SECURE_PROXY_SSL_HEADER` as (META key, value).
    pub secure_proxy_ssl_header: Option<(String, String)>,
    /// Scheme of the connection to this server when no proxy SSL header applies
    /// (the Rust server speaks plain HTTP: false).
    pub server_scheme_https: bool,
    /// `TRUSTED_PROXIES` (`PAPERLESS_TRUSTED_PROXIES`): proxy prefixes for the client IP in the
    /// failed-login log (python-ipware `proxy_list`).
    pub trusted_proxies: Vec<String>,
    /// `SERVER_NAME` fallback when no Host header is present.
    pub server_name: String,
}

/// Python `str_to_bool` (paperless parsers): accepted values, case-insensitive.
pub fn str_to_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Some(true),
        "false" | "0" | "f" | "n" | "no" => Some(false),
        _ => None,
    }
}

fn env(key: &str) -> Option<String> {
    pl_core::config::var(key)
}

fn env_bool(key: &str, default: &str) -> Result<bool, String> {
    let v = env(key).unwrap_or_else(|| default.to_string());
    str_to_bool(&v).ok_or_else(|| format!("{key}: invalid boolean value {v:?}"))
}

/// `get_list_from_env(key)`: split on ",", strip, drop empty; unset/empty -> default.
fn env_list(key: &str, default: Option<Vec<String>>) -> Vec<String> {
    match env(key) {
        Some(v) if !v.is_empty() => v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => default.unwrap_or_default(),
    }
}

impl AuthConfig {
    /// Cookie name of the Django session (`SESSION_COOKIE_NAME`).
    pub fn session_cookie_name(&self) -> String {
        format!("{}sessionid", self.cookie_prefix)
    }

    /// Cookie name of the CSRF secret (`CSRF_COOKIE_NAME`).
    pub fn csrf_cookie_name(&self) -> String {
        format!("{}csrftoken", self.cookie_prefix)
    }

    /// `AUTHENTICATION_BACKENDS` (session `_auth_user_backend` must be one of them).
    pub fn authentication_backends(&self) -> Vec<&'static str> {
        let mut v = vec![
            "guardian.backends.ObjectPermissionBackend",
            "django.contrib.auth.backends.ModelBackend",
            "allauth.account.auth_backends.AuthenticationBackend",
        ];
        if self.remote_user_enabled || self.remote_user_api {
            v.insert(0, "django.contrib.auth.backends.RemoteUserBackend");
        }
        v
    }

    /// Parse from the process environment like `paperless/settings/__init__.py`.
    pub fn from_env() -> Result<Self, String> {
        let debug = env_bool("PAPERLESS_DEBUG", "NO")?;
        let secret_key = env("PAPERLESS_SECRET_KEY").unwrap_or_default();
        if secret_key.trim().is_empty() || secret_key == "change-me" {
            return Err("PAPERRUST_SECRET_KEY is not set or is the default 'change-me' value. \
                        A unique, secret key is required for secure operation."
                .into());
        }
        let remote_user_enabled = env_bool("PAPERLESS_ENABLE_HTTP_REMOTE_USER", "NO")?;
        let remote_user_api = env_bool("PAPERLESS_ENABLE_HTTP_REMOTE_USER_API", "NO")?;
        let remote_user_header = env("PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME").unwrap_or_else(|| "HTTP_REMOTE_USER".into());

        let mut csrf_trusted_origins = env_list("PAPERLESS_CSRF_TRUSTED_ORIGINS", None);
        if debug {
            csrf_trusted_origins.push("http://localhost:4200".into());
        }
        // upstream fix 00096e025 (#14735): `["*"]` only without PAPERLESS_ALLOWED_HOSTS and without
        // PAPERLESS_URL; hostname normalised (trailing dot, `*`/`.…` invalid, IPv6 in `[]`).
        let paperless_url = env("PAPERLESS_URL").filter(|u| !u.is_empty());
        let mut allowed_hosts = pl_core::config::initial_allowed_hosts(env("PAPERLESS_ALLOWED_HOSTS").as_deref(), paperless_url.is_some());
        if let Some(url) = paperless_url {
            let host =
                crate::csrf::urlsplit(&url).and_then(|p| pl_core::config::allowed_host_from_url_hostname(&hostname_of_netloc(&p.netloc)));
            let Some(host) = host else {
                return Err(format!("PAPERRUST_URL {url:?} has no valid hostname, expected a URL like https://example.com"));
            };
            csrf_trusted_origins.push(url);
            allowed_hosts.push(host);
        }
        let secure_proxy_ssl_header = match env("PAPERLESS_PROXY_SSL_HEADER") {
            None => None,
            Some(v) => {
                let parsed: Vec<String> = serde_json::from_str(&v).map_err(|e| format!("PAPERRUST_PROXY_SSL_HEADER: {e}"))?;
                match parsed.as_slice() {
                    [] => None,
                    [h, val] => Some((h.clone(), val.clone())),
                    _ => {
                        return Err("The SECURE_PROXY_SSL_HEADER setting must be a tuple \
                                    containing two values."
                            .into());
                    }
                }
            }
        };
        Ok(Self {
            secret_key,
            cookie_prefix: env("PAPERLESS_COOKIE_PREFIX").unwrap_or_default(),
            debug,
            remote_user_enabled,
            remote_user_api,
            remote_user_header,
            auto_login_username: env("PAPERLESS_AUTO_LOGIN_USERNAME").filter(|s| !s.is_empty()),
            csrf_trusted_origins,
            allowed_hosts,
            use_x_forwarded_host: env_bool("PAPERLESS_USE_X_FORWARD_HOST", "false")?,
            use_x_forwarded_port: env_bool("PAPERLESS_USE_X_FORWARD_PORT", "false")?,
            secure_proxy_ssl_header,
            server_scheme_https: false,
            trusted_proxies: env_list("PAPERLESS_TRUSTED_PROXIES", None),
            server_name: "localhost".into(),
        })
    }

    /// Minimal config for unit tests (no env access).
    pub fn for_tests(secret_key: &str) -> Self {
        Self {
            secret_key: secret_key.into(),
            cookie_prefix: String::new(),
            debug: false,
            remote_user_enabled: false,
            remote_user_api: false,
            remote_user_header: "HTTP_REMOTE_USER".into(),
            auto_login_username: None,
            csrf_trusted_origins: Vec::new(),
            allowed_hosts: vec!["*".into()],
            use_x_forwarded_host: false,
            use_x_forwarded_port: false,
            secure_proxy_ssl_header: None,
            server_scheme_https: false,
            trusted_proxies: Vec::new(),
            server_name: "localhost".into(),
        }
    }
}

/// `urlparse(url).hostname`: netloc without userinfo/port, lowercased, brackets stripped.
fn hostname_of_netloc(netloc: &str) -> String {
    let hostport = netloc.rsplit_once('@').map(|(_, h)| h).unwrap_or(netloc);
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    host.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// upstream fix 00096e025 (#14735): netloc → ALLOWED_HOSTS entry.
    #[test]
    fn url_host_normalised_like_upstream_00096e025() {
        let host = |u: &str| {
            crate::csrf::urlsplit(u).and_then(|p| pl_core::config::allowed_host_from_url_hostname(&hostname_of_netloc(&p.netloc)))
        };
        assert_eq!(host("https://Example.com.:8443").as_deref(), Some("example.com"));
        assert_eq!(host("http://[::1]:8000").as_deref(), Some("[::1]"));
        assert_eq!(host("https://*"), None);
        assert_eq!(host("https://.example.com"), None);
        assert_eq!(host("https://"), None);
        assert_eq!(pl_core::config::initial_allowed_hosts(None, true), vec!["localhost"]);
        assert_eq!(pl_core::config::initial_allowed_hosts(None, false), vec!["*"]);
    }

    #[test]
    fn bools() {
        assert_eq!(str_to_bool("Yes"), Some(true));
        assert_eq!(str_to_bool("NO"), Some(false));
        assert_eq!(str_to_bool("maybe"), None);
    }

    #[test]
    fn hostname() {
        assert_eq!(hostname_of_netloc("User@Example.com:8000"), "example.com");
        assert_eq!(hostname_of_netloc("[::1]:80"), "::1");
    }

    #[test]
    fn backends() {
        let mut c = AuthConfig::for_tests("x");
        assert_eq!(c.authentication_backends().len(), 3);
        c.remote_user_api = true;
        assert_eq!(c.authentication_backends()[0], "django.contrib.auth.backends.RemoteUserBackend");
        assert_eq!(c.session_cookie_name(), "sessionid");
    }
}
