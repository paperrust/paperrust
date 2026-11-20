//! `document_archiver [-f/--overwrite] [-d/--document ID] [--processes N] [--no-progress-bar]` (B1).
//!
//! upstream: src/documents/management/commands/document_archiver.py@v3.4.2 (`Command.handle`):
//! `SCRATCH_DIR.mkdir`, documents `Document.objects.all()` or `filter(pk=document)` (only if
//! `--document` truthy), of these the ones without an archive version (`has_archive_version` =
//! `archive_filename is not None`) or all with `--overwrite`; per document
//! `update_document_content_maybe_archive_file(document_id)` via `process_parallel` (B7), errors
//! as "Failed document {id}: {error}". The task itself: pl-consumer
//! `Consumer::update_document_content_maybe_archive_file` (same as in the worker), afterwards like the
//! worker (pl-server `UPDATE_CONTENT_MAYBE_ARCHIVE`): AI index (`llm_index_add_or_update_document`,
//! feature `ai`) and deadline detection (pl-timeline).

use std::sync::Arc;

use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    args::PROCESSES,
    flag(&["-f", "--overwrite"], "overwrite"),
    value(&["-d", "--document"], "document"),
];

/// Selected document IDs (order `Document.Meta.ordering = ("-created",)`).
pub async fn select_ids(pool: &sqlx::PgPool, document: Option<i64>, overwrite: bool) -> CliResult<Vec<i64>> {
    let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id FROM documents_document WHERE deleted_at IS NULL");
    if let Some(d) = document {
        q.push(" AND id = ").push_bind(i32::try_from(d).unwrap_or(-1));
    }
    if !overwrite {
        q.push(" AND archive_filename IS NULL");
    }
    q.push(" ORDER BY created DESC, id DESC");
    let ids: Vec<i32> = q.build_query_scalar().fetch_all(pool).await?;
    Ok(ids.into_iter().map(i64::from).collect())
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let processes = crate::par::processes(&a)?;
    let c = Ctx::connect().await?;
    std::fs::create_dir_all(&c.cfg.scratch_dir)?;
    // `if options["document"]`: 0 counts as "not set" like upstream
    let document = a.int("document")?.filter(|d| *d != 0);
    let ids = select_ids(&c.pool, document, a.flag("overwrite")).await?;
    if ids.is_empty() {
        return Ok(());
    }
    let consumer = pl_consumer::Consumer::from_env(c.pool.clone(), pl_queue::EventBus::new(16))
        .await
        .map_err(|e| CliError::Command(format!("consumer setup failed: {e}")))?;
    let consumer = Arc::new(consumer);
    let state = after::State::new(&c);
    let mut failed = 0usize;
    crate::par::process_parallel(
        ids,
        processes,
        |id: i64| {
            let (consumer, state) = (consumer.clone(), state.clone());
            async move {
                let outcome = consumer.update_document_content_maybe_archive_file(id, false).await.map_err(|e| e.to_string())?;
                if outcome == pl_consumer::reprocess::ReprocessOutcome::Updated {
                    state.after_update(id).await;
                }
                Ok(())
            }
        },
        |id, r: Result<(), String>| {
            if let Err(e) = r {
                println!("Failed document {id}: {e}");
                failed += 1;
            }
        },
    )
    .await;
    // tests/chaos C2: do not hide failures (e.g. database briefly gone) with exit code 0 – otherwise
    // a script/schedule considers the archives renewed. upstream: always 0.
    if failed > 0 {
        return Err(CliError::Command(format!(
            "{failed} document(s) failed (see above); run the command again to retry them"
        )));
    }
    Ok(())
}

/// Follow-up after a successful re-ingest (like the worker).
mod after {
    use super::*;

    #[derive(Clone)]
    pub struct State {
        pool: sqlx::PgPool,
        #[cfg(feature = "ai")]
        app: Option<pl_core::AppState>,
    }

    impl State {
        pub fn new(c: &Ctx) -> Self {
            Self {
                pool: c.pool.clone(),
                #[cfg(feature = "ai")]
                app: pl_core::AppState::with_pool(c.cfg.clone(), c.pool.clone()).ok(),
            }
        }

        pub async fn after_update(&self, id: i64) {
            self.llm_index(id).await;
            pl_timeline::refresh::after_consumption(&self.pool, id, pl_core::config::var).await;
        }

        #[cfg(feature = "ai")]
        async fn llm_index(&self, id: i64) {
            if let Some(app) = &self.app
                && let Err(e) = pl_ai::server::index::update_document_in_llm_index(app, id).await
            {
                tracing::error!(target: "paperless.tasks", "Error updating LLM index for document {id}: {}", e.message);
            }
        }

        #[cfg(not(feature = "ai"))]
        async fn llm_index(&self, _id: i64) {}
    }
}
