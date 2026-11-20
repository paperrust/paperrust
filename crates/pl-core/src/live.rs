//! Continuously updated configuration (docs/SETTINGS.md E8, §2.5).
//!
//! `AppState::config()` remains the start value (bootstrap, class C/D). Consumers of keys
//! that can be changed **live** in the UI (class A) read
//! [`AppState::live_config`]. `pl-settings` plugs the service [`LiveConfig`] in at startup and
//! replaces its content with every new settings snapshot; without it (tests, CLI) `live_config()`
//! returns the start value.

use std::sync::{Arc, RwLock};

use crate::{AppState, Config};

/// Replaceable configuration (service in `AppState::services`).
#[derive(Clone)]
pub struct LiveConfig(Arc<RwLock<Arc<Config>>>);

impl LiveConfig {
    pub fn new(cfg: Config) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(cfg))))
    }

    /// Current configuration.
    pub fn get(&self) -> Arc<Config> {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Replaces the configuration.
    pub fn set(&self, cfg: Config) {
        let mut g = match self.0.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        *g = Arc::new(cfg);
    }
}

impl AppState {
    /// Configuration with the values saved in the UI (class A/B, rule N). Without a plugged-in
    /// [`LiveConfig`] a copy of the start value.
    pub fn live_config(&self) -> Arc<Config> {
        match self.service::<LiveConfig>() {
            Some(l) => l.get(),
            None => Arc::new(self.config().clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap() {
        let cfg = |h: &str| {
            Config::from_env_map(crate::config::Env::from_pairs([
                ("PAPERLESS_SECRET_KEY", "k"),
                ("PAPERLESS_EMAIL_HOST", h),
            ]))
            .unwrap()
        };
        let l = LiveConfig::new(cfg("a"));
        let before = l.get();
        l.set(cfg("b"));
        assert_eq!(before.email_host, "a");
        assert_eq!(l.get().email_host, "b");
    }
}
