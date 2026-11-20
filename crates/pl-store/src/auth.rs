//! Login context ↔ `pl_domain::Principal`.

use std::net::SocketAddr;
use std::sync::Arc;

use pl_authz::{AuthContext, User};
use pl_domain::{DomainError, DomainResult, Principal};

/// Content of `Principal::ext` in this implementation.
#[derive(Debug, Clone)]
pub struct AuthExt {
    pub ctx: Arc<AuthContext>,
    /// Client address (audit log of the v1 write paths).
    pub peer: Option<SocketAddr>,
}

/// `Principal` from an authenticated context (`None` = anonymous).
pub fn principal(ctx: Arc<AuthContext>, peer: Option<SocketAddr>) -> Option<Principal> {
    let u = ctx.user.clone()?;
    Some(Principal {
        user_id: u.id,
        username: u.username.clone(),
        is_superuser: u.is_superuser,
        ext: Arc::new(AuthExt { ctx, peer }),
    })
}

pub(crate) fn ext(p: &Principal) -> DomainResult<&AuthExt> {
    p.ext
        .downcast_ref::<AuthExt>()
        .ok_or_else(|| DomainError::Internal("principal without pl-store AuthExt".into()))
}

pub(crate) fn user(p: &Principal) -> DomainResult<&User> {
    ext(p)?.ctx.user().ok_or(DomainError::Unauthenticated)
}
