//! Server configuration from the environment.
//!
//! `PAPERRUST_*` variables configure PaperRust itself (old names `PAPERLESS_RS_*` still work as
//! aliases); the `PAPERLESS_*` variables are read exactly like upstream (compatibility with
//! paperless-ngx; `PAPERRUST_<NAME>` takes precedence) so the Rust middleware layer behaves like
//! Django's.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (DEBUG, ALLOWED_HOSTS,
//! CORS_ALLOWED_ORIGINS, CORS_ALLOW_CREDENTIALS, CORS_EXPOSE_HEADERS, _parse_paperless_url,
//! USE_X_FORWARDED_HOST, COOKIE_PREFIX/LANGUAGE_COOKIE_NAME, PAPERLESS_ENABLE_COMPRESSION)
//! upstream: src/paperless/settings/parsers.py@v3.4.2 (str_to_bool, get_list_from_env)

use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{var}: invalid value {value:?}: {reason}")]
    Invalid {
        var: &'static str,
        value: String,
        reason: String,
    },
}

fn invalid(var: &'static str, value: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        var,
        value: value.to_owned(),
        reason: reason.into(),
    }
}

/// The subset of Django settings the Rust middleware layer needs.
#[derive(Debug, Clone)]
pub struct DjangoSettings {
    pub debug: bool,
    pub allowed_hosts: Vec<String>,
    pub use_x_forwarded_host: bool,
    pub cors_allowed_origins: Vec<String>,
    pub cors_allow_credentials: bool,
    pub cors_expose_headers: Vec<String>,
    pub language_cookie_name: String,
    pub enable_compression: bool,
}

impl DjangoSettings {
    /// Take the values from pl-core's `Config` (single source in production; the
    /// local parser in [`ServerConfig::from_lookup`] is kept for tests and must agree,
    /// see `tests::agrees_with_pl_core`).
    pub fn from_core(c: &pl_core::Config) -> Self {
        Self {
            debug: c.debug,
            allowed_hosts: c.allowed_hosts.clone(),
            use_x_forwarded_host: c.use_x_forwarded_host,
            cors_allowed_origins: c.cors_allowed_origins.clone(),
            cors_allow_credentials: true,
            cors_expose_headers: vec!["Content-Disposition".into()],
            language_cookie_name: c.language_cookie_name.clone(),
            enable_compression: c.enable_compression,
        }
    }
}

impl Default for DjangoSettings {
    /// Upstream defaults with no `PAPERLESS_*` variable set.
    fn default() -> Self {
        Self {
            debug: false,
            allowed_hosts: vec!["*".into()],
            use_x_forwarded_host: false,
            cors_allowed_origins: vec!["http://localhost:8000".into()],
            cors_allow_credentials: true,
            cors_expose_headers: vec!["Content-Disposition".into()],
            language_cookie_name: "django_language".into(),
            enable_compression: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    /// `PAPERRUST_STOP_ON_STDIN_EOF`: stop (like Ctrl+C) when stdin reaches EOF (supervisor gone).
    pub stop_on_stdin_eof: bool,
    pub routes_file: PathBuf,
    /// `true` when `PAPERRUST_ROUTES` was given explicitly: then a missing file is an error.
    pub routes_file_required: bool,
    pub settings: DjangoSettings,
    /// `PAPERRUST_BRANDING` / `PAPERRUST_ACCENT` (see `branding`).
    pub branding: crate::branding::Branding,
    /// `PAPERRUST_API_V2` (default on; `off`/`false`/`0`/`no` = native API v2 under
    /// `/rs/api/v2/` not mounted). docs/ARCHITECTURE-V2.md §3.
    pub api_v2: bool,
}

pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        // Process environment plus values saved in the web UI (docs/SETTINGS.md §2.1; without them
        // identical to `std::env::var`).
        Self::from_lookup(pl_core::config::var)
    }

    /// Same as [`from_env`](Self::from_env) with an injectable lookup (tests).
    /// Names follow pl-core's rules ([`pl_core::config::candidates`]): `PAPERRUST_X` first, the
    /// old name `PAPERLESS_RS_X` as an alias; upstream settings `PAPERLESS_X` also as `PAPERRUST_X`.
    pub fn from_lookup(raw: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |k: &str| pl_core::config::lookup(&raw, k);
        let bind_raw = get("PAPERRUST_BIND").unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind: SocketAddr = bind_raw.trim().parse().map_err(|_| {
            invalid(
                "PAPERRUST_BIND",
                &bind_raw,
                "expected IP:PORT, e.g. 127.0.0.1:8080 or [::1]:8080",
            )
        })?;

        // PAPERRUST_DJANGO_URL / PAPERRUST_PROXY_* are ignored (no Django backend any more); a set
        // value only produces a warning at startup (pl_core::config::retired_warnings).

        let (routes_file, routes_file_required) = match get("PAPERRUST_ROUTES") {
            Some(p) if !p.trim().is_empty() => (PathBuf::from(p.trim()), true),
            _ => (PathBuf::from("routes.toml"), false),
        };

        Ok(Self {
            bind,
            stop_on_stdin_eof: bool_env(&get, "PAPERRUST_STOP_ON_STDIN_EOF", "no")?,
            routes_file,
            routes_file_required,
            settings: django_settings(&get)?,
            branding: crate::branding::Branding::from_lookup(&get)?,
            api_v2: !matches!(
                get("PAPERRUST_API_V2").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
                Some("off" | "false" | "0" | "no")
            ),
        })
    }
}

/// upstream: parsers.py::str_to_bool
pub fn str_to_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Some(true),
        "false" | "0" | "f" | "n" | "no" => Some(false),
        _ => None,
    }
}

fn bool_env(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: &str,
) -> Result<bool, ConfigError> {
    let v = get(var).unwrap_or_else(|| default.to_owned());
    str_to_bool(&v).ok_or_else(|| invalid(var, &v, "expected yes/no, true/false, 1/0"))
}

/// upstream: parsers.py::get_list_from_env (separator ",", strip, drop empty;
/// default when unset *or empty*).
fn list_env(get: &impl Fn(&str) -> Option<String>, var: &str, default: &[&str]) -> Vec<String> {
    match get(var) {
        Some(v) if !v.is_empty() => v
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => default.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Python `urlparse(url).hostname`: lowercased host without port/brackets/userinfo.
fn url_hostname(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or("");
    let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = netloc.rsplit_once('@').map(|(_, h)| h).unwrap_or(netloc);
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

fn django_settings(get: &impl Fn(&str) -> Option<String>) -> Result<DjangoSettings, ConfigError> {
    let debug = bool_env(get, "PAPERLESS_DEBUG", "NO")?;

    let mut cors_allowed_origins = list_env(
        get,
        "PAPERLESS_CORS_ALLOWED_HOSTS",
        &["http://localhost:8000"],
    );
    if debug {
        cors_allowed_origins.push("http://localhost:4200".into());
    }

    // upstream fix 00096e025 (#14735): `["*"]` only without PAPERLESS_ALLOWED_HOSTS and without
    // PAPERLESS_URL; URL hostname normalised (trailing dot, `*`/`.…` invalid, IPv6 in `[]`).
    let paperless_url = get("PAPERLESS_URL").filter(|u| !u.is_empty());
    let mut allowed_hosts = pl_core::config::initial_allowed_hosts(
        get("PAPERLESS_ALLOWED_HOSTS").as_deref(),
        paperless_url.is_some(),
    );

    if let Some(url) = paperless_url {
        let host = url_hostname(&url)
            .and_then(|h| pl_core::config::allowed_host_from_url_hostname(&h))
            .ok_or_else(|| {
                invalid(
                    "PAPERLESS_URL",
                    &url,
                    "has no valid hostname, expected a URL like https://example.com",
                )
            })?;
        cors_allowed_origins.push(url.clone());
        allowed_hosts.push(host);
    }

    let prefix = get("PAPERLESS_COOKIE_PREFIX").unwrap_or_default();

    Ok(DjangoSettings {
        debug,
        allowed_hosts,
        use_x_forwarded_host: bool_env(get, "PAPERLESS_USE_X_FORWARD_HOST", "false")?,
        cors_allowed_origins,
        cors_allow_credentials: true,
        cors_expose_headers: vec!["Content-Disposition".into()],
        language_cookie_name: format!("{prefix}django_language"),
        enable_compression: bool_env(get, "PAPERLESS_ENABLE_COMPRESSION", "yes")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<ServerConfig, ConfigError> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        ServerConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_bind_loopback() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.bind.to_string(), "127.0.0.1:8080");
        assert!(!c.stop_on_stdin_eof);
        assert!(!c.routes_file_required);
        assert_eq!(c.settings.allowed_hosts, vec!["*"]);
        assert_eq!(
            c.settings.cors_allowed_origins,
            vec!["http://localhost:8000"]
        );
        assert!(c.settings.enable_compression);
    }

    #[test]
    fn invalid_bind_is_an_error_not_0000() {
        for bad in ["", "8080", "localhost:8080", "0.0.0.0", "garbage"] {
            assert!(cfg(&[("PAPERRUST_BIND", bad)]).is_err(), "{bad}");
        }
        let c = cfg(&[("PAPERRUST_BIND", "[::1]:9000")]).unwrap();
        assert_eq!(c.bind.to_string(), "[::1]:9000");
    }

    #[test]
    fn retired_django_url_is_ignored() {
        // Old installations may still set it: never a startup error (warning in pl-core).
        for v in ["https://x:1", "http://x:1/sub/", "127.0.0.1:8000", "http://127.0.0.1:8000/", "off", ""] {
            assert!(cfg(&[("PAPERRUST_DJANGO_URL", v)]).is_ok(), "{v:?}");
        }
        assert!(cfg(&[("PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS", "x")]).is_ok());
        assert!(cfg(&[("PAPERRUST_STOP_ON_STDIN_EOF", "1")]).unwrap().stop_on_stdin_eof);
        assert!(cfg(&[("PAPERRUST_STOP_ON_STDIN_EOF", "maybe")]).is_err());
    }

    #[test]
    fn legacy_names_still_work() {
        // Old scripts (PAPERLESS_RS_*) keep working; PAPERRUST_* wins when both are set.
        let c = cfg(&[("PAPERLESS_RS_BIND", "127.0.0.1:9001"), ("PAPERLESS_RS_DJANGO_URL", "off")]).unwrap();
        assert_eq!(c.bind.to_string(), "127.0.0.1:9001");
        let c = cfg(&[("PAPERLESS_RS_BIND", "127.0.0.1:9001"), ("PAPERRUST_BIND", "127.0.0.1:9002")]).unwrap();
        assert_eq!(c.bind.to_string(), "127.0.0.1:9002");
        assert!(cfg(&[("PAPERLESS_RS_ROUTES", "x.toml")]).unwrap().routes_file_required);
        // Upstream setting under the primary name.
        let c = cfg(&[("PAPERRUST_ALLOWED_HOSTS", "a.example"), ("PAPERLESS_ALLOWED_HOSTS", "b.example")]).unwrap();
        assert_eq!(c.settings.allowed_hosts, vec!["a.example", "localhost"]);
    }

    #[test]
    fn hosts_like_upstream() {
        let c = cfg(&[
            ("PAPERLESS_ALLOWED_HOSTS", "a.example, b.example,"),
            ("PAPERLESS_URL", "https://Paper.Example:8443"),
            ("PAPERLESS_DEBUG", "true"),
            ("PAPERLESS_COOKIE_PREFIX", "pl_"),
        ])
        .unwrap();
        assert_eq!(
            c.settings.allowed_hosts,
            vec!["a.example", "b.example", "localhost", "paper.example"]
        );
        assert_eq!(
            c.settings.cors_allowed_origins,
            vec![
                "http://localhost:8000",
                "http://localhost:4200",
                "https://Paper.Example:8443"
            ]
        );
        assert_eq!(c.settings.language_cookie_name, "pl_django_language");
        assert!(cfg(&[("PAPERLESS_URL", "nohost")]).is_err());
        assert!(cfg(&[("PAPERLESS_ENABLE_COMPRESSION", "maybe")]).is_err());
        // upstream fix 00096e025 (#14735): only PAPERLESS_URL set → no wildcard any more
        let c = cfg(&[("PAPERLESS_URL", "http://[::1]:8000")]).unwrap();
        assert_eq!(c.settings.allowed_hosts, vec!["localhost", "[::1]"]);
        let c = cfg(&[("PAPERLESS_URL", "https://example.com."), ("PAPERLESS_ALLOWED_HOSTS", "*")]).unwrap();
        assert_eq!(c.settings.allowed_hosts, vec!["*", "example.com"]);
        assert_eq!(cfg(&[("PAPERLESS_ALLOWED_HOSTS", "")]).unwrap().settings.allowed_hosts, vec!["localhost"]);
        for bad in ["https://*", "https://.example.com"] {
            assert!(cfg(&[("PAPERLESS_URL", bad)]).is_err(), "{bad}");
        }
    }

    #[test]
    fn agrees_with_pl_core() {
        let vars = [
            ("PAPERLESS_SECRET_KEY", "test-only-secret-key-0123456789"),
            ("PAPERLESS_ALLOWED_HOSTS", "a.example, b.example,"),
            ("PAPERLESS_URL", "https://Paper.Example:8443"),
            ("PAPERLESS_CORS_ALLOWED_HOSTS", "http://x.example"),
            ("PAPERLESS_DEBUG", "true"),
            ("PAPERLESS_USE_X_FORWARD_HOST", "yes"),
            ("PAPERLESS_COOKIE_PREFIX", "pl_"),
            ("PAPERLESS_ENABLE_COMPRESSION", "no"),
        ];
        let ours = cfg(&vars).unwrap().settings;
        let core = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs(vars)).unwrap();
        let theirs = DjangoSettings::from_core(&core);
        assert_eq!(format!("{ours:?}"), format!("{theirs:?}"));
    }
}
