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
pub const WRITABLE: &[&str] =
    &["desktop.server.port", "desktop.server.open_browser", "desktop.server.lan_sharing", "desktop.server.lan_address"];

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
    env.get(CONTROL_VAR).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Display values from `desktop-settings.json` (if the file is missing: empty).
pub fn read_values(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(dir.join(VALUES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Write atomically (file next to it, then rename). The temporary file name is unique per
/// call, so concurrent writers of the same target never share (and clobber) a temp file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}-{:016x}.tmp", std::process::id(), rand::random::<u64>()));
    if let Err(e) = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Serializes change requests of this process: the supervisor protocol has exactly one request
/// and one result file, so a second request must not start before the first one is answered
/// or withdrawn (it would overwrite the pending request or delete the other one's result).
static REQUEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Does the request file still hold the request `id` (and not one written by someone else)?
fn request_is_ours(dir: &Path, id: &str) -> bool {
    std::fs::read_to_string(dir.join(REQUEST_FILE)).ok().and_then(|t| serde_json::from_str::<Request>(&t).ok()).is_some_and(|r| r.id == id)
}

/// Sends a change request and waits for the response (at most `timeout`, measured after a
/// running request of this process has finished).
pub async fn request(dir: &Path, changes: BTreeMap<String, String>, timeout: Duration) -> Result<Outcome, String> {
    let _serial = REQUEST_LOCK.lock().await;
    let id = format!("{:016x}", rand::random::<u64>());
    let req = Request { id: id.clone(), changes };
    let _ = std::fs::remove_file(dir.join(RESULT_FILE));
    let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(REQUEST_FILE), &body).map_err(|e| format!("cannot write the request for the desktop app: {e}"))?;
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
            // Withdraw only our own pending request, never one written by another writer.
            if request_is_ours(dir, &id) {
                let _ = std::fs::remove_file(dir.join(REQUEST_FILE));
            }
            return Err("the desktop app did not answer".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Supervisor dummy: answers `n` requests one after another (like the real one: read,
    /// remove the request, apply, write the result).
    fn supervisor(dir: PathBuf, n: usize) -> tokio::task::JoinHandle<Vec<String>> {
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while seen.len() < n {
                if let Ok(t) = std::fs::read_to_string(dir.join(REQUEST_FILE))
                    && let Ok(r) = serde_json::from_str::<Request>(&t)
                    && !seen.contains(&r.id)
                {
                    let _ = std::fs::remove_file(dir.join(REQUEST_FILE));
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let o = Outcome { id: r.id.clone(), ok: true, error: None, restart: false };
                    write_atomic(&dir.join(RESULT_FILE), &serde_json::to_vec(&o).unwrap()).unwrap();
                    seen.push(r.id);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            seen
        })
    }

    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pl_settings_desktop_{tag}_{}_{:x}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Regression: two concurrent changes must not overwrite each other's request/result files;
    /// both get their own answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_requests_both_answered() {
        let dir = test_dir("concurrent");
        let sup = supervisor(dir.clone(), 2);
        let ch = |port: &str| -> BTreeMap<String, String> { [("desktop.server.port".to_string(), port.to_string())].into_iter().collect() };
        let (a, b) = tokio::join!(request(&dir, ch("58081"), Duration::from_secs(5)), request(&dir, ch("58082"), Duration::from_secs(5)),);
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(a.ok && b.ok);
        assert_ne!(a.id, b.id);
        let seen = sup.await.unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen.contains(&a.id) && seen.contains(&b.id));
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: a timed-out request must not remove a pending request written by someone else.
    #[tokio::test]
    async fn timeout_keeps_foreign_request() {
        let dir = test_dir("foreign");
        let d2 = dir.clone();
        let other = tokio::spawn(async move {
            // Wait until our request is on disk (REQUEST_LOCK is process-wide: a parallel test can
            // delay it), then replace it with a foreign one before our timeout runs out.
            while !d2.join(REQUEST_FILE).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let foreign = Request { id: "foreign".into(), changes: BTreeMap::new() };
            write_atomic(&d2.join(REQUEST_FILE), &serde_json::to_vec(&foreign).unwrap()).unwrap();
        });
        let e = request(&dir, BTreeMap::new(), Duration::from_millis(400)).await.unwrap_err();
        other.await.unwrap();
        assert!(e.contains("did not answer"));
        assert!(request_is_ours(&dir, "foreign"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn request_round_trip() {
        let dir = std::env::temp_dir().join(format!("pl_settings_desktop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VALUES_FILE), r#"{"desktop.server.port":"58080"}"#).unwrap();
        assert_eq!(read_values(&dir).get("desktop.server.port").map(String::as_str), Some("58080"));
        // supervisor dummy
        let d2 = dir.clone();
        let sup = tokio::spawn(async move {
            loop {
                if let Ok(t) = std::fs::read_to_string(d2.join(REQUEST_FILE)) {
                    let r: Request = serde_json::from_str(&t).unwrap();
                    std::fs::remove_file(d2.join(REQUEST_FILE)).unwrap();
                    let o = Outcome { id: r.id, ok: true, error: None, restart: r.changes.contains_key("desktop.server.port") };
                    write_atomic(&d2.join(RESULT_FILE), &serde_json::to_vec(&o).unwrap()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let ch: BTreeMap<String, String> = [("desktop.server.port".to_string(), "58081".to_string())].into_iter().collect();
        let o = request(&dir, ch, Duration::from_secs(5)).await.unwrap();
        assert!(o.ok && o.restart);
        sup.await.unwrap();
        // without a response: timeout, request is withdrawn
        let e = request(&dir, BTreeMap::new(), Duration::from_millis(300)).await.unwrap_err();
        assert!(e.contains("did not answer"));
        assert!(!dir.join(REQUEST_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(control_dir(&pl_core::config::Env::from_pairs([(CONTROL_VAR, "/run")])), Some(PathBuf::from("/run")));
    }
}
