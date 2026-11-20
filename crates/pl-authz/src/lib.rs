//! pl-authz – authentication, CSRF, passwords, model and object permissions of paperless-ngx.
//!
//! Contract and behaviour: `docs/contract/pl-authz.md`. Architecture: `docs/ARCHITECTURE.md`.
//!
//! Quick start for handlers:
//! ```ignore
//! use pl_authz::{AuthUser, MaybeUser, models, objects, owned};
//!
//! // permission_classes = (IsAuthenticated, PaperlessObjectPermissions)
//! async fn list_tags(auth: AuthUser, /* ... */) -> Result<_, ApiError> {
//!     auth.require_model_perms(&models::TAG)?;
//!     let cond = objects::permitted_objects_filter(Some(&auth), &models::TAG, "documents_tag",
//!                                                  &objects::FilterOpts::default());
//!     // query.cond_where(cond) ... then owned::OwnedInfo::load(...) for the page
//! }
//! ```

pub mod authn;
pub mod basic_throttle;
pub mod clientip;
pub mod config;
mod core_bridge;
pub mod csrf;
pub mod device;
pub mod error;
pub mod extract;
pub mod grants;
pub mod hashers;
pub mod models;
pub mod objects;
pub mod owned;
pub mod perms;
pub mod signing;
pub mod user;

pub use authn::{AuthContext, AuthMethod, Authz, RequestMeta, SessionState};
pub use config::AuthConfig;
pub use error::AuthError;
pub use extract::{AuthUser, MaybeUser};
pub use models::ObjectModel;
pub use user::User;

use pl_core::ApiRoute;

/// pl-authz has no routes of its own (`/api/token/`, `/api/profile/` live in pl-auth-api).
pub fn routes() -> Vec<ApiRoute> {
    Vec::new()
}
