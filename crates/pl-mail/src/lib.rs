//! pl-mail – see docs/ARCHITECTURE.md (module layout) and docs/contract/pl-mail.md (contract).
//!
//! upstream: src/paperless_mail/@v3.4.2 (views.py, serialisers.py, models.py, filters.py, mail.py, oauth.py).
//! `/api/oauth/callback/`: [`oauth_callback`] (Standalone, docs/STANDALONE-GAPS.md).

pub mod accounts;
pub mod body;
pub mod celery;
pub mod ctx;
pub mod documents_email;
pub mod drf;
pub mod effects;
pub mod error;
pub mod fetch;
pub mod imap;
pub mod listing;
pub mod oauth;
pub mod oauth_callback;
pub mod perms;
pub mod processed;
pub mod pyval;
pub mod rules;
pub mod util;

use pl_core::ApiRoute;

pub use ctx::{AuthUserId, MailConfig, MailUser};

/// All routes of this module. The server only mounts those enabled in `routes.toml`;
/// everything else goes to Django via proxy.
pub fn routes() -> Vec<ApiRoute> {
    let mut r = accounts::routes();
    r.extend(rules::routes());
    r.extend(processed::routes());
    r.extend(documents_email::routes());
    r.extend(oauth_callback::routes());
    r
}
