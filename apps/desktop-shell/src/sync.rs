//! Client core for mode 2b/3 (docs/REMOTE-APP.md §1, docs/SYNC.md §4): per active
//! PaperRust profile with `sync = true` or `ui = "app"` one `pl_sync_client::SyncClient`
//! (replica `profiles/<id>/replica.sqlite`, files `profiles/<id>/files/`, token in the
//! Windows keychain under `PaperRust/<id>`) and its reconciliation loop; the gateway
//! ends up in [`crate::prapi::PrapiState`]. paperless-ngx profiles: only with P6.
//!
//! Operation (log in, selection, limits, status) runs via `prapi://…/local/…` – no new
//! IPC commands (docs/contract/pl-sync-client.md "prapi für S9").

use std::sync::{Arc, Mutex};

use paperrust_desktop::paths::AppPaths;
use pl_sync_client::secret::{SecretStore, os_store, target_for};
use pl_sync_client::tls::TlsOptions;
use pl_sync_client::{Gateway, ProfileConfig, SyncClient};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::watch;

use crate::mode::{Flavor, Profile, Ui};
use crate::prapi::PrapiState;

/// Running core (stop via `stop`).
struct Running {
    profile_id: String,
    stop: watch::Sender<bool>,
}

/// Tauri state: at most one active profile (a profile switch restarts the shell).
#[derive(Default)]
pub struct SyncState {
    running: Mutex<Option<Running>>,
}

/// Origin of the app UI (navigation allow list; start address via
/// [`crate::uimode::app_start_url`]). Windows/WebView2: own scheme as `http://<scheme>.localhost`.
#[cfg(windows)]
pub const APP_ORIGIN_URL: &str = "http://prapi.localhost/";
#[cfg(not(windows))]
pub const APP_ORIGIN_URL: &str = "prapi://localhost/";

/// App UI (light UI via prapi) instead of the server page in the window?
pub fn app_ui(p: &Profile) -> bool {
    enabled_for(p) && p.ui == Ui::App
}

/// Light UI build: `PAPERRUST_LIGHT_UI_DIR`, otherwise `light\` next to the program (package), in
/// debug builds alternatively `frontend-light/dist` of the repo.
pub fn app_dir() -> Option<std::path::PathBuf> {
    let ok = |p: std::path::PathBuf| p.join("index.html").is_file().then_some(p);
    if let Some(d) = std::env::var_os("PAPERRUST_LIGHT_UI_DIR").map(std::path::PathBuf::from).and_then(ok) {
        return Some(d);
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("light"))).and_then(ok) {
        return Some(d);
    }
    if cfg!(debug_assertions) {
        return ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend-light/dist"));
    }
    None
}

/// PaperRust (full sync) or paperless-ngx (restricted via v1, P6) – not "unknown".
fn known_flavor(p: &Profile) -> bool {
    matches!(p.flavor, Flavor::Paperrust | Flavor::Ngx)
}

/// Does this profile need the client core?
pub fn enabled_for(p: &Profile) -> bool {
    known_flavor(p) && (p.sync || p.ui == Ui::App)
}

/// Device name for the device token (visible in the profile page of the server).
pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let host = host.trim();
    if host.is_empty() { "PaperRust Desktop".into() } else { format!("PaperRust Desktop ({host})") }
}

/// Configuration of the core for a profile.
pub fn profile_config(paths: &AppPaths, p: &Profile) -> ProfileConfig {
    ProfileConfig {
        profile_id: p.id.clone(),
        url: p.url.clone(),
        tls: TlsOptions { pin: (!p.tls_pin.is_empty()).then(|| p.tls_pin.clone()), ..Default::default() },
        dir: paths.profile_dir(&p.id),
        device_name: device_name(),
        flavor: if p.flavor == Flavor::Ngx { "ngx".into() } else { "paperrust".into() },
    }
}

/// Start the core for the active profile (non-blocking).
pub fn start<R: Runtime>(app: &AppHandle<R>, paths: &AppPaths, p: &Profile) {
    if !enabled_for(p) {
        return;
    }
    let cfg = profile_config(paths, p);
    let app = app.clone();
    let id = p.id.clone();
    tauri::async_runtime::spawn(async move {
        let secrets: Arc<dyn SecretStore> = Arc::from(os_store());
        let client = match SyncClient::open(cfg, secrets).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Sync-Kern konnte nicht starten: {e}");
                return;
            }
        };
        let gw = Arc::new(Gateway::new(client.clone()).with_app_dir(app_dir()));
        let (tx, rx) = watch::channel(false);
        // 2b: `/ws/status/` (processing progress) via a loopback relay of the core
        // (own schemes cannot do WebSockets); address via `prapi …/local/ws`
        match pl_sync_client::wsrelay::bind().await {
            Ok((listener, info)) => {
                gw.set_ws_relay(info.clone());
                let origins = crate::prapi::APP_ORIGINS.iter().map(|o| o.to_string()).collect();
                tauri::async_runtime::spawn(pl_sync_client::wsrelay::serve(listener, info, gw.clone(), origins, rx.clone()));
            }
            Err(e) => tracing::warn!("WebSocket-Vermittler nicht gestartet (Fortschritt ohne Live-Meldungen): {e}"),
        }
        if let Some(s) = app.try_state::<PrapiState>() {
            s.set(Some(gw));
        }
        if let Some(s) = app.try_state::<SyncState>()
            && let Ok(mut r) = s.running.lock()
        {
            if let Some(old) = r.take() {
                let _ = old.stop.send(true);
            }
            *r = Some(Running { profile_id: id, stop: tx });
        }
        tracing::info!("Sync-Kern gestartet");
        client.run(rx).await;
        tracing::info!("Sync-Kern beendet");
    });
}

/// Stop the reconciliation (quitting the app, removing the profile).
pub fn stop<R: Runtime>(app: &AppHandle<R>) {
    if let Some(s) = app.try_state::<SyncState>()
        && let Ok(mut r) = s.running.lock()
        && let Some(old) = r.take()
    {
        let _ = old.stop.send(true);
    }
    if let Some(s) = app.try_state::<PrapiState>() {
        s.set(None);
    }
}

/// Profile removed/logged out: delete the token from the keychain (the revocation on the server is done by the
/// UI beforehand via `prapi …/local/logout`, as long as the server is reachable).
pub fn forget<R: Runtime>(app: &AppHandle<R>, profile_id: &str) {
    let active = app
        .try_state::<SyncState>()
        .and_then(|s| s.running.lock().ok().map(|r| r.as_ref().is_some_and(|r| r.profile_id == profile_id)))
        .unwrap_or(false);
    if active {
        stop(app);
    }
    if let Err(e) = os_store().delete(&target_for(profile_id)) {
        tracing::warn!("Schlüsselbund-Eintrag nicht gelöscht: {e}");
    }
}

/// Active profile for the sync card of the settings page.
#[derive(Debug, serde::Serialize)]
pub struct SyncProfileView {
    pub id: String,
    pub name: String,
    pub flavor: &'static str,
    pub sync: bool,
    /// Offline data possible (PaperRust; paperless-ngx follows with P6)
    pub supported: bool,
    /// `server` | `app`
    pub ui: &'static str,
    /// Light UI build present (app UI selectable)
    pub app_available: bool,
    /// Core runs in this process (`prapi://…/local/…` reachable)
    pub running: bool,
}

/// Sync state of the active profile (`null` without a remote profile).
#[tauri::command]
pub fn sync_profile_state(app: AppHandle) -> Result<Option<SyncProfileView>, String> {
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let cfg = crate::load_config(&paths);
    let running = app.try_state::<SyncState>().and_then(|s| s.running.lock().ok().map(|r| r.is_some())).unwrap_or(false);
    Ok(cfg.profiles.iter().find(|p| p.id == cfg.active_profile).map(|p| SyncProfileView {
        id: p.id.clone(),
        name: p.name.clone(),
        flavor: p.flavor.label(),
        sync: p.sync,
        supported: known_flavor(p),
        ui: if p.ui == Ui::App { "app" } else { "server" },
        app_available: app_dir().is_some(),
        running,
    }))
}

/// Switch offline data (mode 3) on/off for a profile; active profile → restart.
#[tauri::command]
pub fn sync_profile_set(id: String, sync: bool, app: AppHandle, shared: tauri::State<'_, Arc<crate::Shared>>) -> Result<(), String> {
    crate::with_profile(&id, |cfg, _| {
        let p = cfg.profiles.iter_mut().find(|p| p.id == id).ok_or("Unbekanntes Profil.")?;
        if sync && !known_flavor(p) {
            return Err("Server-Art unbekannt – bitte einmal verbinden, dann erneut versuchen.".into());
        }
        p.sync = sync;
        Ok(())
    })?;
    tracing::info!(profile = %id, sync, "Offline-Bestand umgeschaltet");
    if crate::is_active_running(&shared, &id) {
        crate::restart_app(&app, shared.inner());
    }
    Ok(())
}

/// Choose the UI of a profile (`server` = page from the server, `app` = light UI with
/// offline data via prapi); active profile → restart.
#[tauri::command]
pub fn sync_profile_set_ui(id: String, ui: String, app: AppHandle, shared: tauri::State<'_, Arc<crate::Shared>>) -> Result<(), String> {
    let new_ui = match ui.as_str() {
        "app" => Ui::App,
        "server" => Ui::Server,
        _ => return Err("Unbekannte Oberfläche.".into()),
    };
    let available = app_dir().is_some();
    crate::with_profile(&id, |cfg, _| crate::uimode::set_profile_ui(cfg, &id, new_ui, available))?;
    tracing::info!(profile = %id, ui = %ui, "Oberfläche gewechselt");
    if crate::is_active_running(&shared, &id) {
        crate::restart_app(&app, shared.inner());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(flavor: Flavor, ui: Ui, sync: bool) -> Profile {
        Profile {
            id: "a1b2c3".into(),
            name: "Büro".into(),
            url: "https://docs.example.ch/".into(),
            flavor,
            ui,
            tls_pin: String::new(),
            allow_http: false,
            extra_origins: vec![],
            sync,
        }
    }

    #[test]
    fn enabled_only_for_paperrust_app_or_sync() {
        assert!(!enabled_for(&profile(Flavor::Paperrust, Ui::Server, false)));
        assert!(enabled_for(&profile(Flavor::Paperrust, Ui::Server, true)));
        assert!(enabled_for(&profile(Flavor::Paperrust, Ui::App, false)));
        assert!(enabled_for(&profile(Flavor::Ngx, Ui::App, true)), "ngx: P6 (v1, eingeschränkt)");
        assert!(!enabled_for(&profile(Flavor::Ngx, Ui::Server, false)));
        assert!(!enabled_for(&profile(Flavor::Unknown, Ui::App, true)));
    }

    #[test]
    fn config_uses_profile_dir_and_pin() {
        let home = std::env::temp_dir().join("paperrust-p5-sync-test");
        let paths = AppPaths::under(&home);
        let mut p = profile(Flavor::Paperrust, Ui::App, true);
        p.tls_pin = "ab".repeat(32);
        let c = profile_config(&paths, &p);
        assert_eq!(c.dir, paths.profile_dir("a1b2c3"));
        assert_eq!(c.tls.pin.as_deref(), Some(p.tls_pin.as_str()));
        assert!(c.device_name.starts_with("PaperRust Desktop"));
        assert_eq!(target_for(&p.id), "PaperRust/a1b2c3");
    }
}
