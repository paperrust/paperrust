//! L3 (ARCHITECTURE-V2 §5, docs/contract/pl-search.md "L3"): keeps the search index for permissions and
//! trash up to date with the database, so that the fast search path
//! (`PAPERRUST_SEARCH_ENGINE=native`) may apply.
//!
//! Runs in the process that serves HTTP (the pl-search reader lives there and is reloaded
//! immediately after every commit – only then do the entries disappear from `paperrust_search_dirty`).
//! Writes via pl-indexer with the lock compatible with the Python writer. Rests when
//! `PAPERRUST_SEARCH_ENGINE=legacy`, `PAPERRUST_INDEX_WRITER=off`, the migration
//! `pl_indexer.0001_paperrust_search_dirty` is missing or no index directory exists.

use std::time::Duration;

use pl_core::AppState;
use tokio::sync::watch;

const IDLE: Duration = Duration::from_secs(1);
const BUSY_RETRY: Duration = Duration::from_secs(5);
const ERROR_RETRY: Duration = Duration::from_secs(30);

fn writer_off(state: &AppState) -> bool {
    matches!(state.config().env.get("PAPERRUST_INDEX_WRITER").map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "none"))
}

/// Endless loop until `stop`.
pub async fn run(state: AppState, mut stop: watch::Receiver<bool>) {
    let mut wait = IDLE;
    let mut warned = false;
    // The migration does not disappear during operation: after the first `true` do not ask
    // every second anymore (halves the idle queries to PostgreSQL). If the table is missing after all
    // (DB newly created), `drain_once` fails and the check runs again.
    let mut installed = false;
    // Own single-connection pool (memory finding 2026-11-18): the query every second ran over the
    // main pool, which hands out connections in order (FIFO) – so every connection left open after
    // ingestion (one PostgreSQL process each with full caches) stayed in use and never closed.
    let own = pl_core::dbconn::side_pool(state.db(), 1);
    loop {
        tokio::select! {
            _ = stop.changed() => if *stop.borrow() { break },
            _ = tokio::time::sleep(wait) => {}
        }
        if pl_search::views::engine() != pl_search::views::Engine::Native || writer_off(&state) {
            wait = BUSY_RETRY;
            continue;
        }
        let pool = &own;
        match if installed { Ok(true) } else { pl_indexer::dirty::installed(pool).await } {
            Ok(true) => installed = true,
            Ok(false) => {
                wait = ERROR_RETRY;
                continue;
            }
            Err(e) => {
                tracing::debug!(target: "paperless.search", error = %e, "search index sync: database not available");
                wait = ERROR_RETRY;
                continue;
            }
        }
        let ix = pl_indexer::Indexer::new(pl_indexer::IndexerConfig::from_env());
        if !ix.config().index_dir.exists() {
            wait = ERROR_RETRY;
            continue;
        }
        let backend = pl_search::routes::backend(&state);
        let reload = || {
            if let Err(e) = backend.reload() {
                tracing::warn!(target: "paperless.search", error = %e, "search reader reload failed");
            }
        };
        match pl_indexer::dirty::drain_once(&ix, pool, reload).await {
            Ok(d) if d.deferred => {
                tracing::debug!(target: "paperless.search", pending = d.seen, "search index sync: index lock busy");
                wait = BUSY_RETRY;
            }
            Ok(d) if d.seen > 0 => {
                tracing::info!(target: "paperless.search", documents = d.seen, reindexed = d.indexed, "search index permissions/trash brought up to date");
                wait = Duration::ZERO;
                warned = false;
            }
            Ok(_) => {
                wait = IDLE;
                warned = false;
            }
            Err(e) => {
                if warned {
                    tracing::debug!(target: "paperless.search", error = %e, "search index sync failed");
                } else {
                    tracing::warn!(target: "paperless.search", error = %e, "search index sync failed; search uses the exact path until it succeeds");
                    warned = true;
                }
                installed = false;
                wait = ERROR_RETRY;
            }
        }
    }
}
