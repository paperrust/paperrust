//! `document_index reindex|optimize` via pl-indexer.
//!
//! upstream: src/documents/management/commands/document_index.py (v3.4.2).

use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    flag(&["--recreate"], "recreate"),
    flag(&["--if-needed"], "if_needed"),
    value(&["--heap-size-mb"], "heap_size_mb"),
];

#[derive(Debug, Clone, Default)]
pub struct ReindexArgs {
    pub recreate: bool,
    pub if_needed: bool,
    pub heap_size_mb: Option<i64>,
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 1)?;
    let sub = a.positionals.first().map(String::as_str).unwrap_or_default();
    match sub {
        "reindex" => {
            let c = Ctx::connect().await?;
            let r = ReindexArgs { recreate: a.flag("recreate"), if_needed: a.flag("if_needed"), heap_size_mb: a.int("heap_size_mb")? };
            reindex(&c, &r).await
        }
        "optimize" => {
            // upstream: only a log entry (tantivy merges segments itself)
            tracing::info!(target: "paperless.management.document_index", "document_index optimize is a no-op — Tantivy manages segment merging automatically.");
            Ok(())
        }
        "" => Err(CliError::Usage("the following arguments are required: command".into())),
        other => Err(CliError::Usage(format!("argument command: invalid choice: '{other}' (choose from 'reindex', 'optimize')"))),
    }
}

/// `document_index reindex [--recreate] [--if-needed] [--heap-size-mb N]`.
pub async fn reindex(c: &Ctx, r: &ReindexArgs) -> CliResult {
    let mut cfg = pl_indexer::IndexerConfig::from_env();
    cfg.index_dir = c.cfg.index_dir.clone();
    cfg.language = c.cfg.search_language.clone();
    let ix = pl_indexer::Indexer::new(cfg);
    let mut opts = pl_indexer::RebuildOptions { recreate: r.recreate, if_needed: r.if_needed, ..Default::default() };
    if let Some(mb) = r.heap_size_mb {
        opts.writer_heap_bytes = usize::try_from(mb.max(1)).unwrap_or(512) * 1_000_000;
    }
    let stats = ix.rebuild(&c.pool, &opts).await.map_err(|e| CliError::Other(format!("index rebuild failed: {e}")))?;
    if stats.skipped_up_to_date {
        println!("Search index is up to date.");
    }
    Ok(())
}
