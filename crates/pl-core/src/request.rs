//! Request context for handlers: absolute URI like Django `build_absolute_uri`, query like
//! `QueryDict`, negotiation/version like DRF, active language like `LocaleMiddleware`.
//!
//! upstream: django/http/request.py@5.2 (`get_host`, `_get_raw_host`, `scheme`,
//! `build_absolute_uri`, `get_full_path`), django/utils/translation/trans_real.py
//! (`get_language_from_request`, `get_supported_language_variant`, `parse_accept_lang_header`),
//! src/paperless/settings/__init__.py (`USE_X_FORWARDED_HOST`, `USE_X_FORWARDED_PORT`,
//! `SECURE_PROXY_SSL_HEADER`, `FORCE_SCRIPT_NAME`, `LANGUAGES`, `LANGUAGE_COOKIE_NAME`).
//!
//! **Proxy headers:** upstream evaluates `X-Forwarded-Host`/`-Port` only with
//! `PAPERLESS_USE_X_FORWARD_HOST`/`_PORT=true` and `X-Forwarded-Proto` only if
//! `PAPERLESS_PROXY_SSL_HEADER` is set – exactly the same here.

use axum::extract::{FromRequestParts, OriginalUri};
use axum::http::header::HOST;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};

use crate::version::{Negotiation, negotiate};
use crate::{ApiError, ApiVersion, AppState, Config, QueryParams, urlenc};

/// Scheme of the incoming request as the server sees it (without proxy headers)
/// (`wsgi.url_scheme` / ASGI `scope["scheme"]`). `pl-server` sets this extension when it
/// terminates TLS itself; if it is missing, `http` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerScheme(pub &'static str);

/// Local server address for the rare case of a request without `Host` (`SERVER_NAME`,
/// `SERVER_PORT`). Optional extension from `pl-server`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    pub name: String,
    pub port: u16,
}

/// Context of an API request. Use as the **first** extractor (DRF negotiates before
/// authentication).
///
/// ```no_run
/// use pl_core::{ApiResult, Drf, RequestContext};
/// async fn list(ctx: RequestContext) -> ApiResult<Drf<serde_json::Value>> {
///     let _next = ctx.build_absolute_uri();
///     Ok(Drf::ok(serde_json::json!({"version": ctx.version().as_u16()})))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub method: Method,
    /// `request.path` (decoded, incl. `FORCE_SCRIPT_NAME`).
    pub path: String,
    /// Raw query (`QUERY_STRING`), without `?`.
    pub raw_query: String,
    /// `request.GET` / `request.query_params`.
    pub query: QueryParams,
    /// `request.scheme`.
    pub scheme: String,
    /// `request.get_host()` (validated against `ALLOWED_HOSTS`).
    pub host: String,
    /// DRF-Negotiation (Renderer, Version, indent).
    pub negotiation: Negotiation,
    /// Active language (`LocaleMiddleware`), e.g. `en-us`, `de-de`.
    pub language: &'static str,
    /// Request headers (for `pl-authz`, CSRF, etc.).
    pub headers: HeaderMap,
}

impl RequestContext {
    /// Builds the context from request parts and configuration.
    pub fn from_parts(parts: &Parts, config: &Config) -> Result<Self, ApiError> {
        let uri = parts.extensions.get::<OriginalUri>().map(|o| o.0.clone()).unwrap_or_else(|| parts.uri.clone());
        let raw_query = uri.query().unwrap_or("").to_string();
        let query = QueryParams::parse(&raw_query);
        let mut path = urlenc::decode_request_path(uri.path());
        if let Some(sn) = config.force_script_name.as_deref().filter(|s| !s.is_empty())
            && !path.starts_with(sn)
        {
            path = format!("{}/{}", sn.trim_end_matches('/'), path.strip_prefix('/').unwrap_or(&path));
        }
        let scheme = request_scheme(&parts.headers, config, parts.extensions.get::<ServerScheme>().map(|s| s.0));
        let host =
            request_host(&parts.headers, config, uri.authority().map(|a| a.as_str()), parts.extensions.get::<ServerAddr>(), &scheme)?;
        let negotiation = negotiate(&parts.headers, &query)?;
        let language = language_from_request(&parts.headers, &config.language_cookie_name);
        Ok(Self {
            method: parts.method.clone(),
            path,
            raw_query,
            query,
            scheme,
            host,
            negotiation,
            language,
            headers: parts.headers.clone(),
        })
    }

    /// `request.version`.
    pub fn version(&self) -> ApiVersion {
        self.negotiation.version
    }

    /// `request.get_full_path()`: `escape_uri_path(path)` + `?` + `iri_to_uri(QUERY_STRING)`.
    pub fn full_path(&self) -> String {
        let mut s = urlenc::escape_uri_path(&self.path);
        if !self.raw_query.is_empty() {
            s.push('?');
            s.push_str(&urlenc::iri_to_uri(&self.raw_query));
        }
        s
    }

    /// `scheme://host`.
    pub fn scheme_host(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }

    /// `request.build_absolute_uri()` (current URL incl. query).
    pub fn build_absolute_uri(&self) -> String {
        format!("{}{}", self.scheme_host(), self.full_path())
    }

    /// `request.build_absolute_uri(location)` for absolute paths (`/api/...`) or full URLs.
    /// Relative paths are appended to the current path (simplified `urljoin`).
    pub fn build_absolute_uri_for(&self, location: &str) -> String {
        let bits = urlenc::urlsplit(location);
        if !bits.scheme.is_empty() && !bits.netloc.is_empty() {
            return urlenc::iri_to_uri(location);
        }
        if let Some(rest) = location.strip_prefix("//") {
            if bits.netloc.is_empty() {
                return urlenc::iri_to_uri(&format!("{}/{}", self.scheme_host(), rest));
            }
            return urlenc::iri_to_uri(&format!("{}:{}", self.scheme, location));
        }
        if location.starts_with('/') {
            return urlenc::iri_to_uri(&format!("{}{}", self.scheme_host(), location));
        }
        let base = match self.path.rfind('/') {
            Some(i) => &self.path[..=i],
            None => "/",
        };
        urlenc::iri_to_uri(&format!("{}{}{}", self.scheme_host(), base, location))
    }
}

impl FromRequestParts<AppState> for RequestContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(ctx) = parts.extensions.get::<RequestContext>() {
            return Ok(ctx.clone());
        }
        let ctx = RequestContext::from_parts(parts, state.config())?;
        parts.extensions.insert(ctx.clone());
        Ok(ctx)
    }
}

/// Returns the first value of a header as `&str`.
fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// Django `HttpRequest.scheme` (with `SECURE_PROXY_SSL_HEADER`).
pub fn request_scheme(headers: &HeaderMap, config: &Config, server_scheme: Option<&'static str>) -> String {
    if let Some((meta, secure_value)) = &config.secure_proxy_ssl_header
        && let Some(hname) = Config::meta_key_to_header(meta)
        && let Some(v) = header(headers, &hname)
    {
        let first = v.split(',').next().unwrap_or("").trim();
        return if first == secure_value { "https".into() } else { "http".into() };
    }
    server_scheme.unwrap_or("http").to_string()
}

/// Django `HttpRequest.get_host()`: `X-Forwarded-Host` (only with `USE_X_FORWARDED_HOST`),
/// otherwise `Host`, otherwise server name/port; check against `ALLOWED_HOSTS`.
/// Invalid host → Django's 400 page (`DisallowedHost`).
pub fn request_host(
    headers: &HeaderMap,
    config: &Config,
    uri_authority: Option<&str>,
    server: Option<&ServerAddr>,
    scheme: &str,
) -> Result<String, ApiError> {
    let raw = if let (true, Some(x)) = (config.use_x_forwarded_host, header(headers, "x-forwarded-host")) {
        x.to_string()
    } else if let Some(h) = headers.get(HOST).and_then(|v| v.to_str().ok()) {
        h.to_string()
    } else if let Some(a) = uri_authority {
        // HTTP/2 `:authority` – Django servers map it to HTTP_HOST.
        a.to_string()
    } else {
        let (name, port) = match server {
            Some(s) => (s.name.clone(), s.port.to_string()),
            None => ("localhost".to_string(), "80".to_string()),
        };
        let port = match (config.use_x_forwarded_port, header(headers, "x-forwarded-port")) {
            (true, Some(p)) => p.to_string(),
            _ => port,
        };
        let default_port = if scheme == "https" { "443" } else { "80" };
        if port != default_port { format!("{name}:{port}") } else { name }
    };
    if config.host_allowed(&raw) {
        Ok(raw)
    } else {
        tracing::warn!(host = %raw, "Invalid HTTP_HOST header");
        Err(ApiError::django_bad_request())
    }
}

/// Django `get_supported_language_variant(lang_code, strict=False)` against paperless `LANGUAGES`
/// (implementation: [`crate::locale`]).
pub fn supported_language_variant(lang_code: &str) -> Option<&'static str> {
    crate::locale::get_supported_language_variant(lang_code).map(|c| crate::locale::static_language(&c.to_lowercase()))
}

/// Django `get_language_from_request(request, check_path=False)`: language cookie, then
/// `Accept-Language`, otherwise `LANGUAGE_CODE` (`en-us`). Implementation: [`crate::locale`].
pub fn language_from_request(headers: &HeaderMap, language_cookie_name: &str) -> &'static str {
    crate::locale::language_from_headers(headers, language_cookie_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;
    use axum::http::{HeaderValue, Request};

    fn config(extra: &[(&str, &str)]) -> Config {
        let mut v = vec![("PAPERLESS_SECRET_KEY", "x")];
        v.extend_from_slice(extra);
        Config::from_env_map(Env::from_pairs(v)).unwrap()
    }

    fn ctx(uri: &str, headers: &[(&str, &str)], cfg: &Config) -> Result<RequestContext, ApiError> {
        let mut b = Request::builder().uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let (parts, _) = b.body(()).unwrap().into_parts();
        RequestContext::from_parts(&parts, cfg)
    }

    #[test]
    fn absolute_uri() {
        let cfg = config(&[]);
        let c = ctx("/api/documents/?page=2&title__icontains=%C3%A4+b", &[("host", "paper:8000")], &cfg).unwrap();
        assert_eq!(c.build_absolute_uri(), "http://paper:8000/api/documents/?page=2&title__icontains=%C3%A4+b");
        assert_eq!(c.build_absolute_uri_for("/api/tags/1/"), "http://paper:8000/api/tags/1/");
        assert_eq!(c.query.get("title__icontains"), Some("ä b"));
    }

    #[test]
    fn proxy_headers_only_when_configured() {
        let cfg = config(&[]);
        let h = [("host", "internal"), ("x-forwarded-host", "public.example"), ("x-forwarded-proto", "https")];
        let c = ctx("/api/", &h, &cfg).unwrap();
        assert_eq!(c.scheme_host(), "http://internal");
        let cfg =
            config(&[("PAPERLESS_USE_X_FORWARD_HOST", "true"), ("PAPERLESS_PROXY_SSL_HEADER", r#"["HTTP_X_FORWARDED_PROTO", "https"]"#)]);
        let c = ctx("/api/", &h, &cfg).unwrap();
        assert_eq!(c.scheme_host(), "https://public.example");
    }

    #[test]
    fn disallowed_host() {
        let cfg = config(&[("PAPERLESS_ALLOWED_HOSTS", "good.example")]);
        assert!(ctx("/api/", &[("host", "good.example")], &cfg).is_ok());
        assert!(ctx("/api/", &[("host", "localhost:8000")], &cfg).is_ok());
        let e = ctx("/api/", &[("host", "evil.example")], &cfg).unwrap_err();
        assert_eq!(e.status().as_u16(), 400);
    }

    #[test]
    fn script_name() {
        let cfg = config(&[("PAPERLESS_FORCE_SCRIPT_NAME", "/paperless")]);
        let c = ctx("/api/tags/", &[("host", "h")], &cfg).unwrap();
        assert_eq!(c.build_absolute_uri(), "http://h/paperless/api/tags/");
        let c = ctx("/paperless/api/tags/", &[("host", "h")], &cfg).unwrap();
        assert_eq!(c.build_absolute_uri(), "http://h/paperless/api/tags/");
    }

    #[test]
    fn languages() {
        let mut h = HeaderMap::new();
        assert_eq!(language_from_request(&h, "django_language"), "en-us");
        h.insert("accept-language", HeaderValue::from_static("de-CH,de;q=0.9,en;q=0.8"));
        assert_eq!(language_from_request(&h, "django_language"), "de-de");
        h.insert(axum::http::header::COOKIE, HeaderValue::from_static("django_language=fr-fr; other=1"));
        assert_eq!(language_from_request(&h, "django_language"), "fr-fr");
        assert_eq!(supported_language_variant("pt"), Some("pt-br"));
        assert_eq!(supported_language_variant("en"), Some("en-us"));
        assert_eq!(supported_language_variant("xx"), None);
    }
}
