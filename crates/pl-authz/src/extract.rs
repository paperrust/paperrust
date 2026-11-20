//! Axum extractors: [`MaybeUser`] (DRF authentication, AllowAny) and [`AuthUser`]
//! (authentication + `IsAuthenticated`).
//!
//! upstream: rest_framework/views.py@3.16 (APIView.initial → perform_authentication →
//!           check_permissions), rest_framework/permissions.py@3.16 (IsAuthenticated).
//!
//! Requirements: the server puts an [`Authz`] into the request extensions
//! (`router.layer(axum::Extension(authz))`). The computed [`AuthContext`] is cached in the
//! extensions, so several extractors per request authenticate only once.
//!
//! DRF order inside `initial()`: content negotiation, **API version** (406), authentication,
//! permissions, throttling. Put the version extractor before `AuthUser`/`MaybeUser`.

use crate::authn::{AuthContext, Authz};
use crate::error::AuthError;
use crate::user::User;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::ops::Deref;
use std::sync::Arc;

async fn context(parts: &mut Parts) -> Result<Arc<AuthContext>, AuthError> {
    if let Some(ctx) = parts.extensions.get::<Arc<AuthContext>>() {
        return Ok(ctx.clone());
    }
    let authz = parts
        .extensions
        .get::<Authz>()
        .cloned()
        .ok_or_else(|| AuthError::internal("pl_authz::Authz missing in request extensions"))?;
    let meta = crate::authn::RequestMeta {
        remote_addr: parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip()),
        csrf_form_token: parts.extensions.get::<crate::csrf::FormCsrfToken>().map(|t| t.0.clone()),
    };
    let ctx = Arc::new(authz.authenticate_with(&parts.method, parts.uri.path(), &parts.headers, &meta).await?);
    parts.extensions.insert(ctx.clone());
    if let Some(notes) = parts.extensions.get::<pl_core::RequestNotes>() {
        note_context(notes, &ctx, &authz);
    }
    Ok(ctx)
}

/// Tell pl-server what Django's middleware would do with this request:
/// `ApiVersionMiddleware` (authenticated) and `SessionMiddleware` (delete an empty session
/// cookie). `Vary: Cookie` is added by pl-server on every response anyway.
fn note_context(notes: &pl_core::RequestNotes, ctx: &AuthContext, authz: &Authz) {
    if ctx.user.is_some() {
        notes.mark_authenticated();
    }
    if let Some(cookie) = ctx.session.delete_cookie_header(authz.config())
        && let Ok(v) = axum::http::HeaderValue::from_str(&cookie)
    {
        notes.add_set_cookie(v);
    }
}

/// Authentication without `IsAuthenticated` (views with `permission_classes = []` or that
/// check permissions themselves). Invalid credentials still fail (401/403) like DRF.
#[derive(Debug, Clone)]
pub struct MaybeUser(pub Arc<AuthContext>);

impl MaybeUser {
    pub fn user(&self) -> Option<&User> {
        self.0.user()
    }
}

impl Deref for MaybeUser {
    type Target = AuthContext;
    fn deref(&self) -> &AuthContext {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for MaybeUser {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(context(parts).await?))
    }
}

/// Authentication + `IsAuthenticated`. Derefs to the [`AuthContext`]; the user is
/// [`AuthUser::user`]. Anonymous → `NotAuthenticated` (401 `WWW-Authenticate: Token` or
/// `Basic realm="api"`, 403 in remote-user API mode).
#[derive(Debug, Clone)]
pub struct AuthUser {
    ctx: Arc<AuthContext>,
    user: Arc<User>,
}

impl AuthUser {
    /// The authenticated user.
    pub fn user(&self) -> &User {
        &self.user
    }

    /// The full authentication context (method, token, session flags, ...).
    pub fn context(&self) -> &AuthContext {
        &self.ctx
    }

    /// Build from a context (e.g. in tests); `IsAuthenticated` semantics.
    pub fn from_context(ctx: Arc<AuthContext>) -> Result<Self, AuthError> {
        match ctx.user.clone() {
            Some(user) => Ok(Self { ctx, user }),
            None => Err(ctx.deny()),
        }
    }
}

impl Deref for AuthUser {
    type Target = AuthContext;
    fn deref(&self) -> &AuthContext {
        &self.ctx
    }
}

impl AsRef<User> for AuthUser {
    fn as_ref(&self) -> &User {
        &self.user
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        AuthUser::from_context(context(parts).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt as _;

    async fn oneshot(app: axum::Router, req: Request<Body>) -> axum::response::Response {
        match app.oneshot(req).await {
            Ok(r) => r,
            Err(e) => match e {},
        }
    }

    #[tokio::test]
    async fn extractor_rejects_anonymous() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")
            .unwrap();
        let authz = Authz::new(pool, AuthConfig::for_tests("s"));
        let app = axum::Router::new()
            .route("/needs", get(|u: AuthUser| async move { u.user().username.clone() }))
            .route("/maybe", get(|u: MaybeUser| async move { format!("{}", u.user().is_some()) }))
            .layer(axum::Extension(authz));
        let r = oneshot(app.clone(), Request::get("/needs").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers().get("www-authenticate").unwrap(), "Token");
        let body = axum::body::to_bytes(r.into_body(), 10_000).await.unwrap();
        assert_eq!(&body[..], br#"{"detail":"Authentication credentials were not provided."}"#);
        let r = oneshot(app, Request::get("/maybe").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
    }
}
