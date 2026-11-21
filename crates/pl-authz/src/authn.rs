//! Request authentication in the exact order of paperless' DRF `DEFAULT_AUTHENTICATION_CLASSES`.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (REST_FRAMEWORK, remote user, AUTO_LOGIN),
//!           src/paperless/auth.py@v3.4.2 (PaperlessBasicAuthentication, PaperlessRemoteUserAuthentication,
//!           AngularApiAuthenticationOverride, AutoLoginMiddleware, HttpRemoteUserMiddleware),
//!           src/paperless/signals.py@v3.4.2 (handle_failed_login),
//!           rest_framework/authentication.py@3.16 (BasicAuthentication, TokenAuthentication,
//!           SessionAuthentication, RemoteUserAuthentication, get_authorization_header),
//!           rest_framework/request.py@3.16 (Request._authenticate),
//!           django/contrib/auth/__init__.py@5.2 (authenticate, get_user),
//!           django/contrib/auth/backends.py@5.2 (ModelBackend, RemoteUserBackend),
//!           allauth/account/auth_backends.py@65 (AuthenticationBackend._authenticate_by_username),
//!           django/contrib/sessions/backends/{base,db,cached_db}.py@5.2.
//!
//! Order (first match wins, an exception aborts):
//! 1. `PaperlessRemoteUserAuthentication` – only with `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API`
//! 2. `PaperlessBasicAuthentication`
//! 3. `TokenAuthentication`
//!    (3b. PaperRust device token `Bearer prt_…`, see [`crate::device`])
//! 4. `SessionAuthentication` (Django `request.user`: AutoLoginMiddleware or session cookie) + CSRF
//! 5. `AngularApiAuthenticationOverride` – only with `PAPERLESS_DEBUG`

use crate::config::AuthConfig;
use crate::csrf::{self, RequestInfo};
use crate::error::AuthError;
use crate::hashers;
use crate::signing;
use crate::user::{self, User};
use axum::http::{HeaderMap, Method};
use pl_core::dbconn::retry_stale_once;
use sqlx::PgPool;
use std::sync::Arc;

/// Which DRF authentication class authenticated the request (`request.successful_authenticator`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// `PaperlessRemoteUserAuthentication` (header configured by `PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME`).
    RemoteUser,
    /// `PaperlessBasicAuthentication`.
    Basic,
    /// `TokenAuthentication` (`Authorization: Token <key>`).
    Token,
    /// `SessionAuthentication` with a Django session cookie (CSRF enforced).
    Session,
    /// `SessionAuthentication` where `request.user` came from `AutoLoginMiddleware` (CSRF enforced).
    AutoLogin,
    /// `AngularApiAuthenticationOverride` (DEBUG only).
    DebugAngular,
    /// PaperRust device token (`Authorization: Bearer prt_…`, [`crate::device`]); no
    /// Django counterpart (known difference, docs/contract/pl-authz.md).
    DeviceToken,
}

impl AuthMethod {
    /// Whether `request.user` was already set by Django's middleware (`AuthenticationMiddleware`
    /// session, `AutoLoginMiddleware`, `HttpRemoteUserMiddleware`) before DRF authenticated, i.e.
    /// what middleware running before DRF (e.g. `AuditlogMiddleware`) sees as the actor.
    /// Token/Basic/DebugAngular are DRF-only authenticators.
    pub fn is_django_request_user(self) -> bool {
        matches!(self, Self::Session | Self::AutoLogin | Self::RemoteUser)
    }
}

/// What happened to the Django session while authenticating (for the server's response
/// handling: `SessionMiddleware` adds `Vary: Cookie` when accessed and deletes the cookie when the
/// session is empty although a cookie was sent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionState {
    /// The session was loaded (`request.session` accessed).
    pub accessed: bool,
    /// A session cookie was sent.
    pub cookie_sent: bool,
    /// The loaded session turned out empty/missing/expired (or was flushed).
    pub empty: bool,
    /// `get_user` flushed the session (hash mismatch) – row deleted.
    pub flushed: bool,
}

impl SessionState {
    /// `Set-Cookie` value Django's `SessionMiddleware` would send to delete the cookie,
    /// if it would (`cookie present and session empty`).
    pub fn delete_cookie_header(&self, cfg: &AuthConfig) -> Option<String> {
        (self.accessed && self.cookie_sent && self.empty)
            .then(|| format!("{}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=Lax", cfg.session_cookie_name()))
    }
}

/// Request data beyond method/path/headers that Django's request carries (`REMOTE_ADDR`,
/// `request.POST["csrfmiddlewaretoken"]`). The extractors fill it from the request extensions
/// (`ConnectInfo<SocketAddr>`, [`crate::csrf::FormCsrfToken`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestMeta {
    /// `request.META["REMOTE_ADDR"]` (TCP peer), for the failed-login log (ipware).
    pub remote_addr: Option<std::net::IpAddr>,
    /// `csrfmiddlewaretoken` from a form-encoded POST body (gap S6).
    pub csrf_form_token: Option<String>,
}

/// Result of authenticating one request (DRF `request.user` / `request.auth`).
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// `request.user` if authenticated (None = AnonymousUser).
    pub user: Option<Arc<User>>,
    /// `request.successful_authenticator`.
    pub method: Option<AuthMethod>,
    /// `request.auth` for token authentication (the token key).
    pub token_key: Option<String>,
    /// HTTP method of the request (DRF permission maps depend on it).
    pub http_method: Method,
    /// `get_authenticate_header`: challenge of the first authentication class
    /// (`None` → NotAuthenticated renders as 403).
    pub www_authenticate: Option<String>,
    /// Session side effects.
    pub session: SessionState,
}

impl AuthContext {
    /// An anonymous context (for tests and internal calls).
    pub fn anonymous(http_method: Method, www_authenticate: Option<String>) -> Self {
        Self { user: None, method: None, token_key: None, http_method, www_authenticate, session: SessionState::default() }
    }

    /// An authenticated context (for tests and internal calls).
    pub fn for_user(user: User, method: AuthMethod, http_method: Method) -> Self {
        Self {
            user: Some(Arc::new(user)),
            method: Some(method),
            token_key: None,
            http_method,
            www_authenticate: Some("Token".into()),
            session: SessionState::default(),
        }
    }

    /// `request.user` when authenticated.
    pub fn user(&self) -> Option<&User> {
        self.user.as_deref()
    }

    /// `request.user.is_authenticated`.
    pub fn is_authenticated(&self) -> bool {
        self.user.is_some()
    }

    /// `NotAuthenticated()` with this request's challenge.
    pub fn not_authenticated(&self) -> AuthError {
        AuthError::NotAuthenticated { www_authenticate: self.www_authenticate.clone() }
    }

    /// `APIView.permission_denied(request)`: NotAuthenticated if no authenticator succeeded,
    /// otherwise `PermissionDenied` with the default message.
    pub fn deny(&self) -> AuthError {
        self.deny_with(crate::error::PERMISSION_DENIED)
    }

    /// `permission_denied(request, message=...)`.
    pub fn deny_with(&self, message: &str) -> AuthError {
        if self.method.is_none() { self.not_authenticated() } else { AuthError::PermissionDenied { detail: message.into() } }
    }

    /// `IsAuthenticated`: the user, or the DRF error.
    pub fn require_authenticated(&self) -> Result<&User, AuthError> {
        self.user().ok_or_else(|| self.deny())
    }
}

/// Shared authenticator (DB pool + settings). Put it into the request extensions
/// (`axum::Extension(authz)`) so the extractors find it.
#[derive(Clone)]
pub struct Authz {
    inner: Arc<AuthzInner>,
}

struct AuthzInner {
    db: PgPool,
    config: AuthConfig,
}

impl Authz {
    pub fn new(db: PgPool, config: AuthConfig) -> Self {
        // L5: track readiness of the grant index in the background (once per process).
        crate::grants::spawn_refresher(db.clone());
        Self { inner: Arc::new(AuthzInner { db, config }) }
    }

    pub fn db(&self) -> &PgPool {
        &self.inner.db
    }

    pub fn config(&self) -> &AuthConfig {
        &self.inner.config
    }

    /// `APIView.get_authenticate_header`: header of the first authentication class.
    pub fn authenticate_header(&self, headers: &HeaderMap) -> Option<String> {
        if self.config().remote_user_api {
            // RemoteUserAuthentication has no authenticate_header -> 403 instead of 401
            return None;
        }
        // PaperlessBasicAuthentication.authenticate_header
        let auth = csrf::meta_header(headers, "HTTP_AUTHORIZATION").unwrap_or_default();
        if auth.to_lowercase().starts_with("basic ") { Some("Basic realm=\"api\"".into()) } else { Some("Token".into()) }
    }

    /// Authenticate a DRF API request (`Request._authenticate`).
    pub async fn authenticate(&self, method: &Method, path: &str, headers: &HeaderMap) -> Result<AuthContext, AuthError> {
        self.authenticate_with(method, path, headers, &RequestMeta::default()).await
    }

    /// [`Authz::authenticate`] with the client address and the form CSRF token of the request.
    pub async fn authenticate_with(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        meta: &RequestMeta,
    ) -> Result<AuthContext, AuthError> {
        let cfg = self.config();
        let www = self.authenticate_header(headers);
        let mut ctx = AuthContext::anonymous(method.clone(), www.clone());
        let fail = |detail: &str| AuthError::AuthenticationFailed { detail: detail.into(), www_authenticate: www.clone() };

        // 1. PaperlessRemoteUserAuthentication
        if cfg.remote_user_api {
            let remote = csrf::meta_header(headers, &cfg.remote_user_header);
            if let Some(u) = self.remote_user_authenticate(remote.as_deref(), headers, meta).await?
                && u.is_active
            {
                ctx.user = Some(Arc::new(u));
                ctx.method = Some(AuthMethod::RemoteUser);
                return Ok(ctx);
            }
        }

        // Raw Authorization header (bytes, ASGI-joined with ",").
        let authz_raw: Option<Vec<u8>> = authorization_bytes(headers);
        let parts: Vec<&[u8]> = authz_raw.as_deref().map(py_bytes_split).unwrap_or_default();

        // 2. PaperlessBasicAuthentication
        if parts.first().is_some_and(|p| p.eq_ignore_ascii_case(b"basic")) {
            if parts.len() == 1 {
                return Err(fail("Invalid basic header. No credentials provided."));
            }
            if parts.len() > 2 {
                return Err(fail("Invalid basic header. Credentials string should not contain spaces."));
            }
            let decoded = py_b64decode(parts[1]).map(|b| match String::from_utf8(b.clone()) {
                Ok(s) => s,
                Err(_) => csrf::latin1(&b),
            });
            let Some((userid, password)) = decoded.as_deref().and_then(|d| d.split_once(':')) else {
                return Err(fail("Invalid basic header. Credentials not correctly base64 encoded."));
            };
            // PaperRust: throttle failed attempts via Basic auth like the login page (finding S-2).
            // IP only from X-Forwarded-For of trusted proxies (security review 2026-11-19 F7).
            let ip = crate::clientip::throttle_client_ip(headers, meta.remote_addr, &cfg.trusted_proxies);
            if crate::basic_throttle::is_blocked(ip, userid) {
                self.log_failed_login(Some(userid), headers, meta);
                return Err(fail(crate::basic_throttle::DETAIL));
            }
            let Some(u) = self.authenticate_credentials_from(userid, password, headers, meta).await? else {
                crate::basic_throttle::record_failure(ip, userid);
                return Err(fail("Invalid username/password."));
            };
            if !u.is_active {
                return Err(fail("User inactive or deleted."));
            }
            if retry_stale_once(|| user::is_mfa_enabled(self.db(), u.id)).await? {
                return Err(fail("MFA required"));
            }
            ctx.user = Some(Arc::new(u));
            ctx.method = Some(AuthMethod::Basic);
            return Ok(ctx);
        }

        // 3. TokenAuthentication
        if parts.first().is_some_and(|p| p.eq_ignore_ascii_case(b"token")) {
            if parts.len() == 1 {
                return Err(fail("Invalid token header. No credentials provided."));
            }
            if parts.len() > 2 {
                return Err(fail("Invalid token header. Token string should not contain spaces."));
            }
            let Ok(key) = std::str::from_utf8(parts[1]) else {
                return Err(fail("Invalid token header. Token string should not contain invalid characters."));
            };
            // NUL cannot be sent to PostgreSQL; such a key cannot exist.
            let u = if key.contains('\0') { None } else { retry_stale_once(|| user::load_user_by_token(self.db(), key)).await? };
            let Some(u) = u else {
                return Err(fail("Invalid token."));
            };
            if !u.is_active {
                return Err(fail("User inactive or deleted."));
            }
            ctx.user = Some(Arc::new(u));
            ctx.method = Some(AuthMethod::Token);
            ctx.token_key = Some(key.to_string());
            return Ok(ctx);
        }

        // 3b. PaperRust device token (`Bearer prt_…`). Other `Bearer` values stay ignored by all
        // authenticators (unchanged DRF behaviour); Django never sees this scheme.
        if parts.len() == 2 && parts[0].eq_ignore_ascii_case(b"bearer") && parts[1].starts_with(crate::device::PREFIX.as_bytes()) {
            let key = std::str::from_utf8(parts[1]).unwrap_or("");
            let uid = retry_stale_once(|| crate::device::lookup_user_id(self.db(), key)).await?;
            let u = match uid {
                Some(id) => retry_stale_once(|| user::load_user_by_id(self.db(), id)).await?,
                None => None,
            };
            let Some(u) = u else {
                return Err(fail("Invalid token."));
            };
            if !u.is_active {
                return Err(fail("User inactive or deleted."));
            }
            ctx.user = Some(Arc::new(u));
            ctx.method = Some(AuthMethod::DeviceToken);
            return Ok(ctx);
        }

        // 4. SessionAuthentication: Django's request.user
        let (django_user, via) = self.django_request_user(method, path, headers, &mut ctx.session).await?;
        if let Some(u) = django_user.filter(|u| u.is_active) {
            // enforce_csrf
            let info = RequestInfo { method: method.as_str(), headers };
            if let Err(reason) = csrf::enforce_csrf_with_form(cfg, &info, meta.csrf_form_token.as_deref()) {
                return Err(AuthError::PermissionDenied { detail: format!("CSRF Failed: {reason}") });
            }
            ctx.user = Some(Arc::new(u));
            ctx.method = Some(via);
            return Ok(ctx);
        }

        // 5. AngularApiAuthenticationOverride
        if cfg.debug {
            let referer = csrf::meta_header(headers, "HTTP_REFERER");
            if referer.is_some_and(|r| r.starts_with("http://localhost:4200/")) {
                let u = retry_stale_once(|| user::first_staff_user(self.db())).await?;
                tracing::debug!(target: "paperless.auth", "Auto-Login with user {:?}", u.as_ref().map(|u| &u.username));
                // (user, None) is a successful authentication even when user is None.
                ctx.user = u.map(Arc::new);
                ctx.method = Some(AuthMethod::DebugAngular);
                return Ok(ctx);
            }
        }

        Ok(ctx)
    }

    /// `django.contrib.auth.authenticate(request, username=..., password=...)` with paperless'
    /// backends: ModelBackend (exact username) then allauth (case-insensitive username).
    /// Rehashes the password like Django's `check_password` setter. Logs failed logins like
    /// `paperless.signals.handle_failed_login`. Also used by `/api/token/`.
    pub async fn authenticate_credentials(&self, username: &str, password: &str) -> Result<Option<User>, AuthError> {
        self.authenticate_credentials_from(username, password, &HeaderMap::new(), &RequestMeta::default()).await
    }

    /// [`Authz::authenticate_credentials`] with the request's headers and peer address, so a
    /// failed login is logged with the client IP like `handle_failed_login` (ipware).
    pub async fn authenticate_credentials_from(
        &self,
        username: &str,
        password: &str,
        headers: &HeaderMap,
        meta: &RequestMeta,
    ) -> Result<Option<User>, AuthError> {
        // ModelBackend.authenticate
        let mut exact = retry_stale_once(|| user::load_user_by_username(self.db(), username)).await?;
        let mut checked_id = None;
        match &mut exact {
            None => run_blocking_dummy(password).await,
            Some(u) => {
                checked_id = Some(u.id);
                if self.check_password_with_setter(u, password).await? && u.is_active {
                    return Ok(exact);
                }
            }
        }
        // allauth AuthenticationBackend.authenticate: empty password -> None (no timing
        // mitigation); _authenticate_by_username(get_user_by_username); dummy hash if no
        // password was checked.
        if !password.is_empty() && !username.is_empty() {
            let mut found = retry_stale_once(|| user::load_user_by_username_iexact(self.db(), username)).await?;
            match (allauth_hash_run(found.as_ref().map(|u| u.id), checked_id), found.as_mut()) {
                (AllauthHashRun::Check, Some(u)) => {
                    if self.check_password_with_setter(u, password).await? && u.is_active {
                        return Ok(found);
                    }
                }
                _ => run_blocking_dummy(password).await,
            }
        } else if !password.is_empty() {
            run_blocking_dummy(password).await;
        }
        self.log_failed_login(Some(username), headers, meta);
        Ok(None)
    }

    /// `paperless.signals.handle_failed_login` with `IpWare(proxy_list=TRUSTED_PROXIES)`.
    fn log_failed_login(&self, username: Option<&str>, headers: &HeaderMap, meta: &RequestMeta) {
        crate::clientip::log_failed_login(username, headers, meta.remote_addr, &self.config().trusted_proxies);
    }

    /// `user.check_password(raw)` incl. the setter that stores an upgraded hash – in the DB
    /// **and** on the object (Django's setter calls `set_password` on the same instance, so a
    /// later `get_session_auth_hash()` / `update_session_auth_hash` sees the new hash).
    async fn check_password_with_setter(&self, u: &mut User, password: &str) -> Result<bool, AuthError> {
        let (pw, enc) = (password.to_string(), u.password.clone());
        let v = tokio::task::spawn_blocking(move || hashers::verify_password(&pw, &enc)).await.map_err(AuthError::internal)?;
        if v.is_correct && v.must_update {
            let pw = password.to_string();
            let new = tokio::task::spawn_blocking(move || hashers::make_password(&pw)).await.map_err(AuthError::internal)?;
            user::update_password(self.db(), u.id, &new).await?;
            u.password = new;
        }
        Ok(v.is_correct)
    }

    /// `authenticate(request, remote_user=value)` → RemoteUserBackend (create unknown users).
    async fn remote_user_authenticate(
        &self,
        remote: Option<&str>,
        headers: &HeaderMap,
        meta: &RequestMeta,
    ) -> Result<Option<User>, AuthError> {
        match remote {
            Some(name) if !name.is_empty() => {
                // idempotent (ON CONFLICT DO NOTHING) → safe to retry once after a dead connection
                let u = retry_stale_once(|| user::get_or_create_user(self.db(), name)).await?;
                // user_can_authenticate
                Ok(u.filter(|u| u.is_active))
            }
            _ => {
                // all backends returned None -> user_login_failed(credentials={"remote_user": ...})
                self.log_failed_login(None, headers, meta);
                Ok(None)
            }
        }
    }

    /// Django's `request.user` for the API: AutoLoginMiddleware, else the session
    /// (`AuthenticationMiddleware` → `get_user`).
    async fn django_request_user(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        state: &mut SessionState,
    ) -> Result<(Option<User>, AuthMethod), AuthError> {
        let cfg = self.config();
        if let Some(name) = &cfg.auto_login_username {
            let skip = path.starts_with("/api/token/") && method == Method::POST;
            if !skip
                && let Some(u) = retry_stale_once(|| user::load_user_by_username(self.db(), name)).await?
                && u.is_active
            {
                return Ok((Some(u), AuthMethod::AutoLogin));
            }
        }
        Ok((self.session_user(headers, state).await?, AuthMethod::Session))
    }

    /// `django.contrib.auth.get_user(request)` from the session cookie.
    pub async fn session_user(&self, headers: &HeaderMap, state: &mut SessionState) -> Result<Option<User>, AuthError> {
        let cfg = self.config();
        let cookies = csrf::parse_cookies(headers);
        let Some(key) = csrf::cookie(&cookies, &cfg.session_cookie_name()).map(str::to_string) else {
            return Ok(None);
        };
        state.cookie_sent = true;
        state.accessed = true;
        // SessionBase._validate_session_key: key and len(key) >= 8
        if key.chars().count() < 8 || key.contains('\0') {
            state.empty = true;
            return Ok(None);
        }
        let row: Option<(String,)> = retry_stale_once(|| {
            sqlx::query_as("SELECT session_data FROM django_session WHERE session_key = $1 AND expire_date > now()")
                .bind(&key)
                .fetch_optional(self.db())
        })
        .await?;
        let Some((data,)) = row else {
            state.empty = true;
            return Ok(None);
        };
        let session = signing::decode_session(&data, &cfg.secret_key);
        if session.is_empty() {
            state.empty = true;
        }
        let user_id = match session.get("_auth_user_id") {
            Some(serde_json::Value::String(s)) => s.trim().parse::<i64>().ok(),
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            _ => None,
        };
        let backend = session.get("_auth_user_backend").and_then(|v| v.as_str());
        let (Some(user_id), Some(backend)) = (user_id, backend) else {
            return Ok(None);
        };
        if !cfg.authentication_backends().contains(&backend) || backend == "guardian.backends.ObjectPermissionBackend" {
            return Ok(None);
        }
        // ModelBackend.get_user: user if user_can_authenticate (is_active)
        let Some(u) = retry_stale_once(|| user::load_user_by_id(self.db(), user_id)).await?.filter(|u| u.is_active) else {
            return Ok(None);
        };
        let session_hash = session.get("_auth_user_hash").and_then(|v| v.as_str()).unwrap_or("");
        let verified =
            !session_hash.is_empty() && signing::constant_time_eq(session_hash.as_bytes(), u.session_auth_hash(&cfg.secret_key).as_bytes());
        if !verified {
            // request.session.flush(); SECRET_KEY_FALLBACKS is never set by paperless.
            sqlx::query("DELETE FROM django_session WHERE session_key = $1").bind(&key).execute(self.db()).await?;
            state.flushed = true;
            state.empty = true;
            return Ok(None);
        }
        Ok(Some(u))
    }
}

/// Hash run in the allauth step of [`Authz::authenticate_credentials_from`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllauthHashRun {
    /// Check the real password of the found row.
    Check,
    /// Dummy hash (no row, or the same row as ModelBackend).
    Dummy,
}

/// `found`: row from `load_user_by_username_iexact`, `checked`: row already checked by ModelBackend.
/// Security review 2026-11-19 F5: if it is the same row, the result is not checked again
/// (same result, no second rehash), but a dummy hash is computed. Otherwise a known
/// name would need 1 hash run, an unknown one 2 – the name would be recognisable from the response time.
/// The form path (pl-accounts `auth.rs`) checks the same row twice: likewise 2.
fn allauth_hash_run(found: Option<i64>, checked: Option<i64>) -> AllauthHashRun {
    match found {
        Some(id) if Some(id) != checked => AllauthHashRun::Check,
        _ => AllauthHashRun::Dummy,
    }
}

async fn run_blocking_dummy(password: &str) {
    let pw = password.to_string();
    let _ = tokio::task::spawn_blocking(move || hashers::dummy_hash(&pw)).await;
}

/// `request.META.get("HTTP_AUTHORIZATION")` as bytes (latin-1 roundtrip == raw bytes).
fn authorization_bytes(headers: &HeaderMap) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    for v in headers.get_all(axum::http::header::AUTHORIZATION) {
        out = Some(match out {
            Some(mut prev) => {
                prev.push(b',');
                prev.extend_from_slice(v.as_bytes());
                prev
            }
            None => v.as_bytes().to_vec(),
        });
    }
    out
}

/// Python `bytes.split()` (ASCII whitespace, runs collapsed).
pub fn py_bytes_split(b: &[u8]) -> Vec<&[u8]> {
    b.split(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).filter(|p| !p.is_empty()).collect()
}

/// `base64.b64decode(s)` (non-strict `binascii.a2b_base64`): characters outside the standard
/// alphabet are skipped, decoding stops at complete padding; `None` = `binascii.Error`.
pub fn py_b64decode(s: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut quad_pos = 0u8;
    let mut left: u8 = 0;
    let mut pads = 0u8;
    for &c in s {
        if c == b'=' {
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    return Some(out);
                }
            }
            continue;
        }
        let Some(v) = val(c) else { continue };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = v;
            }
            1 => {
                quad_pos = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    if quad_pos != 0 { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_like_python() {
        // Expected values from CPython base64.b64decode (see testvectors/gen_vectors.py output).
        assert_eq!(py_b64decode(b"YWRtaW46cGFzcw==").unwrap(), b"admin:pass");
        assert_eq!(py_b64decode(b"YWRt aW46cGFzcw==").unwrap(), b"admin:pass");
        assert_eq!(py_b64decode(b"YWRtaW46cGFzcw==junk").unwrap(), b"admin:pass");
        assert_eq!(py_b64decode(b"YWRtaW46cGFzcw"), None);
        assert_eq!(py_b64decode(b"Y=WRtaW46cGFzcw==").unwrap(), b"admin:pass");
        assert_eq!(py_b64decode(b"YWRtaW46cGFzcwA=").unwrap(), b"admin:pass\0");
        assert_eq!(py_b64decode(b"").unwrap(), b"");
    }

    /// Security review 2026-11-19 F5: Basic/token login with a wrong password needs the same number of
    /// hash runs for known and unknown names (ModelBackend 1 + allauth 1).
    #[test]
    fn same_hash_runs_for_known_and_unknown_names() {
        // ModelBackend always computes exactly one hash (real or dummy); allauth must in every case
        // compute another one (`Check` or `Dummy`, never "skip").
        // unknown name: dummy + dummy
        assert_eq!(allauth_hash_run(None, None), AllauthHashRun::Dummy);
        // known name, hit exactly: real check + dummy (not skipped)
        assert_eq!(allauth_hash_run(Some(7), Some(7)), AllauthHashRun::Dummy);
        // known only case-insensitively: dummy + real check
        assert_eq!(allauth_hash_run(Some(7), None), AllauthHashRun::Check);
        // exactly another user than iexact (names differ only in spelling)
        assert_eq!(allauth_hash_run(Some(8), Some(7)), AllauthHashRun::Check);
    }

    #[test]
    fn split_like_python() {
        assert_eq!(py_bytes_split(b"  Token \t abc  "), vec![&b"Token"[..], &b"abc"[..]]);
        assert!(py_bytes_split(b"   ").is_empty());
    }

    fn authz() -> Authz {
        // connect_lazy never connects unless a query runs.
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid").unwrap();
        Authz::new(pool, AuthConfig::for_tests("s"))
    }

    fn hm(pairs: &[(&str, &[u8])]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), axum::http::HeaderValue::from_bytes(v).unwrap());
        }
        h
    }

    #[tokio::test]
    async fn header_errors_without_db() {
        let a = authz();
        let run = |h: HeaderMap| {
            let a = a.clone();
            async move { a.authenticate(&Method::GET, "/api/documents/", &h).await }
        };
        let e = run(hm(&[("authorization", b"Basic")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid basic header. No credentials provided.");
        assert_eq!(e.www_authenticate(), Some("Token"), "'basic' without space -> Token");
        let e = run(hm(&[("authorization", b"Basic a b")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid basic header. Credentials string should not contain spaces.");
        assert_eq!(e.www_authenticate(), Some("Basic realm=\"api\""));
        let e = run(hm(&[("authorization", b"Basic invalid")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid basic header. Credentials not correctly base64 encoded.");
        assert_eq!(e.status(), axum::http::StatusCode::UNAUTHORIZED);
        let e = run(hm(&[("authorization", b"basic YWRtaW4=")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid basic header. Credentials not correctly base64 encoded.");
        let e = run(hm(&[("authorization", b"Token")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid token header. No credentials provided.");
        let e = run(hm(&[("authorization", b"token a b")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid token header. Token string should not contain spaces.");
        let e = run(hm(&[("authorization", b"Token \xff\xfe")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid token header. Token string should not contain invalid characters.");
        // No credentials at all: anonymous, challenge "Token".
        let ctx = run(hm(&[])).await.unwrap();
        assert!(ctx.user.is_none());
        assert_eq!(ctx.not_authenticated().status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(ctx.deny().detail(), "Authentication credentials were not provided.");
        // Unknown scheme is ignored by all authenticators.
        let ctx = run(hm(&[("authorization", b"Bearer xyz")])).await.unwrap();
        assert!(ctx.method.is_none());
        // Device token of the wrong shape: rejected without a DB round trip.
        let e = run(hm(&[("authorization", b"Bearer prt_short")])).await.unwrap_err();
        assert_eq!(e.detail(), "Invalid token.");
        assert_eq!(e.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remote_user_api_has_no_challenge() {
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid").unwrap();
        let mut c = AuthConfig::for_tests("s");
        c.remote_user_api = true;
        let a = Authz::new(pool, c);
        assert_eq!(a.authenticate_header(&HeaderMap::new()), None);
    }

    #[test]
    fn delete_cookie() {
        let s = SessionState { accessed: true, cookie_sent: true, empty: true, flushed: false };
        assert_eq!(
            s.delete_cookie_header(&AuthConfig::for_tests("x")).as_deref(),
            Some("sessionid=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=Lax")
        );
    }
}
