//! Process-wide overlay of the environment (docs/SETTINGS.md §2.1, §8).
//!
//! `pl-settings` sets the values saved in the UI here (canonical name →
//! env string). [`var`] reads like `std::env::var(name).ok()`, but falls back to the overlay
//! if the name is missing from the process environment (precedence: environment > database).
//! `pl_core::config::{var, set_live_overlay, live_overlay}` use the same storage.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

static OVERLAY: RwLock<Option<Arc<BTreeMap<String, String>>>> = RwLock::new(None);

/// Sets the overlay (empty = none).
pub fn set(values: BTreeMap<String, String>) {
    let mut g = match OVERLAY.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *g = if values.is_empty() { None } else { Some(Arc::new(values)) };
}

/// Current overlay.
pub fn get() -> Option<Arc<BTreeMap<String, String>>> {
    match OVERLAY.read() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

/// `std::env::var(name).ok()`, otherwise the value from the overlay (exactly this name).
pub fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| get().and_then(|o| o.get(name).cloned()))
}

/// Like [`var`] for `OsString` readers.
pub fn var_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).or_else(|| get().and_then(|o| o.get(name).map(Into::into)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_after_process() {
        assert_eq!(var("PL_ENVLIVE_TEST_ONLY_X"), None);
        set([("PL_ENVLIVE_TEST_ONLY_X".to_string(), "1".to_string())].into_iter().collect());
        assert_eq!(var("PL_ENVLIVE_TEST_ONLY_X").as_deref(), Some("1"));
        assert_eq!(var_os("PL_ENVLIVE_TEST_ONLY_X").as_deref(), Some(std::ffi::OsStr::new("1")));
        set(BTreeMap::new());
        assert!(get().is_none());
    }
}
