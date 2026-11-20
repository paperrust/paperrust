//! Sequencer loop (runs in the worker, `paperrust serve --worker`; SYNC.md §2 "Sequenzer").
//!
//! Woken via `NOTIFY paperrust_sync_pending` (trigger) and additionally every
//! [`SyncConfig::interval`]; retention once per [`SyncConfig::retention_interval`]. Several
//! processes may start the loop – the advisory lock in `paperrust_sync_sequence()`
//! lets only one number at a time. Without an applied migration the loop waits
//! (check every minute) and does nothing.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::{PENDING_CHANNEL, SyncConfig};

const NOT_INSTALLED_RECHECK: Duration = Duration::from_secs(60);
const ERROR_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Runs until `stop` becomes `true`.
pub async fn run(pool: PgPool, cfg: SyncConfig, mut stop: watch::Receiver<bool>) {
    if !cfg.sequencer {
        tracing::info!("sync sequencer disabled (PAPERRUST_SYNC_SEQUENCER)");
        return;
    }
    let mut listener: Option<PgListener> = None;
    let mut installed = false;
    let mut announced_missing = false;
    let mut announced_error = false;
    let mut last_retention: Option<Instant> = None;
    let mut backoff = Duration::ZERO;
    loop {
        if *stop.borrow() {
            break;
        }
        let mut wait = cfg.interval;
        if !installed {
            match crate::installed(&pool).await {
                Ok(true) => {
                    installed = true;
                    announced_error = false;
                    tracing::info!("sync sequencer started");
                }
                Ok(false) => {
                    if !announced_missing {
                        tracing::info!("sync capture not installed; sequencer idle");
                        announced_missing = true;
                    }
                    wait = NOT_INSTALLED_RECHECK;
                }
                Err(e) => {
                    // without a reachable DB (e.g. pure proxy operation) do not warn every 5 s
                    if !announced_error {
                        tracing::warn!(error = %e, "sync sequencer: database check failed (retrying quietly)");
                        announced_error = true;
                    } else {
                        tracing::debug!(error = %e, "sync sequencer: database check failed");
                    }
                    wait = Duration::from_secs(5);
                }
            }
        }
        if installed {
            if listener.is_none() {
                match connect_listener(&pool).await {
                    Ok(l) => listener = Some(l),
                    Err(e) => tracing::warn!(error = %e, "sync sequencer: LISTEN failed, polling meanwhile"),
                }
            }
            match crate::sequence_once(&pool, cfg.batch).await {
                Ok(r) => {
                    backoff = Duration::ZERO;
                    if r.pending > 0 {
                        tracing::debug!(pending = r.pending, rounds = r.rounds, "sync sequencer");
                    }
                }
                Err(e) => {
                    backoff = (backoff * 2).clamp(Duration::from_secs(1), ERROR_BACKOFF_MAX);
                    wait = backoff;
                    tracing::warn!(error = %e, retry_in_ms = wait.as_millis() as u64, "sync sequencer run failed");
                    // tables gone (DB recreated / `sync disable --purge`)? → check again
                    installed = false;
                    announced_missing = false;
                }
            }
            if installed && last_retention.is_none_or(|t| t.elapsed() >= cfg.retention_interval) {
                match crate::retention(&pool, cfg.tombstone_days).await {
                    Ok(n) => {
                        last_retention = Some(Instant::now());
                        if n > 0 {
                            tracing::info!(tombstones = n, days = cfg.tombstone_days, "sync retention");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "sync retention failed"),
                }
            }
        }
        tokio::select! {
            r = stop.changed() => if r.is_err() { break },
            r = async {
                match listener.as_mut() {
                    Some(l) if installed => l.recv().await.map(|_| ()),
                    _ => std::future::pending().await,
                }
            } => if let Err(e) = r {
                tracing::warn!(error = %e, "sync sequencer: LISTEN connection lost");
                listener = None;
            },
            _ = tokio::time::sleep(wait) => {}
        }
    }
    tracing::info!("sync sequencer stopped");
}

async fn connect_listener(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut l = PgListener::connect_with(pool).await?;
    l.listen(PENDING_CHANNEL).await?;
    Ok(l)
}
