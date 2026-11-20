//! pl-store – implementation of the `pl-domain` interfaces on the Django tables of paperless-ngx
//! v3.4.2 and the identity table `paperrust_sync_object` (docs/SYNC.md §2).
//!
//! * Reading: own SQL queries, visibility and permissions via `pl-authz` (the same decision
//!   as v1), document filter transitionally via `pl_docfilter` (ARCHITECTURE-V2 §5 L1).
//! * Writing: transitionally via the **v1 handlers** (parity-checked: validation, audit log,
//!   workflows, index, file names), which `pl-server` mounts as [`LegacyRouter`]. The v2 contract
//!   knows nothing of this; the v1 JSON body is built and read only here.
//! * Identity: `uuid`/`rev` from `paperrust_sync_object` (P2). If a row is missing there (e.g.
//!   capture switched off), `pl-store` creates it (`ON CONFLICT DO NOTHING`); if the
//!   table is missing, the object endpoints answer `503 not_installed`.
//!
//! Contract: docs/contract/pl-api-v2.md (section "pl-store").

mod auth;
pub mod datamodel;
pub mod docsql;
mod documents;
mod files;
mod ident;
mod legacy;
mod objects;
mod perm;
mod rules;
mod sync;
pub mod textcache;

use std::sync::Arc;

use pl_domain::model::*;
use pl_domain::query::DocQuery;
use pl_domain::{BoxFut, DomainError, Kind, Principal, Repo};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

pub use auth::{AuthExt, principal};

/// Own migrations (expand-only, via `pl_cli::migrate::paperrust_migrations`).
pub mod migrations {
    /// L1: partial index of the document list (docs/contract/pl-docfilter.md "L1").
    pub const DOCQUERY_INDEXES: (&str, &str) = (
        "pl_store.0001_paperrust_docquery_indexes",
        include_str!("../migrations/0001_paperrust_docquery_indexes.sql"),
    );
    /// L3: trigram indexes for substring search over title/content (docs/contract/pl-docfilter.md
    /// "L3: trigram indexes").
    pub const DOC_TRGM: (&str, &str) = (
        "pl_store.0002_paperrust_doc_trgm",
        include_str!("../migrations/0002_paperrust_doc_trgm.sql"),
    );
    /// Selection data over large selections: partial index with the FK columns (docs/PERF-SCALE.md §12).
    pub const DOC_LIVE_FK: (&str, &str) = (
        "pl_store.0003_paperrust_doc_live_fk",
        include_str!("../migrations/0003_paperrust_doc_live_fk.sql"),
    );
    pub const ALL: &[(&str, &str)] = &[DOCQUERY_INDEXES, DOC_TRGM, DOC_LIVE_FK];
    /// Selection data "all" from the counters: deleted field values (docs/PERF-SCALE.md §13). Runs
    /// **after** the conversion stages ([`AFTER_STAGES`]), because from stage 3 the index belongs on
    /// `paperrust_field_value`.
    pub const FIELD_VALUE_DELETED: (&str, &str) = (
        "pl_store.0004_paperrust_field_value_deleted",
        include_str!("../migrations/0004_paperrust_field_value_deleted.sql"),
    );
    /// Indexes on tables of the conversion stages (after `pl_store::datamodel::migrations()`).
    pub const AFTER_STAGES: &[(&str, &str)] = &[FIELD_VALUE_DELETED];
}
pub use legacy::LegacyRouter;

/// Repository over PostgreSQL.
#[derive(Clone)]
pub struct Store {
    pub(crate) db: PgPool,
    pub(crate) cfg: Arc<pl_core::Config>,
    pub(crate) legacy: Option<LegacyRouter>,
}

impl Store {
    /// `legacy` = router with the v1 write handlers (from `pl-server`); without it
    /// write accesses answer `501 not_implemented`.
    pub fn new(db: PgPool, cfg: Arc<pl_core::Config>, legacy: Option<LegacyRouter>) -> Self {
        Store { db, cfg, legacy }
    }

    pub fn db(&self) -> &PgPool {
        &self.db
    }
}

/// Visible ids of a kind for `p` as an SQL subquery with one column (for side-table
/// queries like the timeline, pl-timeline): documents **exactly like the document list**
/// (root documents outside the trash + `permitted_objects_filter`), other kinds via
/// their object permissions. If the model permission `view_<kind>` is missing (or the user is inactive), the
/// set is empty. The values are inlined into the SQL (sea-query), no placeholders.
pub fn visible_ids_sql(p: &Principal, kind: Kind) -> Result<String, DomainError> {
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    if perm::require(user, spec.view_perm).is_err() {
        return Ok("SELECT NULL::int4 WHERE false".into());
    }
    Ok(sync::visible_sql(user, kind))
}

pub(crate) fn db_err(e: sqlx::Error) -> DomainError {
    if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("42P01") {
        return DomainError::NotInstalled("sync capture (paperrust_sync_object, `paperrust manage migrate`)");
    }
    DomainError::Internal(e.to_string())
}

pub(crate) fn a(s: &str) -> sea_query::Alias {
    sea_query::Alias::new(s)
}

impl Repo for Store {
    fn me<'a>(&'a self, p: &'a Principal) -> BoxFut<'a, Me> {
        Box::pin(objects::me(self, p))
    }

    fn list<'a>(&'a self, p: &'a Principal, kind: Kind, req: &'a ListRequest) -> BoxFut<'a, Page<Object>> {
        Box::pin(objects::list(self, p, kind, req))
    }

    fn get<'a>(&'a self, p: &'a Principal, kind: Kind, uuid: Uuid, sharing: bool) -> BoxFut<'a, Object> {
        Box::pin(async move {
            if kind == Kind::Document {
                let inc = DocInclude { sharing, ..Default::default() };
                return documents::get(self, p, uuid, inc).await.map(|d| Object::Document(Box::new(d)));
            }
            objects::get(self, p, kind, uuid, sharing).await
        })
    }

    fn batch<'a>(
        &'a self,
        p: &'a Principal,
        kind: Kind,
        uuids: &'a [Uuid],
        include: DocInclude,
    ) -> BoxFut<'a, (Vec<Object>, Vec<Uuid>)> {
        Box::pin(async move {
            if kind == Kind::Document {
                return documents::batch(self, p, uuids, include).await;
            }
            objects::batch(self, p, kind, uuids).await
        })
    }

    fn by_legacy_ids<'a>(
        &'a self,
        p: &'a Principal,
        kind: Kind,
        ids: &'a [i64],
        embed: bool,
    ) -> BoxFut<'a, Vec<(i64, Option<Object>)>> {
        Box::pin(async move {
            if kind == Kind::Document {
                return documents::by_legacy_ids(self, p, ids, embed).await;
            }
            objects::by_legacy_ids(self, p, kind, ids).await
        })
    }

    fn query_documents<'a>(&'a self, p: &'a Principal, q: &'a DocQuery) -> BoxFut<'a, Page<Document>> {
        Box::pin(documents::query(self, p, q))
    }

    fn get_document<'a>(&'a self, p: &'a Principal, uuid: Uuid, include: DocInclude) -> BoxFut<'a, Document> {
        Box::pin(documents::get(self, p, uuid, include))
    }

    fn notes<'a>(&'a self, p: &'a Principal, doc: Uuid) -> BoxFut<'a, Vec<Note>> {
        Box::pin(documents::notes(self, p, doc))
    }

    fn add_note<'a>(&'a self, p: &'a Principal, doc: Uuid, text: &'a str) -> BoxFut<'a, Vec<Note>> {
        Box::pin(legacy::add_note(self, p, doc, text))
    }

    fn delete_note<'a>(&'a self, p: &'a Principal, doc: Uuid, note: i64) -> BoxFut<'a, Vec<Note>> {
        Box::pin(legacy::delete_note(self, p, doc, note))
    }

    fn create<'a>(&'a self, p: &'a Principal, kind: Kind, w: &'a ObjectWrite) -> BoxFut<'a, Object> {
        Box::pin(legacy::create(self, p, kind, w))
    }

    fn update<'a>(
        &'a self,
        p: &'a Principal,
        kind: Kind,
        uuid: Uuid,
        if_match: Option<i64>,
        w: &'a ObjectWrite,
    ) -> BoxFut<'a, Object> {
        Box::pin(legacy::update(self, p, kind, uuid, if_match, w))
    }

    fn delete<'a>(&'a self, p: &'a Principal, kind: Kind, uuid: Uuid, if_match: Option<i64>) -> BoxFut<'a, ()> {
        Box::pin(legacy::delete(self, p, kind, uuid, if_match))
    }

    fn patch_document<'a>(
        &'a self,
        p: &'a Principal,
        uuid: Uuid,
        if_match: Option<i64>,
        patch: &'a DocumentPatch,
    ) -> BoxFut<'a, Document> {
        Box::pin(legacy::patch_document(self, p, uuid, if_match, patch))
    }

    fn file<'a>(&'a self, p: &'a Principal, uuid: Uuid, variant: FileVariant) -> BoxFut<'a, FileRef> {
        Box::pin(files::file(self, p, uuid, variant))
    }

    fn ids<'a>(&'a self, p: &'a Principal, kind: Kind) -> BoxFut<'a, Vec<(Uuid, i64)>> {
        Box::pin(sync::ids(self, p, kind))
    }

    fn resolve<'a>(&'a self, p: &'a Principal, sel: &'a Selection) -> BoxFut<'a, Vec<ResolvedDoc>> {
        Box::pin(sync::resolve(self, p, sel))
    }

    fn ui_settings<'a>(&'a self, p: &'a Principal) -> BoxFut<'a, Value> {
        Box::pin(objects::ui_settings(self, p))
    }

    fn set_ui_settings<'a>(&'a self, p: &'a Principal, settings: &'a Value) -> BoxFut<'a, Value> {
        Box::pin(legacy::set_ui_settings(self, p, settings))
    }
}
