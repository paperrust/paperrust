// upstream: src/documents/views.py@v3.4.2 (update/destroy/notes/TrashView: get_backend().add_or_update /
//           remove / batch_update), src/documents/search/_backend.py@v3.4.2 (SearchIndexLockError →
//           index_document / remove_document_from_index with countdown=60), src/documents/tasks.py@v3.4.2
//
//! Keep the search index up to date. `PAPERRUST_INDEX_WRITER`:
//!
//! * `queue` (default; old name `celery`): task `documents.tasks.index_document(document_id)`
//!   or `remove_document_from_index(doc_id)` into the PaperRust queue (not monitored →
//!   no PaperlessTask row, like upstream).
//! * `rust`: synchronously via pl-indexer (like upstream in the request); if the lock is taken
//!   (`WriteOutcome::Deferred`), the task is enqueued with `countdown=60` via pl-queue.
//! * `off`: nothing.

use serde_json::{Map, Value};

use pl_core::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Via the queue (default).
    Queue,
    Rust,
    Off,
}

pub fn mode(state: &AppState) -> Mode {
    match state.config().env.get("PAPERRUST_INDEX_WRITER").unwrap_or("queue").to_ascii_lowercase().as_str() {
        "rust" | "inline" | "native" => Mode::Rust,
        "off" | "none" => Mode::Off,
        _ => Mode::Queue,
    }
}

async fn enqueue(state: &AppState, task: &str, arg: i64, countdown: Option<f64>) {
    // The queue attached by pl-server; otherwise (tools) a PgQueue on the pool.
    let q = match state.service::<std::sync::Arc<dyn pl_queue::TaskQueue>>() {
        Some(q) => q,
        None => match pl_queue::QueueConfig::from_env(&state.config().env) {
            Ok(cfg) => pl_queue::from_config(&cfg, state),
            Err(e) => {
                tracing::error!("queue config: {e}");
                return;
            }
        },
    };
    let opts = pl_queue::EnqueueOptions { args: vec![Value::from(arg)], countdown, ..Default::default() };
    if let Err(e) = q.enqueue(task, Map::new(), opts).await {
        tracing::error!("enqueue {task}: {e:?}");
    }
}

/// Is a not yet started task `task(id)` already waiting in the PaperRust queue?
///
/// Load suite (s2, 30 users): every change enqueued its own `index_document` task, even
/// if one was already waiting for the same document; the queue grew faster than the worker
/// processed it. A waiting task only reads the state from the DB at execution – the
/// caller has already committed its change by then, so the waiting
/// task covers it. Only state `queued`, never delivered (`attempts = 0`), due, without expiry/revocation;
/// a running task does not count (it may have read the state already). Error (e.g. another
/// queue without this table) → `false` (then enqueue as upstream).
async fn already_pending(state: &AppState, task: &str, id: i64) -> bool {
    let r: Result<Option<i32>, sqlx::Error> = sqlx::query_scalar(
        "SELECT 1 FROM paperrust_task_queue WHERE task_name = $1 AND state = 'queued' AND attempts = 0 \
         AND eta <= now() AND expires IS NULL AND NOT revoke_requested \
         AND args = jsonb_build_array($2::bigint) AND kwargs = '{}'::jsonb LIMIT 1",
    )
    .bind(task)
    .bind(id)
    .fetch_optional(state.db())
    .await;
    matches!(r, Ok(Some(_)))
}

/// Enqueue, unless the same task is already waiting (see [`already_pending`]).
async fn enqueue_once(state: &AppState, task: &str, id: i64) {
    if already_pending(state, task, id).await {
        tracing::debug!(target: "paperless.search", "{task}({id}) already queued, not queued again");
        return;
    }
    enqueue(state, task, id, None).await;
}

/// `get_backend().add_or_update(document)` for the documents `ids`.
pub async fn add_or_update(state: &AppState, ids: &[i64]) {
    match mode(state) {
        Mode::Off => {}
        Mode::Queue => {
            for id in ids {
                enqueue_once(state, "documents.tasks.index_document", *id).await;
            }
        }
        Mode::Rust => {
            let ix = pl_indexer::Indexer::new(pl_indexer::IndexerConfig::from_env());
            for id in ids {
                match ix.add_or_update(state.db(), *id).await {
                    Ok(pl_indexer::WriteOutcome::Deferred) => {
                        enqueue(state, "documents.tasks.index_document", *id, Some(60.0)).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(target: "paperless.search", "index add_or_update {id}: {e}"),
                }
            }
        }
    }
}

/// `get_backend().remove(pk)`.
pub async fn remove(state: &AppState, ids: &[i64]) {
    match mode(state) {
        Mode::Off => {}
        Mode::Queue => {
            for id in ids {
                enqueue(state, "documents.tasks.remove_document_from_index", *id, None).await;
            }
        }
        Mode::Rust => {
            let ix = pl_indexer::Indexer::new(pl_indexer::IndexerConfig::from_env());
            for id in ids {
                match ix.remove(*id).await {
                    Ok(pl_indexer::WriteOutcome::Deferred) => {
                        enqueue(state, "documents.tasks.remove_document_from_index", *id, Some(60.0)).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(target: "paperless.search", "index remove {id}: {e}"),
                }
            }
        }
    }
}
