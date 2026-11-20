//! Request context of the account pages: form, session, CSRF, messages, response completion.
//!
//! upstream: django/middleware/csrf.py@5.2 (`get_token`, `rotate_token`, `_get_secret`,
//! `process_view`: Origin/Referer, field `csrfmiddlewaretoken` before header `X-CSRFToken`,
//! `process_response`: cookie + `Vary: Cookie`), django/views/csrf.py (`csrf_failure`, page
//! without DEBUG), django/views/decorators/cache.py (`never_cache`), django/http/request.py
//! (`QueryDict`), src/paperless/middleware.py (`ApiVersionMiddleware` → `pl_core::Authenticated`).
//! Order of the `Set-Cookie` headers like Django's middleware return path: messages, csrftoken, sessionid.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, EXPIRES, LOCATION, SET_COOKIE, VARY};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use chrono::Utc;
use pl_authz::User;
use pl_core::AppState;

use crate::cache::Cache;
use crate::messages::Messages;
use crate::session::{Session, http_date};
use crate::settings::{CSRF_COOKIE_AGE, Settings};

/// Internal error (→ Django's 500 page).
#[derive(Debug)]
pub struct Internal(pub String);

macro_rules! internal_from {
    ($($t:ty),*) => {$(
        impl From<$t> for Internal {
            fn from(e: $t) -> Self {
                Internal(format!("{e:?}"))
            }
        }
    )*};
}
internal_from!(sqlx::Error, tokio::task::JoinError, crate::session::SessionError, crate::cache::CacheError);

pub type R<T> = Result<T, Internal>;

/// Django's `server_error` page (DEBUG=False, no 500.html).
pub fn server_error(e: &Internal) -> Response {
    tracing::error!(target: "pl_accounts", "internal error: {}", e.0);
    let mut r = Response::new(Body::from("\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Server Error (500)</title>\n</head>\n<body>\n  <h1>Server Error (500)</h1><p></p>\n</body>\n</html>\n"));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    r
}

/// Python `parse_qsl(keep_blank_values=True)` like `QueryDict`.
pub fn parse_qs(s: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(s.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn last<'a>(v: &'a [(String, String)], k: &str) -> Option<&'a str> {
    v.iter().rev().find(|(a, _)| a == k).map(|(_, b)| b.as_str())
}

/// Everything belonging to one request.
pub struct Ctx {
    pub st: AppState,
    pub s: Arc<Settings>,
    pub cache: Cache,
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub form: Vec<(String, String)>,
    pub lang: &'static str,
    pub remote_addr: Option<String>,
    pub session: Session,
    pub messages: Messages,
    csrf_secret: Option<String>,
    csrf_cookie_invalid: Option<String>,
    csrf_needs_update: bool,
    /// `request.user`.
    pub user: Option<User>,
    pub never_cache: bool,
}

/// Process-wide settings (from `AppState`).
pub fn settings(st: &AppState) -> Arc<Settings> {
    if let Some(s) = st.service::<Arc<Settings>>() {
        return s;
    }
    // Live from the web UI (docs/SETTINGS.md WP4b): rebuilt when the live configuration changes.
    let live = st.live_config();
    if let Some(LiveAccountSettings(c, s)) = st.service::<LiveAccountSettings>()
        && Arc::ptr_eq(&c, &live)
    {
        return s;
    }
    let s = Arc::new(Settings::from_core(&live));
    if st.service::<pl_core::live::LiveConfig>().is_some() {
        st.set_service(LiveAccountSettings(live, s.clone()));
    } else {
        // without live configuration (tests, CLI) as before: once per process
        st.set_service(s.clone());
    }
    s
}

/// Account settings together with the live configuration they were built from.
#[derive(Clone)]
struct LiveAccountSettings(Arc<pl_core::Config>, Arc<Settings>);

fn cookie_val(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = pl_authz::csrf::parse_cookies(headers);
    pl_authz::csrf::cookie(&cookies, name).map(str::to_string)
}

impl Ctx {
    /// Split the request, load session/messages. `body` is only evaluated for urlencoded POST.
    pub async fn new(st: AppState, req: Request<Body>) -> R<Self> {
        let s = settings(&st);
        let (parts, body) = req.into_parts();
        let remote_addr = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip().to_string());
        let bytes: Bytes = axum::body::to_bytes(body, pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE).await.unwrap_or_default();
        let headers = parts.headers;
        let is_form = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/x-www-form-urlencoded"));
        let form = if parts.method == Method::POST && is_form { parse_qs(&String::from_utf8_lossy(&bytes)) } else { Vec::new() };
        let query = parse_qs(parts.uri.query().unwrap_or(""));
        let lang = pl_core::locale::language_from_headers(&headers, &s.language_cookie_name);
        let cache = Cache::new(&st);
        let session = Session::load(st.db(), &s, cookie_val(&headers, &s.session_cookie_name).as_deref()).await?;
        let messages = Messages::load(&s.secret_key, cookie_val(&headers, crate::messages::COOKIE_NAME).as_deref());
        // CsrfViewMiddleware.process_request / _get_secret
        let mut csrf_secret = None;
        let mut csrf_cookie_invalid = None;
        let mut csrf_needs_update = false;
        if let Some(c) = cookie_val(&headers, &s.csrf_cookie_name) {
            match pl_authz::csrf::check_token_format(&c) {
                Ok(()) => {
                    csrf_secret = Some(if c.len() == pl_authz::csrf::CSRF_TOKEN_LENGTH {
                        pl_authz::csrf::unmask_cipher_token(&c).unwrap_or_default()
                    } else {
                        c
                    });
                }
                Err(reason) => {
                    csrf_cookie_invalid = Some(format!("CSRF cookie {reason}."));
                    csrf_secret = Some(new_csrf_secret());
                    csrf_needs_update = true;
                }
            }
        }
        Ok(Ctx {
            st,
            s,
            cache,
            method: parts.method,
            path: parts.uri.path().to_string(),
            query,
            headers,
            form,
            lang,
            remote_addr,
            session,
            messages,
            csrf_secret,
            csrf_cookie_invalid,
            csrf_needs_update,
            user: None,
            never_cache: false,
        })
    }

    /// `request.GET.get(k)`.
    pub fn get(&self, k: &str) -> Option<&str> {
        last(&self.query, k)
    }

    /// `request.POST.get(k)`.
    pub fn post(&self, k: &str) -> Option<&str> {
        last(&self.form, k)
    }

    /// allauth `get_request_param`: POST, then GET.
    pub fn param(&self, k: &str) -> Option<&str> {
        self.post(k).or_else(|| self.get(k))
    }

    pub fn is_post(&self) -> bool {
        self.method == Method::POST
    }

    /// `request.get_host()` (ALLOWED_HOSTS checked; pl-server forwards invalid hosts to Django).
    pub fn host(&self) -> String {
        pl_authz::csrf::get_host(&self.s.auth, &self.headers).unwrap_or_else(|| pl_authz::csrf::raw_host(&self.s.auth, &self.headers))
    }

    /// `request.is_secure()`.
    pub fn is_secure(&self) -> bool {
        pl_authz::csrf::is_secure(&self.s.auth, &self.headers)
    }

    /// `request.build_absolute_uri(path)`.
    pub fn absolute(&self, path: &str) -> String {
        format!("{}://{}{}", if self.is_secure() { "https" } else { "http" }, self.host(), path)
    }

    /// `request.get_full_path()`.
    pub fn full_path(&self, raw_query: Option<&str>) -> String {
        match raw_query {
            Some(q) if !q.is_empty() => format!("{}?{}", self.path, q),
            _ => self.path.clone(),
        }
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).map(|v| pl_authz::csrf::latin1(v.as_bytes()))
    }

    /// allauth `get_client_ip` (+ `truncate_ip`). `Err` → ImproperlyConfigured (500).
    pub fn client_ip(&self) -> R<Option<String>> {
        let ip = crate::ratelimit::client_ip(
            self.s.trusted_client_ip_header.as_deref(),
            self.s.trusted_proxy_count,
            |h| self.header(h),
            self.remote_addr.as_deref(),
        )
        .map_err(|_| Internal("ALLAUTH_TRUSTED_PROXY_COUNT does not match X-Forwarded-For".into()))?;
        Ok(ip.map(|i| crate::ratelimit::truncate_ip(&i)))
    }

    /// CSRF check of an unsafe method. `Err(reason)` → 403 page.
    pub fn check_csrf(&self) -> Result<(), String> {
        if matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE") {
            return Ok(());
        }
        let mut h = self.headers.clone();
        if let Some(reason) = &self.csrf_cookie_invalid {
            // Origin/Referer are checked beforehand
            let mut no_cookie = h.clone();
            no_cookie.remove(axum::http::header::COOKIE);
            pl_authz::csrf::enforce_csrf(&self.s.auth, &pl_authz::csrf::RequestInfo { method: "POST", headers: &no_cookie })
                .or_else(|e| if e == pl_authz::csrf::REASON_NO_CSRF_COOKIE || e.starts_with("CSRF token") { Ok(()) } else { Err(e) })?;
            return Err(reason.clone());
        }
        if let Some(t) = self.post("csrfmiddlewaretoken").filter(|t| !t.is_empty())
            && let Ok(v) = HeaderValue::from_str(t)
        {
            h.insert("x-csrftoken", v);
        }
        pl_authz::csrf::enforce_csrf(&self.s.auth, &pl_authz::csrf::RequestInfo { method: self.method.as_str(), headers: &h })
            .map_err(|e| e.replace("from the 'X-Csrftoken' HTTP header", if self.post("csrfmiddlewaretoken").is_some_and(|t| !t.is_empty()) { "from POST" } else { "from the 'X-Csrftoken' HTTP header" }))
    }

    /// `get_token(request)`: masked token, cookie is (re)sent.
    pub fn csrf_token(&mut self) -> String {
        let secret = self.csrf_secret.get_or_insert_with(new_csrf_secret).clone();
        self.csrf_needs_update = true;
        pl_authz::csrf::mask_cipher_secret(&secret)
    }

    /// `rotate_token(request)`.
    pub fn rotate_csrf(&mut self) {
        self.csrf_secret = Some(new_csrf_secret());
        self.csrf_needs_update = true;
    }

    /// `{% csrf_token %}`.
    pub fn csrf_input(&mut self) -> String {
        format!("<input type=\"hidden\" name=\"csrfmiddlewaretoken\" value=\"{}\">", self.csrf_token())
    }

    /// Middleware return path: save session, cookies, `never_cache`, authenticated marker.
    pub async fn finish(mut self, mut resp: Response) -> Response {
        let status = resp.status().as_u16();
        let h = resp.headers_mut();
        if self.never_cache {
            let now = http_date(Utc::now());
            if let Ok(v) = HeaderValue::from_str(&now) {
                h.insert(EXPIRES, v);
            }
            h.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0, no-cache, no-store, must-revalidate, private"));
        }
        let mut cookies: Vec<String> = Vec::new();
        if let Some(c) = self.messages.finish(&self.s.secret_key, Utc::now().timestamp()) {
            cookies.push(c);
        }
        if self.csrf_needs_update
            && let Some(secret) = &self.csrf_secret
        {
            let exp = http_date(Utc::now() + chrono::Duration::seconds(CSRF_COOKIE_AGE));
            cookies.push(format!("{}={secret}; expires={exp}; Max-Age={CSRF_COOKIE_AGE}; Path=/; SameSite=Lax", self.s.csrf_cookie_name));
            pl_server_vary(h);
        }
        match self.session.finish(self.st.db(), &self.cache, &self.s, status).await {
            Ok(Some(c)) => cookies.push(c),
            Ok(None) => {}
            Err(e) => return server_error(&Internal(format!("{e:?}"))),
        }
        let h = resp.headers_mut();
        for c in cookies {
            if let Ok(v) = HeaderValue::from_str(&c) {
                h.append(SET_COOKIE, v);
            }
        }
        if self.user.is_some() {
            resp.extensions_mut().insert(pl_core::Authenticated);
        }
        resp
    }
}

fn pl_server_vary(h: &mut HeaderMap) {
    let existing = h.get(VARY).and_then(|v| v.to_str().ok()).map(str::to_string);
    let v = match existing {
        Some(e) if e.split(',').any(|x| x.trim().eq_ignore_ascii_case("cookie")) => e,
        Some(e) => format!("{e}, Cookie"),
        None => "Cookie".into(),
    };
    if let Ok(v) = HeaderValue::from_str(&v) {
        h.insert(VARY, v);
    }
}

/// `_get_new_csrf_string()`: 32 characters `[a-zA-Z0-9]`.
pub fn new_csrf_secret() -> String {
    pl_authz::hashers::get_random_string(pl_authz::csrf::CSRF_SECRET_LENGTH)
}

/// `TemplateResponse` (`text/html; charset=utf-8`).
pub fn html(status: StatusCode, body: String) -> Response {
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = status;
    r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    r
}

/// `HttpResponseRedirect(url)`.
pub fn redirect(url: &str) -> Response {
    let mut r = html(StatusCode::FOUND, String::new());
    // Django: iri_to_uri(url)
    let loc = iri_to_uri(url);
    if let Ok(v) = HeaderValue::from_str(&loc) {
        r.headers_mut().insert(LOCATION, v);
    }
    r
}

/// `django.utils.encoding.iri_to_uri` (characters outside the allowed set %-encoded).
pub fn iri_to_uri(iri: &str) -> String {
    const SAFE: &[u8] = b"/#%[]=:;$&()+,!?*@'~";
    let mut out = String::new();
    for b in iri.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-".contains(&b) || SAFE.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Django `PermissionDenied` without 403.html.
pub fn forbidden() -> Response {
    html(StatusCode::FORBIDDEN, "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>403 Forbidden</title>\n</head>\n<body>\n  <h1>403 Forbidden</h1><p></p>\n</body>\n</html>\n".into())
}

/// Django `Http404` without 404.html (DEBUG=False).
pub fn not_found(path: &str) -> Response {
    let _ = path;
    html(StatusCode::NOT_FOUND, "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Not Found</title>\n</head>\n<body>\n  <h1>Not Found</h1><p>The requested resource was not found on this server.</p>\n</body>\n</html>\n".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qs_and_iri() {
        assert_eq!(parse_qs("next=/a/?x=1&b=&c=a+b"), vec![("next".into(), "/a/?x=1".into()), ("b".into(), "".into()), ("c".into(), "a b".into())]);
        assert_eq!(iri_to_uri("/documents/?q=ä b"), "/documents/?q=%C3%A4%20b");
        assert_eq!(new_csrf_secret().len(), 32);
    }
}
