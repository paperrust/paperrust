//! Core interfaces. `pl-api-v2` only calls these traits; the implementation lives in `pl-store`
//! (Django tables + `paperrust_sync_object`) or – for the change feed – in `pl-sync`.
//! All methods check permissions themselves (no parameter widens visibility).

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::DomainResult;
use crate::model::*;
use crate::query::DocQuery;

/// Boxed future (dyn-capable traits).
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = DomainResult<T>> + Send + 'a>>;

/// The requesting user. `ext` carries the sign-in context of the implementation (currently
/// `Arc<pl_authz::AuthContext>`) so that the core decides permissions itself; the API sees
/// none of it.
#[derive(Clone)]
pub struct Principal {
    pub user_id: i64,
    pub username: String,
    pub is_superuser: bool,
    pub ext: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Principal").field("user_id", &self.user_id).field("username", &self.username).finish()
    }
}

/// Repository of the v2 objects.
pub trait Repo: Send + Sync {
    /// `GET /me`.
    fn me<'a>(&'a self, p: &'a Principal) -> BoxFut<'a, Me>;

    /// List of a non-document kind (cursor pagination, sorted by name).
    fn list<'a>(&'a self, p: &'a Principal, kind: Kind, req: &'a ListRequest) -> BoxFut<'a, Page<Object>>;

    /// One object (also documents, then without `include`).
    fn get<'a>(&'a self, p: &'a Principal, kind: Kind, uuid: Uuid, sharing: bool) -> BoxFut<'a, Object>;

    /// Several objects of a kind (≤ 500). Returns: (found, missing-or-invisible).
    fn batch<'a>(&'a self, p: &'a Principal, kind: Kind, uuids: &'a [Uuid], include: DocInclude)
    -> BoxFut<'a, (Vec<Object>, Vec<Uuid>)>;

    /// Visible objects by `legacy_id` (for the feed: visibility + embedding).
    fn by_legacy_ids<'a>(&'a self, p: &'a Principal, kind: Kind, ids: &'a [i64], embed: bool)
    -> BoxFut<'a, Vec<(i64, Option<Object>)>>;

    /// Document list via the query AST.
    fn query_documents<'a>(&'a self, p: &'a Principal, q: &'a DocQuery) -> BoxFut<'a, Page<Document>>;

    fn get_document<'a>(&'a self, p: &'a Principal, uuid: Uuid, include: DocInclude) -> BoxFut<'a, Document>;

    fn notes<'a>(&'a self, p: &'a Principal, doc: Uuid) -> BoxFut<'a, Vec<Note>>;
    fn add_note<'a>(&'a self, p: &'a Principal, doc: Uuid, text: &'a str) -> BoxFut<'a, Vec<Note>>;
    fn delete_note<'a>(&'a self, p: &'a Principal, doc: Uuid, note: i64) -> BoxFut<'a, Vec<Note>>;

    /// Create tags, correspondents, document types.
    fn create<'a>(&'a self, p: &'a Principal, kind: Kind, w: &'a ObjectWrite) -> BoxFut<'a, Object>;
    /// Change with optimistic locking (`if_match` = expected `rev`).
    fn update<'a>(&'a self, p: &'a Principal, kind: Kind, uuid: Uuid, if_match: Option<i64>, w: &'a ObjectWrite)
    -> BoxFut<'a, Object>;
    fn delete<'a>(&'a self, p: &'a Principal, kind: Kind, uuid: Uuid, if_match: Option<i64>) -> BoxFut<'a, ()>;

    fn patch_document<'a>(&'a self, p: &'a Principal, uuid: Uuid, if_match: Option<i64>, patch: &'a DocumentPatch)
    -> BoxFut<'a, Document>;

    /// File of a document (permissions like v1 `download`).
    fn file<'a>(&'a self, p: &'a Principal, uuid: Uuid, variant: FileVariant) -> BoxFut<'a, FileRef>;

    /// All visible `(uuid, rev)` of a kind (docs/SYNC.md §3.3 `sync/ids`).
    fn ids<'a>(&'a self, p: &'a Principal, kind: Kind) -> BoxFut<'a, Vec<(Uuid, i64)>>;

    /// Resolve selection (docs/SYNC.md §3.5).
    fn resolve<'a>(&'a self, p: &'a Principal, sel: &'a Selection) -> BoxFut<'a, Vec<ResolvedDoc>>;

    fn ui_settings<'a>(&'a self, p: &'a Principal) -> BoxFut<'a, Value>;
    fn set_ui_settings<'a>(&'a self, p: &'a Principal, settings: &'a Value) -> BoxFut<'a, Value>;
}

// ------------------------------------------------------------------------------ Feed

/// Header data of the change feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedMeta {
    pub server_uuid: String,
    pub perm_epoch: i64,
    pub min_cursor: i64,
    /// highest assigned `seq` (for `as_of_seq`).
    pub last_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedOp {
    Upsert,
    Delete,
}

/// An entry of the feed **before** the permission filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    pub seq: i64,
    pub kind: Kind,
    pub local_id: i64,
    pub uuid: Uuid,
    pub op: FeedOp,
    pub rev: i64,
    /// `seq` of the last permission change (docs/SYNC.md §2a no. 1).
    pub perm_seq: Option<i64>,
}

impl FeedEntry {
    /// Invisible and permission change after the cursor → report to the client as `delete`.
    pub fn perm_changed_after(&self, cursor: i64) -> bool {
        self.perm_seq.is_some_and(|p| p > cursor)
    }
}

/// One page of the feed (unfiltered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedPage {
    pub entries: Vec<FeedEntry>,
    /// last **read** `seq`
    pub next_seq: i64,
    pub has_more: bool,
    pub meta: FeedMeta,
}

/// Change feed (implemented by `pl-sync`, P2). `pl-api-v2` filters by visibility.
pub trait ChangeFeed: Send + Sync {
    fn meta(&self) -> BoxFut<'_, FeedMeta>;
    /// Entries with `seq > after` (`None` = full state), at most `limit`.
    /// `DomainError::CursorExpired` if `after < min_cursor`.
    fn changes<'a>(&'a self, after: Option<i64>, limit: i64, kinds: Option<&'a [Kind]>) -> BoxFut<'a, FeedPage>;
    /// Long poll: waits at most `timeout` for entries `> after`.
    fn wait(&self, after: i64, timeout: Duration) -> BoxFut<'_, bool>;
}

/// Cursor format of the feed (`c1.<seq>`, docs/SYNC.md §3.2).
pub fn format_feed_cursor(seq: i64) -> String {
    format!("c1.{seq}")
}

pub fn parse_feed_cursor(s: &str) -> Option<i64> {
    s.strip_prefix("c1.").and_then(|n| n.parse::<i64>().ok()).filter(|n| *n >= 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_cursor() {
        assert_eq!(format_feed_cursor(1043), "c1.1043");
        assert_eq!(parse_feed_cursor("c1.1043"), Some(1043));
        assert_eq!(parse_feed_cursor("c1.-1"), None);
        assert_eq!(parse_feed_cursor("1043"), None);
    }
}
