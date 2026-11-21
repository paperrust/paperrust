//! `document_consumer [directory] [--oneshot] [--testing]` (B4).
//!
//! upstream: src/documents/management/commands/document_consumer.py@v3.4.2 (`Command.handle`):
//! directory = argument or `CONSUMPTION_DIR` ("CONSUMPTION_DIR is not configured"),
//! `Path.resolve()`, "Consumption directory does not exist: …" / "Consumption path is not a
//! directory: …", `SCRATCH_DIR.mkdir`, settings `CONSUMER_*`, enqueue existing files,
//! with `--oneshot` exit afterwards, otherwise watch the folder (pl-consumer `FolderWatcher`, native
//! events or polling as upstream). `consume_file` is enqueued into the
//! PaperRust queue (like `consume_file.apply_async`); the worker does the ingestion.
//! `--testing` (shorter time limits in upstream tests) is accepted and ignored.
//! `PAPERLESS_CONSUMER_DISABLE` applies, as upstream, only to the service, not to this command.

use std::path::PathBuf;
use std::sync::Arc;

use crate::args::{self, flag};
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[flag(&["--oneshot"], "oneshot"), flag(&["--testing"], "testing")];

/// Positional argument or `CONSUMPTION_DIR` → validated, absolute directory.
pub fn resolve_directory(arg: Option<&str>, setting: Option<PathBuf>) -> CliResult<PathBuf> {
    let dir = match arg.filter(|s| !s.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => {
            setting.filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| CliError::Command("CONSUMPTION_DIR is not configured".into()))?
        }
    };
    pl_consumer::watcher::FolderWatcher::check_directory(&dir).map_err(CliError::Command)
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 1)?;
    let settings = pl_consumer::ConsumerSettings::from_env().map_err(|e| CliError::Command(format!("configuration error: {}", e.0)))?;
    let directory = resolve_directory(a.positionals.first().map(String::as_str), Some(settings.consumption_dir.clone()))?;
    std::fs::create_dir_all(&settings.scratch_dir)?;
    let c = crate::ctx::Ctx::connect().await?;
    let qcfg = pl_queue::config::QueueConfig::from_env(&pl_core::config::Env::from_process())
        .map_err(|e| CliError::Command(format!("configuration error: {e}")))?;
    let queue: Arc<dyn pl_queue::TaskQueue> = Arc::new(pl_queue::PgQueue::new(c.pool.clone(), qcfg.max_deliveries));
    let watcher = pl_consumer::watcher::FolderWatcher {
        opts: pl_consumer::watcher::WatchOptions {
            directory,
            recursive: settings.consumer_recursive,
            subdirs_as_tags: settings.consumer_subdirs_as_tags,
            polling_interval: settings.consumer_polling_interval,
            stability_delay: settings.consumer_stability_delay,
            rescan_interval: std::time::Duration::from_secs(300),
        },
        filter: pl_consumer::watcher::ConsumerFilter::with_registry(&settings.consumer_ignore_patterns, &settings.consumer_ignore_dirs),
        pool: c.pool.clone(),
        sink: Arc::new(pl_consumer::watcher::QueueSink(queue)),
    };
    let (tx, rx) = tokio::sync::watch::channel(false);
    let oneshot = a.flag("oneshot");
    if !oneshot {
        // KeyboardInterrupt → "Received interrupt, stopping consumer"
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!(target: "paperless.management.consumer", "Received interrupt, stopping consumer");
                let _ = tx.send(true);
            }
        });
    }
    watcher.run_command(oneshot, rx).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_resolution_like_handle() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().to_string_lossy().to_string();
        assert!(resolve_directory(Some(&d), None).is_ok());
        assert!(resolve_directory(None, Some(t.path().to_path_buf())).is_ok());
        assert!(matches!(resolve_directory(None, None), Err(CliError::Command(m)) if m == "CONSUMPTION_DIR is not configured"));
        let missing = t.path().join("nope").to_string_lossy().to_string();
        assert!(
            matches!(resolve_directory(Some(&missing), None), Err(CliError::Command(m)) if m.starts_with("Consumption directory does not exist: "))
        );
        let f = t.path().join("file.txt");
        std::fs::write(&f, b"x").unwrap();
        let f = f.to_string_lossy().to_string();
        assert!(
            matches!(resolve_directory(Some(&f), None), Err(CliError::Command(m)) if m.starts_with("Consumption path is not a directory: "))
        );
    }
}
