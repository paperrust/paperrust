//! pl-api-v2 – native API v2 unter `/rs/api/v2/` (docs/ARCHITECTURE-V2.md §3, docs/SYNC.md §3, §8).
//!
//! * own shapes (UUID + `legacy_id` + `rev`), errors `{"error":{…}}`, cursor pagination
//!   `{"items":[…],"next_cursor":…}`, timestamps RFC 3339 UTC;
//! * never depends on v1 serializers (`pl-documents`) or `pl_core::pagination`; all data and
//!   permissions come via the `pl-domain` traits ([`pl_domain::Repo`], [`pl_domain::ChangeFeed`]);
//! * login shared with v1 (`pl-authz`: session+CSRF, `Token`, device token `Bearer prt_…`).
//!
//! Mounting: `pl-server` nests [`router`] under [`PREFIX`] before the Django fallback
//! (`PAPERRUST_API_V2=off` switches it off). Contract: docs/contract/pl-api-v2.md, OpenAPI:
//! docs/api/v2.yaml.

/// Questions to the archive `/ask…` (PaperRust UI, retrieval like MCP `ask_documents`).
#[cfg(feature = "ai")]
pub mod ask;
/// ASN label generator `/asn/…` (pl-labels).
pub mod asn;
/// Shared parts of the PaperRust UI (design 2).
pub mod d2;
pub mod devices;
pub mod error;
pub mod files;
/// Folder view `/folders…` (PaperRust only).
pub mod folders;
pub mod handlers;
pub mod http;
/// Life areas `/life-areas…` (PaperRust UI).
pub mod life_areas;
/// Rollen `/roles…` (docs/DESIGN2-ENTWIRREN.md §1).
pub mod roles;
/// System settings `/settings` (docs/SETTINGS.md §3, WP5).
pub mod settings;
pub mod settings_logo;
/// Migration helper, export/import (docs/SETTINGS.md §2.7, §5, WP8).
pub mod settings_migration;
pub mod sync;
/// Timeline `/timeline…` (PaperRust UI, pl-timeline).
pub mod timeline;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::routing::{get, post};
use axum::{Extension, Router};
use pl_core::AppState;
use pl_domain::model::ObjectWrite;
use pl_domain::{ChangeFeed, DomainError, Kind, Repo};

use crate::error::V2Error;
use crate::http::{Caller, V2Json};

/// Path prefix of API v2.
pub const PREFIX: &str = "/rs/api/v2";

/// Services of API v2 (attached as a request extension).
#[derive(Clone)]
pub struct V2 {
    pub repo: Arc<dyn Repo>,
    /// Change feed (P2); `None` → `/sync/changes` answers 501.
    pub feed: Option<Arc<dyn ChangeFeed>>,
    /// Internal router with the v1 read handlers `documents.list`/`documents.retrieve` (from
    /// `pl-server`) for `/ask`; `None` → `/ask` answers 501.
    pub legacy_read: Option<axum::Router>,
    /// Tests only: replace the LLM index for `/ask` (`None` = existing LLM index).
    #[cfg(feature = "ai")]
    pub ask_knowledge: Option<Arc<dyn pl_ai::mcp::knowledge::KnowledgeIndex>>,
}

impl V2 {
    pub fn new(repo: Arc<dyn Repo>, feed: Option<Arc<dyn ChangeFeed>>, legacy_read: Option<axum::Router>) -> Self {
        V2 {
            repo,
            feed,
            legacy_read,
            #[cfg(feature = "ai")]
            ask_knowledge: None,
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for V2 {
    type Rejection = V2Error;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<V2>().cloned().ok_or_else(|| V2Error(DomainError::Internal("api v2 services missing".into())))
    }
}

/// Kinds with their own list under `/rs/api/v2/<plural>`.
pub const LIST_KINDS: [Kind; 8] =
    [Kind::Tag, Kind::Correspondent, Kind::DocumentType, Kind::StoragePath, Kind::CustomField, Kind::SavedView, Kind::User, Kind::Group];

/// Kinds that v2 can write (transitionally via the v1 handlers). Not storage paths: their
/// v1 write routes still run via Django (template validation, routes.toml).
pub const WRITE_KINDS: [Kind; 3] = [Kind::Tag, Kind::Correspondent, Kind::DocumentType];

/// Router (relative to [`PREFIX`]); needs `pl_authz::Authz` as an extension (set by `pl-server`).
pub fn router(v2: V2) -> Router<AppState> {
    let mut r: Router<AppState> = Router::new()
        .route("/", get(handlers::server))
        .route("/server", get(handlers::server))
        .route("/me", get(handlers::me))
        .route("/ui_settings", get(handlers::get_ui_settings).put(handlers::put_ui_settings))
        .route("/documents", get(handlers::list_documents))
        .route("/documents/query", post(handlers::query_documents))
        .route("/documents/{uuid}", get(handlers::get_document).patch(handlers::patch_document))
        .route("/documents/{uuid}/notes", get(handlers::notes).post(handlers::add_note))
        .route("/documents/{uuid}/notes/{note}", axum::routing::delete(handlers::delete_note))
        .route("/documents/{uuid}/files/{variant}", get(handlers::file))
        .route("/objects/batch", post(handlers::batch))
        .route("/sync/changes", get(sync::changes))
        .route("/sync/ids", get(sync::ids))
        .route("/sync/resolve", post(sync::resolve))
        .route("/auth/devices", get(devices::list).post(devices::create))
        .route("/auth/devices/{uuid}", axum::routing::delete(devices::revoke));
    r = settings::routes(r);
    r = asn::routes(r);
    r = life_areas::routes(r);
    r = folders::routes(r);
    r = timeline::routes(r);
    r = roles::routes(r);
    #[cfg(feature = "ai")]
    {
        r = ask::routes(r);
    }
    #[cfg(not(feature = "ai"))]
    {
        r = r.route("/ask", post(ask_disabled));
    }
    for kind in LIST_KINDS {
        let list = get(move |v2: V2, c: Caller, Query(q): Query<HashMap<String, String>>| handlers::list_objects(v2, kind, c, q));
        let one = get(move |v2: V2, c: Caller, Path(u): Path<String>, Query(q): Query<HashMap<String, String>>| {
            handlers::get_object(v2, kind, c, u, q)
        });
        let (list, one) = if WRITE_KINDS.contains(&kind) {
            (
                list.post(move |v2: V2, c: Caller, V2Json(w): V2Json<ObjectWrite>| handlers::create_object(v2, kind, c, w)),
                one.patch(move |v2: V2, c: Caller, Path(u): Path<String>, h: HeaderMap, V2Json(w): V2Json<ObjectWrite>| {
                    handlers::update_object(v2, kind, c, u, h, w)
                })
                .delete(move |v2: V2, c: Caller, Path(u): Path<String>, h: HeaderMap| handlers::delete_object(v2, kind, c, u, h)),
            )
        } else {
            (list, one)
        };
        r = r.route(&format!("/{}", kind.plural()), list).route(&format!("/{}/{{uuid}}", kind.plural()), one);
    }
    r.fallback(handlers::not_found).method_not_allowed_fallback(method_not_allowed).layer(Extension(v2))
}

#[cfg(not(feature = "ai"))]
async fn ask_disabled() -> axum::response::Response {
    use axum::response::IntoResponse as _;
    V2Error(DomainError::NotImplemented("built without AI (feature `ai`)".into())).into_response()
}

async fn method_not_allowed() -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let body = serde_json::json!({"error": {"code": "method_not_allowed", "message": "Method not allowed."}});
    (axum::http::StatusCode::METHOD_NOT_ALLOWED, axum::Json(body)).into_response()
}
