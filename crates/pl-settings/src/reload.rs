//! Live reload (docs/SETTINGS.md E8, §2.5).
//!
//! Every process holds a [`SettingsHandle`] (`Arc<ArcSwap<Settings>>`); it hangs as a service in
//! `AppState::services` (pl-core cannot know pl-settings – dependency direction). A task
//! listens via `LISTEN paperrust_settings`, reloads all rows on every message, builds the
//! snapshot from environment + DB + file and swaps it. Fallback without NOTIFY (connection gone,
//! PgBouncer): every 30 s a comparison of `paperrust_setting_rev`.
//!
//! Consumers read `handle.load()` per use; subsystems with effect "W" subscribe to
//! [`SettingsHandle::subscribe`] and restart themselves (WP4). S keys set
//! `restart_pending` (comparison with the start values).
//!
//! AC fields are in the snapshot for display; consumers read them like upstream on every
//! use from the table (changes via v1/Django do not trigger NOTIFY).

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use pl_core::config::Env;
use sqlx::PgPool;
use tokio::sync::watch;

use crate::crypto::SecretBox;
use crate::layer::{BootValues, Layers, Settings};
use crate::store;

/// Fallback interval without NOTIFY.
pub const FALLBACK_INTERVAL: Duration = Duration::from_secs(30);

struct Inner {
    current: ArcSwap<Settings>,
    boot: BootValues,
    env: Env,
    sb: SecretBox,
    tx: watch::Sender<Arc<Settings>>,
    /// Publish process-wide (overlay in `pl_core::config`, [`LiveConfig`]) – only the
    /// holder of the server/worker ([`install`]), not test or display holders.
    publish: std::sync::OnceLock<Publish>,
}

struct Publish {
    live: pl_core::live::LiveConfig,
    /// Start values that do not come from the environment (branding defaults from pl-server).
    startup: pl_core::Config,
    /// Values of the restart keys (S/1) valid at startup from the DB – remain until restart.
    boot_overlay: std::collections::BTreeMap<String, String>,
    /// Pure environment (process + file) as the basis of the [`LiveConfig`].
    base: Env,
}

use pl_core::live::LiveConfig;

/// Snapshot holder of a process (cheap to clone).
#[derive(Clone)]
pub struct SettingsHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SettingsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsHandle")
            .field("rev", &self.load().rev)
            .finish()
    }
}

impl SettingsHandle {
    /// From a first snapshot; its S values count as start values.
    pub fn new(initial: Settings, env: Env, sb: SecretBox) -> Self {
        let boot = initial.boot_values();
        let initial = Arc::new(initial);
        let (tx, _) = watch::channel(initial.clone());
        Self {
            inner: Arc::new(Inner {
                current: ArcSwap::new(initial),
                boot,
                env,
                sb,
                tx,
                publish: std::sync::OnceLock::new(),
            }),
        }
    }

    /// Without database (process without DB, tests): environment/file/default only.
    pub fn without_db(env: Env, sb: SecretBox) -> Self {
        let s = Settings::build(&Layers::new(env.clone()), 0, None);
        Self::new(s, env, sb)
    }

    /// First snapshot from the database.
    pub async fn load_initial(pool: &PgPool, env: Env, sb: SecretBox) -> Result<Self, sqlx::Error> {
        let s = store::load_settings(pool, env.clone(), &sb, None).await?;
        Ok(Self::new(s, env, sb))
    }

    /// Make values effective process-wide: rule-N values from the DB as an overlay for
    /// `pl_core::config::var`/`Env::from_process` (consumers that read per use) and a
    /// freshly built [`LiveConfig`] for `AppState::live_config()`.
    pub fn publish_process_wide(&self, live: LiveConfig, startup: pl_core::Config, base: Env) {
        let restart_only = |k: &str| {
            crate::schema::find(k).is_some_and(|d| {
                !matches!(
                    d.effect,
                    crate::schema::Effect::Live | crate::schema::Effect::Worker
                )
            })
        };
        let boot_overlay = pl_core::config::live_overlay()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| restart_only(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if self
            .inner
            .publish
            .set(Publish {
                live,
                startup,
                boot_overlay,
                base,
            })
            .is_ok()
        {
            self.publish(&self.load());
        }
    }

    fn publish(&self, s: &Settings) {
        let Some(p) = self.inner.publish.get() else {
            return;
        };
        let mut overlay: std::collections::BTreeMap<String, String> = s
            .entries()
            .iter()
            // Only immediately effective keys (L/W); S keys apply only after the restart
            // (`restart_pending`), otherwise e.g. cookie names would change in the middle of operation.
            .filter(|e| e.source == crate::layer::Source::Db && crate::layer::rule_n(e.def))
            .filter(|e| {
                matches!(
                    e.def.effect,
                    crate::schema::Effect::Live | crate::schema::Effect::Worker
                )
            })
            .filter_map(|e| e.value.clone().map(|v| (e.def.key.to_string(), v)))
            .collect();
        overlay.extend(p.boot_overlay.iter().map(|(k, v)| (k.clone(), v.clone())));
        let env = p.base.with_overlay(&overlay);
        pl_core::config::set_live_overlay(overlay);
        match pl_core::Config::from_env_map(env) {
            Ok(mut c) => {
                c.product_name.clone_from(&p.startup.product_name);
                c.app_title.clone_from(&p.startup.app_title);
                c.app_logo.clone_from(&p.startup.app_logo);
                p.live.set(c);
            }
            Err(e) => {
                tracing::warn!(error = %e, "settings: live configuration not rebuilt (invalid value), keeping the previous one")
            }
        }
    }

    /// Sets a snapshot directly (without DB, e.g. desktop supervisor or tests); like
    /// [`SettingsHandle::reload`] with notification when the state changed.
    pub fn replace(&self, s: Settings) {
        let changed = s.rev != self.load().rev;
        let s = Arc::new(s);
        self.inner.current.store(s.clone());
        if changed {
            self.publish(&s);
            self.inner.tx.send_replace(s);
        }
    }

    /// Start values of the restart keys (for own snapshots with [`Settings::build`]).
    pub fn boot(&self) -> &BootValues {
        &self.inner.boot
    }

    /// Current snapshot.
    pub fn load(&self) -> Arc<Settings> {
        self.inner.current.load_full()
    }

    /// Subscribe to changes (every new snapshot).
    pub fn subscribe(&self) -> watch::Receiver<Arc<Settings>> {
        self.inner.tx.subscribe()
    }

    /// Environment the process was started from.
    pub fn env(&self) -> &Env {
        &self.inner.env
    }

    /// Key for secrets of this process.
    pub fn secret_box(&self) -> &SecretBox {
        &self.inner.sb
    }

    /// Fresh snapshot directly from the DB (without swapping), with the same start values – for
    /// display (AC columns may have changed via v1/Django without NOTIFY).
    pub async fn fresh(&self, pool: &PgPool) -> Result<Settings, sqlx::Error> {
        store::load_settings(
            pool,
            self.inner.env.clone(),
            &self.inner.sb,
            Some(&self.inner.boot),
        )
        .await
    }

    /// Reloads and swaps the snapshot. `true` if the state changed.
    pub async fn reload(&self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        let s = store::load_settings(
            pool,
            self.inner.env.clone(),
            &self.inner.sb,
            Some(&self.inner.boot),
        )
        .await?;
        let changed = s.rev != self.load().rev;
        let s = Arc::new(s);
        self.inner.current.store(s.clone());
        if changed {
            tracing::info!(
                rev = s.rev,
                restart_pending = s.restart_pending_count(),
                "settings reloaded"
            );
            self.publish(&s);
            self.inner.tx.send_replace(s);
        }
        Ok(changed)
    }

    /// Starts the listener task (until `stop` becomes `true`).
    pub fn spawn_listener(
        &self,
        pool: PgPool,
        stop: watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        self.spawn_listener_with(pool, stop, FALLBACK_INTERVAL)
    }

    /// Like [`SettingsHandle::spawn_listener`] with its own fallback interval (tests).
    pub fn spawn_listener_with(
        &self,
        pool: PgPool,
        mut stop: watch::Receiver<bool>,
        fallback: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let h = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(fallback);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                // FINDINGS B2: LISTEN on its own connection outside the general pool (otherwise
                // the listener permanently holds one of the connections for requests and tasks).
                let mut listener = match pl_core::dbconn::listener(&pool).await {
                    Ok(mut l) => match l.listen(store::NOTIFY_CHANNEL).await {
                        Ok(()) => Some(l),
                        Err(e) => {
                            tracing::warn!(error = %e, "settings: LISTEN failed, polling every {}s", fallback.as_secs());
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "settings: no LISTEN connection, polling every {}s", fallback.as_secs());
                        None
                    }
                };
                // Load once after (re)connecting: missed messages.
                if let Err(e) = h.reload(&pool).await {
                    tracing::debug!(error = %e, "settings reload failed");
                }
                loop {
                    tokio::select! {
                        _ = stop.changed() => {
                            if *stop.borrow() { return; }
                        }
                        n = async {
                            match listener.as_mut() {
                                Some(l) => l.recv().await.map(|_| ()),
                                None => std::future::pending().await,
                            }
                        } => {
                            match n {
                                Ok(()) => {
                                    if let Err(e) = h.reload(&pool).await {
                                        tracing::warn!(error = %e, "settings reload failed");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "settings: LISTEN connection lost, reconnecting");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    break;
                                }
                            }
                        }
                        _ = tick.tick() => {
                            let rev = match pool.acquire().await {
                                Ok(mut c) => store::current_rev(&mut c).await.ok(),
                                Err(_) => None,
                            };
                            if rev.is_some_and(|r| r != h.load().rev)
                                && let Err(e) = h.reload(&pool).await
                            {
                                tracing::warn!(error = %e, "settings reload failed");
                            }
                            if listener.is_none() {
                                break; // try LISTEN again
                            }
                        }
                    }
                }
            }
        })
    }
}

/// Assembly in the server/worker: first snapshot (with DB, otherwise environment only), hook the service into
/// `AppState`, start the listener task. Errors while loading are not fatal (table missing
/// → environment only; `manage migrate` installs the migration).
pub async fn install(
    state: &pl_core::AppState,
    with_db: bool,
    stop: watch::Receiver<bool>,
) -> SettingsHandle {
    let cfg = state.config();
    // Layering always over the pure environment (without the overlay from [`startup_overlay`],
    // otherwise DB values would appear locked as "from environment").
    let env = Env::from_process_raw();
    let sb = SecretBox::from_secret_key(&cfg.secret_key);
    let handle = if with_db {
        let h = match SettingsHandle::load_initial(state.db(), env.clone(), sb.clone()).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "settings: database values not available yet, using environment until reachable");
                SettingsHandle::without_db(env, sb)
            }
        };
        // The task reloads itself after connecting (even if the DB was just unreachable).
        h.spawn_listener(state.db().clone(), stop);
        h
    } else {
        SettingsHandle::without_db(env, sb)
    };
    tracing::info!(
        rev = handle.load().rev,
        supervised = crate::restart::supported(handle.env()),
        "settings snapshot loaded"
    );
    let live = LiveConfig::new(cfg.clone());
    state.set_service(live.clone());
    handle.publish_process_wide(live, cfg.clone(), Env::from_process_raw());
    state.set_service(handle.clone());
    handle
}

/// At process start, **before** the configuration is read: loads the rule-N values saved in the
/// UI (also restart keys) and sets them as an overlay so that
/// `Config::from_env()` contains them. `true` if values were set. Without a reachable DB or
/// without the table: `false` (start as before, environment only).
pub async fn startup_overlay(core: &pl_core::Config) -> bool {
    use sqlx::Connection as _;
    if core.db.engine != pl_core::config::DbEngine::Postgresql {
        return false;
    }
    let opts = pl_core::state::pg_connect_options(&core.db);
    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        sqlx::PgConnection::connect_with(&opts),
    )
    .await;
    let mut conn = match conn {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "settings: database not reachable at startup, environment only");
            return false;
        }
        Err(_) => return false,
    };
    let env = Env::from_process_raw();
    let sb = SecretBox::from_secret_key(&core.secret_key);
    let db = match store::load_db(&mut conn, &sb).await {
        Ok(db) => db,
        Err(e) => {
            tracing::debug!(error = %e, "settings: cannot read paperrust_setting at startup");
            return false;
        }
    };
    let _ = conn.close().await;
    let s = Settings::build(
        &Layers {
            env,
            db,
            ac: None,
            desktop: Default::default(),
        },
        0,
        None,
    );
    let overlay: std::collections::BTreeMap<String, String> = s
        .entries()
        .iter()
        .filter(|e| e.source == crate::layer::Source::Db && crate::layer::rule_n(e.def))
        .filter_map(|e| e.value.clone().map(|v| (e.def.key.to_string(), v)))
        .collect();
    if overlay.is_empty() {
        return false;
    }
    tracing::info!(
        keys = overlay.len(),
        "settings from the web UI applied at startup"
    );
    pl_core::config::set_live_overlay(overlay);
    true
}
