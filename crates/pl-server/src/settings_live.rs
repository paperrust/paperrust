//! Settings from the UI in the running server/worker (docs/SETTINGS.md WP4a/WP4c, §2.5).
//!
//! * [`restartable`]: subsystems with effect "W" (scheduler, folder watcher,
//!   sync sequencer) run under a supervisor that, when their keys change, shuts them down in an
//!   orderly way and starts them again with the new values (running work ends normally).
//! * [`LiveConsume`]: ingestion rebuilds its building blocks as soon as the settings
//!   (`paperrust_setting` state) or the ApplicationConfiguration row have changed – so
//!   changes apply from the next document (upstream reads `OcrConfig`/`settings.*` per document).
//! * [`reload_log_filter`]: `PAPERRUST_LOG` live via a `tracing_subscriber::reload` handle.

use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use pl_core::AppState;
use pl_queue::{TaskContext, TaskError, TaskHandler};
use pl_settings::{Settings, SettingsHandle};
use serde_json::Value;
use tokio::sync::{Mutex, watch};

/// Values of the keys `keys` in a snapshot (comparison "has anything changed?").
pub fn values_of(s: &Settings, keys: &[&str]) -> Vec<Option<String>> {
    keys.iter().map(|k| s.value(k).map(str::to_string)).collect()
}

/// All crontab keys (scheduler, effect W).
pub fn cron_keys() -> Vec<&'static str> {
    pl_settings::SCHEMA.iter().filter(|d| matches!(d.ty, pl_settings::schema::Ty::Cron)).map(|d| d.key).collect()
}

/// Keys of the folder watcher.
pub const WATCHER_KEYS: &[&str] = &[
    "PAPERLESS_CONSUMER_POLLING_INTERVAL",
    "PAPERLESS_CONSUMER_RECURSIVE",
    "PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS",
    "PAPERLESS_CONSUMER_STABILITY_DELAY",
    "PAPERLESS_CONSUMER_IGNORE_PATTERNS",
    "PAPERLESS_CONSUMER_IGNORE_DIRS",
    "PAPERLESS_CONSUMER_DISABLE",
];

/// Keys of the worker pool (size, queues, lease/polling; read when the pool is built).
pub const WORKER_KEYS: &[&str] = &[
    "PAPERLESS_TASK_WORKERS",
    "PAPERLESS_THREADS_PER_WORKER",
    "PAPERRUST_QUEUE_NAMES",
    "PAPERRUST_QUEUE_VISIBILITY_TIMEOUT",
    "PAPERRUST_QUEUE_MAX_DELIVERIES",
    "PAPERRUST_QUEUE_POLL_INTERVAL",
    "PAPERRUST_QUEUE_SHUTDOWN_GRACE",
    "PAPERRUST_QUEUE_RESULT_EXPIRES",
    "PAPERLESS_WORKER_TIMEOUT",
];

/// Keys of the sync sequencer.
pub const SYNC_KEYS: &[&str] = &[
    "PAPERRUST_SYNC_SEQUENCER",
    "PAPERRUST_SYNC_TOMBSTONE_DAYS",
    "PAPERRUST_SYNC_INTERVAL",
    "PAPERRUST_SYNC_RETENTION_INTERVAL",
    "PAPERRUST_SYNC_BATCH",
];

/// Start function of a subsystem: receives its own stop signal.
pub type Start = Box<dyn Fn(watch::Receiver<bool>) -> Result<BoxFuture<'static, ()>, String> + Send + Sync>;

/// Runs a subsystem and restarts it as soon as one of the `keys` changes. Without a
/// [`SettingsHandle`] (no DB access) once as before.
pub async fn restartable(state: AppState, name: &'static str, keys: Vec<&'static str>, start: Start, mut stop: watch::Receiver<bool>) {
    let mut sub = state.service::<SettingsHandle>().map(|h| h.subscribe());
    loop {
        let current = sub.as_mut().map(|s| values_of(&s.borrow_and_update(), &keys));
        let (child_tx, child_rx) = watch::channel(false);
        let mut task = match start(child_rx) {
            Ok(f) => Some(tokio::spawn(f)),
            Err(e) => {
                tracing::error!(subsystem = name, "not started: {e}");
                None
            }
        };
        loop {
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() {
                        let _ = child_tx.send(true);
                        if let Some(t) = task { let _ = t.await; }
                        return;
                    }
                }
                r = async {
                    match task.as_mut() {
                        Some(t) => t.await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Err(e) = r { tracing::error!(subsystem = name, error = %e, "subsystem task failed"); }
                    task = None;
                    if sub.is_none() { return; }
                }
                changed = async {
                    match sub.as_mut() {
                        Some(s) => s.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                } => {
                    if !changed { sub = None; continue; }
                    let now = sub.as_mut().map(|s| values_of(&s.borrow_and_update(), &keys));
                    if now != current {
                        tracing::info!(subsystem = name, "settings changed – restarting");
                        let _ = child_tx.send(true);
                        if let Some(t) = task.take() { let _ = t.await; }
                        break;
                    }
                }
            }
        }
    }
}

/// Builds the ingestion building blocks (see `worker::build_consumer`).
pub type BuildConsumer = Arc<dyn Fn() -> BoxFuture<'static, Result<pl_consumer::Consumer, String>> + Send + Sync>;

/// `consume_file` with settings from the next document on.
pub struct LiveConsume {
    state: AppState,
    build: BuildConsumer,
    current: Mutex<(Option<ConsumeKey>, Arc<dyn TaskHandler>)>,
}

/// (settings state, checksum of the AC row).
type ConsumeKey = (i64, String);

impl LiveConsume {
    pub fn new(state: AppState, initial: Arc<pl_consumer::Consumer>, build: BuildConsumer) -> Self {
        let h: Arc<dyn TaskHandler> = Arc::new(pl_consumer::task::handler(initial));
        Self { state, build, current: Mutex::new((None, h)) }
    }

    /// (settings state, checksum of the AC row).
    async fn key(&self) -> Option<ConsumeKey> {
        let rev = self.state.service::<SettingsHandle>().map(|h| h.load().rev).unwrap_or(0);
        let ac: Option<String> = sqlx::query_scalar(
            "SELECT md5(coalesce((SELECT to_jsonb(c)::text FROM paperless_applicationconfiguration c ORDER BY id LIMIT 1), ''))",
        )
        .fetch_one(self.state.db())
        .await
        .ok();
        Some((rev, ac?))
    }

    async fn handler(&self) -> Arc<dyn TaskHandler> {
        let key = self.key().await;
        let mut g = self.current.lock().await;
        match (&g.0, &key) {
            (None, _) => g.0 = key,
            (Some(old), Some(new)) if old != new => match (self.build)().await {
                Ok(c) => {
                    tracing::info!(target: "paperless.consumer", "settings changed – consumer configuration reloaded");
                    g.1 = Arc::new(pl_consumer::task::handler(Arc::new(c)));
                    g.0 = key;
                }
                Err(e) => {
                    tracing::warn!(target: "paperless.consumer", "consumer configuration not reloaded, keeping the previous one: {e}")
                }
            },
            _ => {}
        }
        g.1.clone()
    }

    pub async fn call(&self, ctx: TaskContext) -> Result<Value, TaskError> {
        let h = self.handler().await;
        h.call(ctx).await
    }
}

type ReloadFn = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
static LOG_RELOAD: OnceLock<ReloadFn> = OnceLock::new();

/// Registers the switch of the console log filter (from `cli::init_tracing`).
pub fn set_log_reload(f: ReloadFn) {
    let _ = LOG_RELOAD.set(f);
}

/// Sets `PAPERRUST_LOG` live as soon as it changes in the snapshot.
pub fn reload_log_filter(handle: &SettingsHandle, mut stop: watch::Receiver<bool>) {
    let Some(_) = LOG_RELOAD.get() else { return };
    let mut sub = handle.subscribe();
    let mut last = handle.load().value("PAPERRUST_LOG").map(str::to_string);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop.changed() => if *stop.borrow() { return },
                r = sub.changed() => {
                    if r.is_err() { return; }
                    let now = sub.borrow_and_update().value("PAPERRUST_LOG").map(str::to_string);
                    if now != last {
                        let f = now.clone().unwrap_or_else(|| "info".into());
                        match LOG_RELOAD.get().map(|r| r(&f)) {
                            Some(Ok(())) => tracing::info!(filter = %f, "log filter changed (PAPERRUST_LOG)"),
                            Some(Err(e)) => tracing::warn!(error = %e, "invalid PAPERRUST_LOG, keeping the previous filter"),
                            None => {}
                        }
                        last = now;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn keys_exist_in_schema() {
        for k in WATCHER_KEYS.iter().chain(SYNC_KEYS).chain(WORKER_KEYS) {
            assert!(pl_settings::schema::find(k).is_some(), "{k}");
        }
        assert!(cron_keys().len() >= 9);
    }

    /// WP4c: crontab change → subsystem restarts (without a process restart); other
    /// changes let it keep running.
    #[tokio::test]
    async fn restartable_restarts_on_relevant_change() {
        use pl_settings::layer::{DbEntry, DbValue, Layers};
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([("PAPERLESS_SECRET_KEY", "k")])).unwrap();
        let state =
            AppState::with_pool(cfg, sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://x@127.0.0.1:1/x").unwrap()).unwrap();
        let env = pl_core::config::Env::from_pairs([("X", "1")]);
        let h = SettingsHandle::without_db(env.clone(), pl_settings::crypto::SecretBox::from_secret_key("k"));
        state.set_service(h.clone());
        let starts = Arc::new(AtomicUsize::new(0));
        let s2 = starts.clone();
        let start: Start = Box::new(move |mut rx: watch::Receiver<bool>| {
            s2.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(async move {
                let _ = rx.changed().await;
            }) as BoxFuture<'static, ()>)
        });
        let (tx, rx) = watch::channel(false);
        let j = tokio::spawn(restartable(state, "scheduler", cron_keys(), start, rx));
        let snap = |rev: i64, kv: &[(&str, &str)]| {
            let mut l = Layers::new(env.clone());
            for (k, v) in kv {
                l.db.insert((*k).into(), DbEntry { value: DbValue::Plain((*v).into()), rev, updated_at: None, updated_by: None });
            }
            pl_settings::Settings::build(&l, rev, Some(h.boot()))
        };
        let wait = |n: usize| {
            let s = starts.clone();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while s.load(Ordering::SeqCst) < n {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .is_ok()
            }
        };
        assert!(wait(1).await);
        // unrelated key: no restart
        h.replace(snap(1, &[("PAPERLESS_EMAIL_HOST", "x")]));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        // crontab: restart
        h.replace(snap(2, &[("PAPERLESS_EMAIL_HOST", "x"), ("PAPERLESS_TRAIN_TASK_CRON", "1 2 * * *")]));
        assert!(wait(2).await, "scheduler not restarted");
        tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), j).await.unwrap().unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    /// Without a settings holder: start once, end on stop.
    #[tokio::test]
    async fn restartable_runs_once_without_handle() {
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([("PAPERLESS_SECRET_KEY", "k")])).unwrap();
        let state =
            AppState::with_pool(cfg, sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://x@127.0.0.1:1/x").unwrap()).unwrap();
        let starts = Arc::new(AtomicUsize::new(0));
        let s2 = starts.clone();
        let start: Start = Box::new(move |mut rx: watch::Receiver<bool>| {
            s2.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(async move {
                let _ = rx.changed().await;
            }) as BoxFuture<'static, ()>)
        });
        let (tx, rx) = watch::channel(false);
        let j = tokio::spawn(restartable(state, "test", vec![], start, rx));
        tokio::task::yield_now().await;
        tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), j).await.unwrap().unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }
}
