//! `request.user`, `authenticate`, `login`, `logout` as in Django/allauth/paperless.
//!
//! upstream: django/contrib/auth/__init__.py@5.2 (`get_user`, `authenticate`, `login`, `logout`,
//! `user_logged_in` → `update_last_login`), django/contrib/auth/backends.py (`ModelBackend`,
//! `RemoteUserBackend`), django/contrib/auth/middleware.py (`PersistentRemoteUserMiddleware`),
//! allauth 65.20.1 account/auth_backends.py (`AuthenticationBackend`: username case-insensitive,
//! "stash" of correct credentials of inactive users), src/paperless/auth.py@v3.4.2
//! (`AutoLoginMiddleware`, `HttpRemoteUserMiddleware`), src/paperless/signals.py
//! (`handle_failed_login`).

use pl_authz::hashers;
use pl_authz::user::{self, User};
use serde_json::Value;
use sqlx::PgPool;

use crate::session::{BACKEND_SESSION_KEY, HASH_SESSION_KEY, SESSION_KEY};
use crate::web::{Ctx, R};

pub const MODEL_BACKEND: &str = "django.contrib.auth.backends.ModelBackend";
pub const ALLAUTH_BACKEND: &str = "allauth.account.auth_backends.AuthenticationBackend";
pub const REMOTE_USER_BACKEND: &str = "django.contrib.auth.backends.RemoteUserBackend";

/// Result of `adapter.authenticate`.
pub enum Authenticated {
    /// Active user + backend path (`user.backend`).
    Active(User, &'static str),
    /// Correct password, but inactive (allauth stash).
    Inactive(User),
    None,
}

/// `user.check_password(raw)` with setter (`AbstractBaseUser.check_password.setter`):
/// `set_password(raw)` + `save(update_fields=["password"])` – **also on the object itself**.
/// Afterwards Django computes `_auth_user_hash` in `login()` from the *new* hash; if
/// `u.password` stayed old, the session hash would not match the DB after a rehash (e.g. salt with < 128 bit
/// entropy) and the next request would discard the session (flush → 401).
async fn check_password_with_setter(db: &PgPool, u: &mut User, password: &str) -> R<bool> {
    let (pw, enc) = (password.to_string(), u.password.clone());
    let v = tokio::task::spawn_blocking(move || hashers::verify_password(&pw, &enc)).await?;
    if v.is_correct && v.must_update {
        let pw = password.to_string();
        let new = tokio::task::spawn_blocking(move || hashers::make_password(&pw)).await?;
        user::update_password(db, u.id, &new).await?;
        u.password = new;
    }
    Ok(v.is_correct)
}

async fn dummy(password: &str) {
    let pw = password.to_string();
    let _ = tokio::task::spawn_blocking(move || hashers::dummy_hash(&pw)).await;
}

/// `django.contrib.auth.authenticate(username=, password=)` with paperless' backends
/// (guardian without effect, ModelBackend, allauth) + allauth stash for inactive users.
/// `client`: request data for the failed-login log (client IP as in ipware).
pub async fn authenticate(db: &PgPool, username: &str, password: &str, client: &FailedLoginClient<'_>) -> R<Authenticated> {
    // ModelBackend
    match user::load_user_by_username(db, username).await? {
        None => dummy(password).await,
        Some(mut u) => {
            if check_password_with_setter(db, &mut u, password).await? && u.is_active {
                return Ok(Authenticated::Active(u, MODEL_BACKEND));
            }
        }
    }
    // allauth AuthenticationBackend (only with password)
    if password.is_empty() {
        client.log(username);
        return Ok(Authenticated::None);
    }
    let mut stashed = None;
    match user::load_user_by_username_iexact(db, username).await? {
        None => dummy(password).await,
        Some(mut u) => {
            if check_password_with_setter(db, &mut u, password).await? {
                if u.is_active {
                    return Ok(Authenticated::Active(u, ALLAUTH_BACKEND));
                }
                stashed = Some(u);
            }
        }
    }
    client.log(username);
    Ok(match stashed {
        Some(u) => Authenticated::Inactive(u),
        None => Authenticated::None,
    })
}

/// Request data for `handle_failed_login` (`IpWare(proxy_list=TRUSTED_PROXIES)`).
pub struct FailedLoginClient<'a> {
    pub headers: &'a axum::http::HeaderMap,
    /// `REMOTE_ADDR` (TCP peer).
    pub remote_addr: Option<std::net::IpAddr>,
    pub trusted_proxies: &'a [String],
}

impl FailedLoginClient<'_> {
    /// From the page context.
    pub fn of(ctx: &Ctx) -> FailedLoginClient<'_> {
        FailedLoginClient {
            headers: &ctx.headers,
            remote_addr: ctx.remote_addr.as_deref().and_then(|a| a.parse().ok()),
            trusted_proxies: &ctx.s.auth.trusted_proxies,
        }
    }

    /// `paperless.signals.handle_failed_login`.
    pub fn log(&self, username: &str) {
        pl_authz::clientip::log_failed_login(Some(username), self.headers, self.remote_addr, self.trusted_proxies);
    }
}

/// `AuthenticationMiddleware` → `get_user(request)` from the session (hash check, flush on
/// mismatch), then `AutoLoginMiddleware` and `HttpRemoteUserMiddleware`.
pub async fn resolve_user(ctx: &mut Ctx) -> R<()> {
    ctx.user = session_user(ctx).await?;
    // AutoLoginMiddleware (runs after AuthenticationMiddleware, calls auth.login every time)
    if let Some(name) = ctx.s.auto_login_username.clone()
        && let Some(u) = user::load_user_by_username(ctx.st.db(), &name).await?
        && u.is_active
    {
        login(ctx, u, MODEL_BACKEND).await?;
    }
    // HttpRemoteUserMiddleware (PersistentRemoteUserMiddleware)
    if ctx.s.enable_http_remote_user {
        let meta = ctx.s.http_remote_user_header_name.clone();
        if let Some(remote) = pl_authz::csrf::meta_header(&ctx.headers, &meta) {
            let same = ctx.user.as_ref().is_some_and(|u| u.username == remote);
            if !same {
                if ctx.user.is_some() && ctx.session.get(BACKEND_SESSION_KEY).and_then(Value::as_str) == Some(REMOTE_USER_BACKEND) {
                    logout(ctx);
                }
                if !remote.is_empty()
                    && let Some(u) = user::get_or_create_user(ctx.st.db(), &remote).await?
                    && u.is_active
                {
                    login(ctx, u, REMOTE_USER_BACKEND).await?;
                }
            }
        }
    }
    Ok(())
}

async fn session_user(ctx: &mut Ctx) -> R<Option<User>> {
    let Some(id) = ctx.session.user_id() else { return Ok(None) };
    let backend = ctx.session.get(BACKEND_SESSION_KEY).and_then(Value::as_str).unwrap_or("").to_string();
    let allowed =
        backend == MODEL_BACKEND || backend == ALLAUTH_BACKEND || (backend == REMOTE_USER_BACKEND && ctx.s.has_remote_user_backend());
    if !allowed {
        return Ok(None);
    }
    let Some(u) = user::load_user_by_id(ctx.st.db(), id).await? else { return Ok(None) };
    if !u.is_active {
        return Ok(None);
    }
    let stored = ctx.session.get(HASH_SESSION_KEY).and_then(Value::as_str).unwrap_or("");
    let ok =
        !stored.is_empty() && pl_authz::signing::constant_time_eq(stored.as_bytes(), u.session_auth_hash(&ctx.s.secret_key).as_bytes());
    if !ok {
        ctx.session.flush();
        return Ok(None);
    }
    Ok(Some(u))
}

/// `django.contrib.auth.login(request, user, backend)`: rotate or clear the session,
/// set keys, rotate CSRF, update `last_login`.
pub async fn login(ctx: &mut Ctx, u: User, backend: &str) -> R<()> {
    let hash = u.session_auth_hash(&ctx.s.secret_key);
    if ctx.session.get(SESSION_KEY).is_some() {
        let same_user = ctx.session.user_id() == Some(u.id);
        let same_hash = ctx
            .session
            .get(HASH_SESSION_KEY)
            .and_then(Value::as_str)
            .is_some_and(|h| pl_authz::signing::constant_time_eq(h.as_bytes(), hash.as_bytes()));
        if !same_user || !same_hash {
            ctx.session.flush();
        }
    } else {
        ctx.session.cycle_key();
    }
    ctx.session.set(SESSION_KEY, Value::from(u.id.to_string()));
    ctx.session.set(BACKEND_SESSION_KEY, Value::from(backend));
    ctx.session.set(HASH_SESSION_KEY, Value::from(hash));
    ctx.rotate_csrf();
    // user_logged_in → update_last_login
    sqlx::query("UPDATE auth_user SET last_login = now() WHERE id = $1").bind(u.id).execute(ctx.st.db()).await?;
    ctx.user = Some(u);
    Ok(())
}

/// `django.contrib.auth.logout(request)`.
pub fn logout(ctx: &mut Ctx) {
    ctx.session.flush();
    ctx.user = None;
}
