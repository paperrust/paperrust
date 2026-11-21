//! Mounting of the native API v2 (`/rs/api/v2/`, docs/ARCHITECTURE-V2.md §3, ROADMAP-NEXT P3).
//!
//! * Reading: `pl_store::Store` (own SQL, permissions via pl-authz).
//! * Writing: for now via the v1 handlers – assembled here as an internal router from `all_routes()`
//!   (independent of `routes.toml`, handlers are parity-checked).
//! * Change feed: `pl_domain::ChangeFeed` via `pl-sync` ([`SyncFeed`], P2). Without an applied
//!   migration `/rs/api/v2/sync/changes` answers with `503 not_installed`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::routing::MethodRouter;
use pl_core::AppState;

/// Path prefix (paths below it never go to Django, not even %-encoded).
pub const MOUNT_PREFIX: &str = "/rs/api/v2";

/// v1 routes through which v2 writes.
pub const LEGACY_WRITE_ROUTES: &[&str] = &[
    "documents.partial_update",
    "documents.notes.create",
    "documents.notes.destroy",
    "tags.create",
    "tags.partial_update",
    "tags.destroy",
    "correspondents.create",
    "correspondents.partial_update",
    "correspondents.destroy",
    "document_types.create",
    "document_types.partial_update",
    "document_types.destroy",
    "ui_settings.post",
    // Reading for /rs/api/v2/ask (retrieval like MCP ask_documents via the v1 document list)
    "documents.list",
    "documents.retrieve",
];

/// Internal router with the v1 write handlers (state + `Authz` set).
pub fn legacy_router(state: &AppState) -> Router {
    let mut grouped: BTreeMap<&'static str, MethodRouter<AppState>> = BTreeMap::new();
    for r in crate::all_routes() {
        if LEGACY_WRITE_ROUTES.contains(&r.id) {
            let e = grouped.remove(r.path).unwrap_or_default();
            grouped.insert(r.path, e.merge(r.handler));
        }
    }
    let mut router: Router<AppState> = Router::new();
    for (path, mr) in grouped {
        router = router.route(path, mr);
    }
    let authz = pl_authz::Authz::new(state.db().clone(), pl_authz::AuthConfig::from_core(state.config()));
    router
        .with_state(state.clone())
        .layer(axum::Extension(authz))
        .layer(axum::extract::DefaultBodyLimit::max(pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE))
}

/// `pl_domain::ChangeFeed` via the change capture `pl-sync` (docs/contract/pl-sync.md).
pub struct SyncFeed {
    pool: sqlx::PgPool,
    /// Long poll (`wait`): own, bounded secondary pool (FINDINGS B2). `wait_for_changes` holds one LISTEN
    /// connection per waiting client until the timeout; from the main pool even a few
    /// sync clients would occupy all connections for requests and tasks.
    longpoll: sqlx::PgPool,
}

/// Maximum number of simultaneous long-poll connections (`PAPERRUST_DB_LONGPOLL_CONNECTIONS`, default 8).
fn longpoll_connections() -> u32 {
    pl_core::config::var("PAPERRUST_DB_LONGPOLL_CONNECTIONS")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| *v > 0)
        .map_or(8, |v| v.min(200))
}

impl SyncFeed {
    pub fn new(pool: sqlx::PgPool) -> Self {
        let longpoll = pl_core::dbconn::side_pool(&pool, longpoll_connections());
        SyncFeed { pool, longpoll }
    }
}

fn feed_error(e: pl_sync::SyncError) -> pl_domain::DomainError {
    match e {
        pl_sync::SyncError::CursorExpired { .. } => pl_domain::DomainError::CursorExpired,
        pl_sync::SyncError::NotInstalled => pl_domain::DomainError::NotInstalled("sync capture (pl_sync.0001_paperrust_sync)"),
        other => pl_domain::DomainError::Internal(other.to_string()),
    }
}

impl pl_domain::ChangeFeed for SyncFeed {
    fn meta(&self) -> pl_domain::BoxFut<'_, pl_domain::repo::FeedMeta> {
        Box::pin(async move {
            let m = pl_sync::read_meta(&self.pool).await.map_err(feed_error)?;
            let last_seq = pl_sync::last_seq(&self.pool).await.map_err(feed_error)?;
            Ok(pl_domain::repo::FeedMeta { server_uuid: m.server_uuid, perm_epoch: m.perm_epoch, min_cursor: m.min_cursor, last_seq })
        })
    }

    fn changes<'a>(
        &'a self,
        after: Option<i64>,
        limit: i64,
        kinds: Option<&'a [pl_domain::Kind]>,
    ) -> pl_domain::BoxFut<'a, pl_domain::repo::FeedPage> {
        Box::pin(async move {
            let kinds: Option<Vec<pl_sync::Kind>> = kinds.map(|ks| ks.iter().filter_map(|k| pl_sync::Kind::from_code(k.code())).collect());
            let page = pl_sync::changes(&self.pool, after, limit, kinds.as_deref()).await.map_err(feed_error)?;
            let entries = page
                .changes
                .into_iter()
                .filter_map(|c| {
                    Some(pl_domain::repo::FeedEntry {
                        seq: c.seq,
                        kind: pl_domain::Kind::from_code(c.kind.code())?,
                        local_id: c.local_id,
                        uuid: c.uuid,
                        op: match c.op {
                            pl_sync::Op::Upsert => pl_domain::repo::FeedOp::Upsert,
                            pl_sync::Op::Delete => pl_domain::repo::FeedOp::Delete,
                        },
                        rev: c.rev,
                        perm_seq: c.perm_seq,
                    })
                })
                .collect();
            Ok(pl_domain::repo::FeedPage {
                entries,
                next_seq: page.next_cursor,
                has_more: page.has_more,
                meta: pl_domain::repo::FeedMeta {
                    server_uuid: page.server_uuid,
                    perm_epoch: page.perm_epoch,
                    min_cursor: page.min_cursor,
                    last_seq: page.last_seq,
                },
            })
        })
    }

    fn wait(&self, after: i64, timeout: std::time::Duration) -> pl_domain::BoxFut<'_, bool> {
        Box::pin(async move { pl_sync::wait_for_changes(&self.longpoll, after, timeout).await.map_err(feed_error) })
    }
}

/// Router of API v2 (relative to [`MOUNT_PREFIX`]).
pub fn router(state: &AppState) -> Router<AppState> {
    let feed: Arc<dyn pl_domain::ChangeFeed> = Arc::new(SyncFeed::new(state.db().clone()));
    let legacy = pl_store::LegacyRouter::new(legacy_router(state));
    let store = pl_store::Store::new(state.db().clone(), Arc::new(state.config().clone()), Some(legacy.clone()));
    pl_api_v2::router(pl_api_v2::V2::new(Arc::new(store), Some(feed), Some(legacy.router())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_routes_exist() {
        let ids: Vec<&str> = crate::all_routes().iter().map(|r| r.id).collect();
        for id in LEGACY_WRITE_ROUTES {
            assert!(ids.contains(id), "missing v1 route {id}");
        }
        assert_eq!(MOUNT_PREFIX, pl_api_v2::PREFIX);
    }
}
