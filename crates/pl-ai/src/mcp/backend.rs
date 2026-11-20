//! Data access of the MCP server. Implementations: [`super::memory::MemoryBackend`]
//! (tests, without DB) and `crate::pg::PgBackend` (PostgreSQL, Django schema, permissions via pl-authz).
//!
//! Every method receives the logged-in [`Actor`] and must filter **itself**: return only visible
//! objects, perform only permitted changes. The server does not trust any ids from the
//! request.

use std::collections::BTreeSet;
use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::taxonomy::{Alias, Principal, TaxonomyEntry, TaxonomyKind};

/// Logged-in user (from the API token).
#[derive(Debug, Clone, PartialEq)]
pub struct Actor {
    pub principal: Principal,
    pub username: String,
    /// Global model permissions `app_label.codename` (superuser: all).
    pub perms: BTreeSet<String>,
    /// First 4 characters of the token + "…" (for audit/logs; never the token itself).
    pub token_hint: String,
    /// Stable key for the rate limit (hash of the token).
    pub rate_key: String,
    /// The token itself (only for calls on behalf of the user; never logged).
    pub credential: Credential,
}

/// Secret with redacted `Debug` output.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Credential(String);

impl Credential {
    pub fn new(s: &str) -> Self {
        Self(s.to_string())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential(***)")
    }
}

impl Actor {
    pub fn user_id(&self) -> Option<i64> {
        self.principal.user_id
    }

    /// `user.has_perm("documents.<codename>")` (active superusers have everything).
    pub fn has_perm(&self, perm: &str) -> bool {
        if !self.principal.is_active {
            return false;
        }
        self.principal.is_superuser || self.perms.contains(perm)
    }

    /// Model permission for a taxonomy kind, e.g. `documents.add_tag`.
    pub fn has_kind_perm(&self, action: &str, kind: TaxonomyKind) -> bool {
        self.has_perm(&format!("documents.{action}_{}", kind.model_name()))
    }
}

/// Short info of a document (search result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub id: i64,
    pub title: String,
    pub created: Option<String>,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    pub tag_ids: Vec<i64>,
    pub archive_serial_number: Option<i64>,
    /// Time of addition (ISO), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
}

/// Document with content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub summary: DocumentSummary,
    pub original_filename: Option<String>,
    pub owner_id: Option<i64>,
    pub content: String,
    /// Custom field values: (field_id, value as JSON).
    pub custom_fields: Vec<(i64, Value)>,
    pub notes_count: i64,
}

/// Search parameters.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchParams {
    pub query: Option<String>,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Sorting like `?ordering=` (`-added`, `-created`, …); not settable by the agent.
    #[serde(default, skip_deserializing)]
    pub ordering: Option<String>,
}

/// Change to a document (metadata only; no owner/permission change, no deletion).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct DocumentPatch {
    pub title: Option<String>,
    /// `Some(None)` = remove.
    #[serde(default, with = "double_option")]
    pub correspondent_id: Option<Option<i64>>,
    #[serde(default, with = "double_option")]
    pub document_type_id: Option<Option<i64>>,
    #[serde(default, with = "double_option")]
    pub storage_path_id: Option<Option<i64>>,
    /// `YYYY-MM-DD`.
    pub created: Option<String>,
    #[serde(default)]
    pub add_tag_ids: Vec<i64>,
    #[serde(default)]
    pub remove_tag_ids: Vec<i64>,
    /// (field_id, value); `null` removes the value.
    #[serde(default)]
    pub custom_fields: Vec<(i64, Value)>,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<Option<i64>>, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
        Option::<i64>::deserialize(d).map(Some)
    }
}

impl DocumentPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.correspondent_id.is_none()
            && self.document_type_id.is_none()
            && self.storage_path_id.is_none()
            && self.created.is_none()
            && self.add_tag_ids.is_empty()
            && self.remove_tag_ids.is_empty()
            && self.custom_fields.is_empty()
    }
}

/// Additional details on creation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreateSpec {
    pub kind: TaxonomyKind,
    pub name: String,
    /// Storage path: path template (required); custom field: data_type (required).
    pub extra: Option<String>,
}


/// Audit entry (table `paperrust_ai_audit`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub actor_id: Option<i64>,
    pub channel: &'static str,
    pub token_hint: Option<String>,
    pub run_id: Option<String>,
    pub action: &'static str,
    pub object_kind: String,
    pub object_id: Option<i64>,
    pub forced: bool,
    pub reason: Option<String>,
    pub candidates: Option<Value>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub confidence: Option<f64>,
}

/// A file for normal processing (`POST /api/documents/post_document/`). All ids are
/// already checked (visible to the user).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IngestRequest {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub title: Option<String>,
    /// `YYYY-MM-DD`.
    pub created: Option<String>,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    pub tag_ids: Vec<i64>,
    /// (field_id, value).
    pub custom_fields: Vec<(i64, Value)>,
}

/// A note.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteEntry {
    pub id: i64,
    pub note: String,
    pub created: Option<String>,
    pub user: Option<String>,
}

/// Saved view (only what is needed for the query).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SavedViewInfo {
    pub id: i64,
    pub name: String,
    pub sort_field: Option<String>,
    pub sort_reverse: bool,
    /// `[(rule_type, value)]`.
    pub filter_rules: Vec<(i64, Option<String>)>,
}

/// Backend error (texts go to the agent – no internals).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BackendError {
    #[error("not found")]
    NotFound,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal error")]
    Internal,
}

pub type BResult<T> = Result<T, BackendError>;

/// Data access. All methods must return `Send` futures (axum).
pub trait Backend: Send + Sync + 'static {
    /// API token → user (`authtoken_token`). `None` = invalid.
    fn authenticate(&self, token: &str) -> impl Future<Output = BResult<Option<Actor>>> + Send;
    /// Visible objects of a kind (sorted by name).
    fn list_taxonomy(&self, actor: &Actor, kind: TaxonomyKind) -> impl Future<Output = BResult<Vec<TaxonomyEntry>>> + Send;
    /// Aliases with a visible target.
    fn aliases(&self, actor: &Actor, kind: TaxonomyKind) -> impl Future<Output = BResult<Vec<Alias>>> + Send;
    /// Aliases whose target is in the already loaded list `visible` (review 2 N6: no
    /// second query of the complete taxonomy).
    fn aliases_within(&self, kind: TaxonomyKind, visible: &[TaxonomyEntry]) -> impl Future<Output = BResult<Vec<Alias>>> + Send;
    /// May `actor` edit the object with `action` (`view`/`change`)?
    fn can_object(&self, actor: &Actor, kind: TaxonomyKind, id: i64, action: &str) -> impl Future<Output = BResult<bool>> + Send;
    /// Creates an object (owner = actor). No matching here – the server does that.
    fn create_taxonomy(&self, actor: &Actor, spec: &CreateSpec) -> impl Future<Output = BResult<TaxonomyEntry>> + Send;
    /// Stores an alias (the server does the check).
    fn add_alias(&self, actor: &Actor, alias: &Alias) -> impl Future<Output = BResult<bool>> + Send;
    /// Search in visible documents (without trash).
    fn search_documents(&self, actor: &Actor, p: &SearchParams) -> impl Future<Output = BResult<(i64, Vec<DocumentSummary>)>> + Send;
    /// A visible document.
    fn get_document(&self, actor: &Actor, id: i64) -> impl Future<Output = BResult<Option<DocumentDetail>>> + Send;
    /// May `actor` change the document (`change_document`, owner-aware)?
    fn can_change_document(&self, actor: &Actor, id: i64) -> impl Future<Output = BResult<bool>> + Send;
    /// Applies the change, returns (before, after) as JSON.
    fn update_document(&self, actor: &Actor, id: i64, patch: &DocumentPatch) -> impl Future<Output = BResult<(Value, Value)>> + Send;
    /// Write an audit entry.
    fn audit(&self, entry: &AuditEntry) -> impl Future<Output = BResult<()>> + Send;

    // ---- Extension "second brain" (ingest, knowledge, notes, views, scopes) ----

    /// Hands a file over to normal processing (consumer task, workflows, OCR,
    /// assignment). Returns the task id.
    fn submit_document(&self, actor: &Actor, req: &IngestRequest) -> impl Future<Output = BResult<String>> + Send;
    /// Status of a task (own or visible only, like `GET /api/tasks/?task_id=`).
    fn task_status(&self, actor: &Actor, task_id: &str) -> impl Future<Output = BResult<Option<Value>>> + Send;
    /// Notes of a visible document.
    fn list_notes(&self, actor: &Actor, document_id: i64) -> impl Future<Output = BResult<Vec<NoteEntry>>> + Send;
    /// Create a note (permissions like `POST /api/documents/<id>/notes/`).
    fn add_note(&self, actor: &Actor, document_id: i64, note: &str) -> impl Future<Output = BResult<Vec<NoteEntry>>> + Send;
    /// Visible saved views.
    fn list_saved_views(&self, actor: &Actor) -> impl Future<Output = BResult<Vec<SavedViewInfo>>> + Send;
    /// Document list with raw v1 query parameters (e.g. from filter rules). The server filters permissions.
    fn query_documents(
        &self,
        actor: &Actor,
        params: &[(String, String)],
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = BResult<(i64, Vec<DocumentSummary>)>> + Send;
    /// Scopes of the token (`None` = no entry → all enabled groups).
    fn token_scopes(&self, actor: &Actor) -> impl Future<Output = BResult<Option<Vec<String>>>> + Send;
}
