//! Desktop settings via the supervisor (docs/SETTINGS.md §2.6, WP7).
//!
//! The desktop supervisor sets `PAPERRUST_SUPERVISOR_CONTROL=<run folder>` and puts
//! `desktop-settings.json` there (non-secret values from `desktop.toml`, for display). Changes are
//! written by the server as `settings.request.json` (atomic, `{id, changes}`); the supervisor checks,
//! writes `desktop.toml`, answers with `settings.result.json` (`{id, ok, error, restart}`) and
//! restarts the server if needed. Loopback only (checked by the API).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Environment variable with the run folder of the supervisor.
pub const CONTROL_VAR: &str = "PAPERRUST_SUPERVISOR_CONTROL";
/// Display values (written by the supervisor).
pub const VALUES_FILE: &str = "desktop-settings.json";
/// Change request (written by the server).
pub const REQUEST_FILE: &str = "settings.request.json";
/// Response (written by the supervisor).
pub const RESULT_FILE: &str = "settings.result.json";

/// Desktop keys changeable via the UI (the supervisor checks them once more itself).
pub const WRITABLE: &[&str] = &[
    "desktop.server.port",
    "desktop.server.open_browser",
    "desktop.server.lan_sharing",
    "desktop.server.lan_address",
];

/// Change request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    /// Key → canonical env string.
    pub changes: BTreeMap<String, String>,
}

/// Response of the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Server is restarted (port/LAN sharing).
    #[serde(default)]
    pub restart: bool,
}

/// Run folder of the supervisor (if this process runs under it).
pub fn control_dir(env: &pl_core::config::Env) -> Option<PathBuf> {
    env.get(CONTROL_VAR)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Display values from `desktop-settings.json` (if the file is missing: empty).
pub fn read_values(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(dir.join(VALUES_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Write atomically (file next to it, then rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Sends a change request and waits for the response (at most `timeout`).
pub async fn request(
    dir: &Path,
    changes: BTreeMap<String, String>,
    timeout: Duration,
) -> Result<Outcome, String> {
    let id = format!("{:016x}", rand::random::<u64>());
    let req = Request {
        id: id.clone(),
        changes,
    };
    let _ = std::fs::remove_file(dir.join(RESULT_FILE));
    let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(REQUEST_FILE), &body)
        .map_err(|e| format!("cannot write the request for the desktop app: {e}"))?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(t) = std::fs::read_to_string(dir.join(RESULT_FILE))
            && let Ok(o) = serde_json::from_str::<Outcome>(&t)
            && o.id == id
        {
            let _ = std::fs::remove_file(dir.join(RESULT_FILE));
            return Ok(o);
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = std::fs::remove_file(dir.join(REQUEST_FILE));
            return Err("the desktop app did not answer".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_round_trip() {
        let dir = std::env::temp_dir().join(format!("pl_settings_desktop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VALUES_FILE), r#"{"desktop.server.port":"58080"}"#).unwrap();
        assert_eq!(
            read_values(&dir)
                .get("desktop.server.port")
                .map(String::as_str),
            Some("58080")
        );
        // supervisor dummy
        let d2 = dir.clone();
        let sup = tokio::spawn(async move {
            loop {
                if let Ok(t) = std::fs::read_to_string(d2.join(REQUEST_FILE)) {
                    let r: Request = serde_json::from_str(&t).unwrap();
                    std::fs::remove_file(d2.join(REQUEST_FILE)).unwrap();
                    let o = Outcome {
                        id: r.id,
                        ok: true,
                        error: None,
                        restart: r.changes.contains_key("desktop.server.port"),
                    };
                    write_atomic(&d2.join(RESULT_FILE), &serde_json::to_vec(&o).unwrap()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let ch: BTreeMap<String, String> =
            [("desktop.server.port".to_string(), "58081".to_string())]
                .into_iter()
                .collect();
        let o = request(&dir, ch, Duration::from_secs(5)).await.unwrap();
        assert!(o.ok && o.restart);
        sup.await.unwrap();
        // without a response: timeout, request is withdrawn
        let e = request(&dir, BTreeMap::new(), Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(e.contains("did not answer"));
        assert!(!dir.join(REQUEST_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            control_dir(&pl_core::config::Env::from_pairs([(CONTROL_VAR, "/run")])),
            Some(PathBuf::from("/run"))
        );
    }
}
