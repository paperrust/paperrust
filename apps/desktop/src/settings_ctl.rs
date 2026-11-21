//! Desktop settings from the UI (docs/SETTINGS.md §2.6, WP7).
//!
//! Protocol with the server (`pl_settings::desktop`, files in the run folder):
//! * `desktop-settings.json` – non-secret display values from `desktop.toml` (written by the
//!   supervisor at startup and after every change);
//! * `settings.request.json` `{id, changes}` – change request from the server (only from loopback, the
//!   API checks superuser + loopback);
//! * `settings.result.json` `{id, ok, error, restart}` – response; on `restart` the
//!   supervisor restarts the server with the new configuration.
//!
//! The supervisor validates every value itself once more (allow list, types) – the server is not
//! the only party that could write to the run folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::DesktopConfig;

pub const VALUES_FILE: &str = "desktop-settings.json";
pub const REQUEST_FILE: &str = "settings.request.json";
pub const RESULT_FILE: &str = "settings.result.json";

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub id: String,
    pub changes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub restart: bool,
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" | "on" => Some(true),
        "false" | "0" | "f" | "n" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// LAN address: IPv4 or hostname (without port/scheme), empty = automatic.
fn valid_host(v: &str) -> bool {
    v.is_empty()
        || v.parse::<std::net::Ipv4Addr>().is_ok()
        || (v.len() <= 253
            && v.split('.')
                .all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !l.starts_with('-')))
}

/// Applies the changes (all or nothing). Returns: is a server restart needed?
pub fn apply(cfg: &mut DesktopConfig, changes: &BTreeMap<String, String>) -> Result<bool, String> {
    let mut next = cfg.clone();
    let mut restart = false;
    for (k, v) in changes {
        match k.as_str() {
            "desktop.server.port" => {
                let p: u16 = v.trim().parse().map_err(|_| format!("{k}: expected a port number"))?;
                if p < 1024 {
                    return Err(format!("{k}: ports below 1024 are not allowed"));
                }
                restart |= p != next.server.port;
                next.server.port = p;
            }
            "desktop.server.open_browser" => {
                next.server.open_browser = parse_bool(v).ok_or_else(|| format!("{k}: expected yes/no"))?;
            }
            "desktop.server.lan_sharing" => {
                let b = parse_bool(v).ok_or_else(|| format!("{k}: expected yes/no"))?;
                restart |= b != next.server.lan_sharing;
                next.server.lan_sharing = b;
            }
            "desktop.server.lan_address" => {
                let a = v.trim().to_string();
                if !valid_host(&a) {
                    return Err(format!("{k}: expected an IPv4 address or host name"));
                }
                restart |= a != next.server.lan_address;
                next.server.lan_address = a;
            }
            other => return Err(format!("{other}: cannot be changed from the web UI")),
        }
    }
    *cfg = next;
    Ok(restart)
}

/// Non-secret display values (for `desktop-settings.json`).
pub fn public_values(cfg: &DesktopConfig) -> BTreeMap<String, String> {
    let b = |x: bool| if x { "true" } else { "false" }.to_string();
    let mut m = BTreeMap::new();
    m.insert("desktop.database.port".into(), cfg.database.port.to_string());
    m.insert("desktop.database.user".into(), cfg.database.user.clone());
    m.insert("desktop.database.app_user".into(), cfg.database.app_user.clone());
    m.insert("desktop.database.name".into(), cfg.database.name.clone());
    m.insert("desktop.database.locale_provider".into(), cfg.database.locale_provider.clone());
    m.insert("desktop.server.port".into(), cfg.server.port.to_string());
    m.insert("desktop.server.time_zone".into(), cfg.server.time_zone.clone());
    m.insert("desktop.server.ocr_language".into(), cfg.server.ocr_language.clone());
    m.insert("desktop.server.open_browser".into(), b(cfg.server.open_browser));
    m.insert("desktop.server.lan_sharing".into(), b(cfg.server.lan_sharing));
    m.insert("desktop.server.lan_address".into(), cfg.server.lan_address.clone());
    m
}

/// Writes `desktop-settings.json`.
pub fn write_values(run: &Path, cfg: &DesktopConfig) {
    if let Ok(t) = serde_json::to_vec_pretty(&public_values(cfg)) {
        let _ = crate::config::write_private(&run.join(VALUES_FILE), &t);
    }
}

/// Writes the response atomically.
pub fn write_outcome(run: &Path, o: &Outcome) {
    if let Ok(t) = serde_json::to_vec(o) {
        let tmp = run.join("settings.result.tmp");
        if crate::config::write_private(&tmp, &t).is_ok() {
            let _ = std::fs::rename(&tmp, run.join(RESULT_FILE));
        }
    }
}

/// Waits for a change request (polled every 500 ms), reads and deletes it.
pub async fn next_request(file: PathBuf) -> Result<Request, String> {
    loop {
        if let Ok(t) = std::fs::read_to_string(&file) {
            let _ = std::fs::remove_file(&file);
            return serde_json::from_str::<Request>(&t).map_err(|e| format!("invalid settings request: {e}"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn apply_validates_and_reports_restart() {
        let mut c = DesktopConfig::generate();
        let before = c.clone();
        assert_eq!(apply(&mut c, &ch(&[("desktop.server.open_browser", "no")])), Ok(false));
        assert!(!c.server.open_browser);
        assert_eq!(apply(&mut c, &ch(&[("desktop.server.lan_sharing", "true"), ("desktop.server.lan_address", "192.168.1.20")])), Ok(true));
        assert!(c.server.lan_sharing);
        // all or nothing
        let snap = c.clone();
        assert!(apply(&mut c, &ch(&[("desktop.server.port", "60000"), ("desktop.server.lan_address", "evil host/x")])).is_err());
        assert_eq!(c, snap);
        assert!(apply(&mut c, &ch(&[("desktop.server.port", "80")])).is_err());
        assert!(apply(&mut c, &ch(&[("desktop.server.secret_key", "x")])).is_err());
        assert!(apply(&mut c, &ch(&[("desktop.database.password", "x")])).is_err());
        assert_ne!(c, before);
        // display without secrets
        let v = public_values(&c);
        assert!(!v.values().any(|x| *x == c.server.secret_key || *x == c.database.password || *x == c.database.app_password));
        assert_eq!(v["desktop.server.lan_sharing"], "true");
    }

    #[tokio::test]
    async fn request_file_is_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(REQUEST_FILE);
        std::fs::write(&f, r#"{"id":"a1","changes":{"desktop.server.open_browser":"no"}}"#).unwrap();
        let r = next_request(f.clone()).await.unwrap();
        assert_eq!(r.id, "a1");
        assert!(!f.exists());
        write_outcome(dir.path(), &Outcome { id: "a1".into(), ok: true, error: None, restart: false });
        assert!(std::fs::read_to_string(dir.path().join(RESULT_FILE)).unwrap().contains("\"a1\""));
    }
}
