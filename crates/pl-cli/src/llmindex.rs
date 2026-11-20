//! `document_llmindex {rebuild,update,compact,migrate}` (B3).
//!
//! upstream: src/documents/management/commands/document_llmindex.py@v3.4.2 (`Command.handle`),
//! src/documents/tasks.py (`llmindex_index(rebuild=)`: only with `llm_index_enabled`, otherwise
//! "LLM index is disabled, skipping update."), src/paperless_ai/indexing.py (`llm_index_compact`,
//! `llm_index_migrate`). Implementation: pl-ai `server::index` (same functions as the tasks
//! in the worker). Without feature `ai` → CommandError.

use crate::args;
use crate::{CliError, CliResult};

pub const CHOICES: &[&str] = &["rebuild", "update", "compact", "migrate"];

/// argparse `choices` for the positional argument `command`.
pub fn parse_command(argv: &[String]) -> CliResult<String> {
    let a = args::parse(argv, &[args::PROGRESS], 1)?;
    let Some(cmd) = a.positionals.first() else {
        return Err(CliError::Usage("the following arguments are required: command".into()));
    };
    if !CHOICES.contains(&cmd.as_str()) {
        return Err(CliError::Usage(format!(
            "argument command: invalid choice: '{cmd}' (choose from 'rebuild', 'update', 'compact', 'migrate')"
        )));
    }
    Ok(cmd.clone())
}

#[cfg(not(feature = "ai"))]
pub async fn command(argv: Vec<String>) -> CliResult {
    parse_command(&argv)?;
    Err(CliError::Command("This build of PaperRust has no AI support (feature `ai`).".into()))
}

#[cfg(feature = "ai")]
pub async fn command(argv: Vec<String>) -> CliResult {
    let cmd = parse_command(&argv)?;
    let c = crate::ctx::Ctx::connect().await?;
    let state = pl_core::AppState::with_pool(c.cfg.clone(), c.pool.clone()).map_err(|e| CliError::Other(e.to_string()))?;
    let err = |e: pl_ai::server::index::AiTaskError| CliError::Command(format!("{}: {}", e.error_type, e.message));
    match cmd.as_str() {
        "compact" => pl_ai::server::index::llm_index_compact(&state).await.map_err(err),
        "migrate" => migrate(&state).await,
        other => {
            let r = pl_ai::server::index::llmindex_index(&state, other == "rebuild").await.map_err(err)?;
            if let Some(msg) = r.as_str() {
                tracing::info!(target: "paperless.tasks", "{msg}");
            }
            Ok(())
        }
    }
}

/// `llm_index_migrate()`: only with the index enabled; PaperRust only knows schema 2, an older
/// index needs a rebuild (upstream `REEMBED_REQUIRED` → warning, no rebuild).
#[cfg(feature = "ai")]
async fn migrate(state: &pl_core::AppState) -> CliResult {
    let cfg = pl_ai::server::effective_config(state).await.map_err(|e| CliError::Command(e.to_string()))?;
    if !cfg.llm_index_enabled() {
        return Ok(());
    }
    let dir = pl_ai::server::index::index_dir(state);
    if !dir.join(pl_ai::vector_store::DB_FILENAME).exists() {
        return Ok(());
    }
    let pending = tokio::task::spawn_blocking(move || {
        pl_ai::vector_store::VecStore::open(&dir, None).and_then(|s| s.has_pending_migration()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| CliError::Other(e.to_string()))?
    .map_err(CliError::Command)?;
    if pending {
        tracing::warn!(
            target: "paperless_ai.indexing",
            "LLM index requires re-embedding, which this automatic migration check will not do on its own - it can be slow and, for a metered embedding backend, cost money. Run 'document_llmindex rebuild' manually when ready."
        );
        eprintln!("LLM index requires re-embedding. Run 'document_llmindex rebuild' manually when ready.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_like_argparse() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_command(&s(&["rebuild"])).unwrap(), "rebuild");
        assert_eq!(parse_command(&s(&["update", "--no-progress-bar"])).unwrap(), "update");
        assert!(matches!(parse_command(&s(&[])), Err(CliError::Usage(_))));
        assert!(matches!(parse_command(&s(&["bogus"])), Err(CliError::Usage(m)) if m.contains("invalid choice: 'bogus'")));
    }
}
