//! PaperRust Desktop – Tauri 2 shell (docs/DESKTOP.md, sections 1, 11, 16 and 22; docs/REMOTE-APP.md).
//!
//! Two modes (`app.toml`, [`mode`]):
//!
//! - **Local (mode 1)**: all domain work (PostgreSQL, migrations, import, server child process,
//!   backup) is done by the library `paperrust_desktop` (`Supervisor`), which the console shell also
//!   uses. Window first `ui/index.html` (splash, first-start form, error page), then – as soon as
//!   `GET /__rs/health` returns 200 – the UI on `http://127.0.0.1:<port>/`.
//! - **Remote (mode 2a)**: no supervisor, no PostgreSQL, no ports. After detection and reachability
//!   check the window loads the UI **from the server** (login there, also
//!   MFA/SSO); own WebView data directory per profile (`profiles/<id>/webview`), navigation only
//!   to the profile's origin (+ confirmed SSO origins), offline page with automatic retry,
//!   confirmed self-signed certificates via SPKI pin ([`wv2`]).
//!
//! Common:
//! - Foreign origins (server, 127.0.0.1) get **no** IPC rights (capability only for the
//!   local pages); navigation per [`nav::NavPolicy`], everything else is opened by the system browser.
//! - **Tray** per mode ([`tray`]). Closing the window = hiding it.
//! - **Single instance**: `tauri-plugin-single-instance` plus the file lock `run/instance.lock`
//!   (held locally by the supervisor, remotely by the shell itself).
//! - **Stop from outside** (`paperrust-desktop stop` → `run/stop.request`) in both modes.
//! - **Windows job object**: all children (PostgreSQL, server, WebView2) end with the app.
//! - **Automatic updates** ([`update`], DESKTOP.md §23): search at start + every 24 h, download
//!   and signature check in the background, installing only after confirmation (window `update`) – locally
//!   after an orderly stop of server and PostgreSQL.
//! - **Without a form** (managed installations, tests): `PAPERRUST_DESKTOP_MODE=local|remote`,
//!   `PAPERRUST_DESKTOP_SERVER_URL`, `PAPERRUST_DESKTOP_ADMIN_*`, `PAPERRUST_DESKTOP_IMPORT[_DB|_MEDIA]`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod design;
mod mode;
mod nav;
// P5: `prapi://` scheme + client core (mode 2b/3), docs/REMOTE-APP.md §5, docs/SYNC.md §4/§5.
mod prapi;
mod remote;
mod sync;
mod tray;
// Interface choice in remote mode (server UI / bundled PaperRust UI) and "change mode", DESKTOP.md §26.
mod uimode;
mod update;
mod wv2;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paperrust_desktop::job::open_external;
use paperrust_desktop::layout::InstallLayout;
use paperrust_desktop::lock::{Acquire, InstanceLock};
use paperrust_desktop::paths::AppPaths;
use paperrust_desktop::supervisor::{
    AdminPrompt, BackupReply, Event, Opened, SetupChoice, SetupOptions, Supervisor, control_channel, setup_from_env, validate_setup,
};
use serde::Serialize;
use tauri::menu::MenuItem;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::{DownloadEvent, PageLoadEvent};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};

use mode::{AppConfig, Mode, Profile, Start};
use nav::{NavDecision, NavPolicy, is_local_page};
use remote::{DetectError, FetchError, Fetcher, ServerUrl, TlsKind};
use tray::{BACKUP_LABEL, PROFILE_PREFIX, TrayMode};

const MAIN: &str = "main";
const TRAY: &str = "main";
const ENV_MODE: &str = "PAPERRUST_DESKTOP_MODE";
const ENV_SERVER_URL: &str = "PAPERRUST_DESKTOP_SERVER_URL";
const ENV_ALLOW_HTTP: &str = "PAPERRUST_DESKTOP_ALLOW_HTTP";
/// Marker in the profile folder: delete WebView data at the next start (log out).
const LOGOUT_MARKER: &str = "logout.request";

/// Offline state (mode 2a) for the local page.
#[derive(Debug, Clone, Default, Serialize)]
struct OfflineView {
    profile: String,
    url: String,
    error: String,
    /// Last successful connection (local time, text) – `None` = never in this run.
    last_ok: Option<String>,
    /// Next automatic attempt (Unix milliseconds).
    retry_at_ms: u64,
}

/// What the local page shows (queried via `shell_state`).
#[derive(Debug, Clone, Default, Serialize)]
struct View {
    stage: String,
    error: Option<String>,
    /// Hint about the error (what to do?).
    advice: Option<String>,
    /// Log file with the details (name under `<home>/logs`).
    error_log: Option<String>,
    /// Mode 1 with automatic login: `…/rs/desktop/login?token=…&next=…` (visible only to the
    /// own pages via IPC, never logged).
    ready_url: Option<String>,
    /// First-start choice (fresh / import export / adopt database).
    need_setup: bool,
    can_migrate_db: bool,
    /// `first` | `local` | `remote`
    mode: String,
    /// First start before any choice: local (three ways) or "Mit einem Server verbinden".
    need_first_start: bool,
    /// Remote mode without a (valid) profile or "add server" from the settings.
    need_connect: bool,
    /// "Add server" came from the settings (cancel leads back there).
    connect_from_settings: bool,
    offline: Option<OfflineView>,
    /// Origin to which the server wants to redirect for login (confirmation pending).
    sso: Option<String>,
    /// Name of the active profile (remote).
    profile: Option<String>,
    /// Profile over unencrypted http.
    insecure: bool,
    /// Package contains the bundled PaperRust interface (`light\`): connect form offers it.
    app_ui_available: bool,
    /// Reconciliation after "Von paperless-ngx umsteigen" (first start only): the page shows verdict and
    /// short summary before it switches to the UI.
    switch: Option<SwitchView>,
}

/// Display of the switch reconciliation (from [`paperrust_desktop::supervisor::SwitchCheck`]).
#[derive(Debug, Clone, Default, Serialize)]
struct SwitchView {
    /// `RUNNING` | `OK` | `WARN` | `FAIL`
    verdict: String,
    summary: Vec<String>,
    /// `umstieg-bericht.html` is in the log folder
    has_report: bool,
}

/// Answer of the form to the supervisor thread.
enum FormAnswer {
    Setup(SetupChoice),
}

/// Running remote mode.
#[derive(Debug, Clone)]
struct RemoteCtx {
    profile: Profile,
    server: ServerUrl,
    online: bool,
    /// Target of an SSO redirect to be confirmed.
    pending_sso: Option<Url>,
}

/// State between Tauri (main thread, commands) and the background threads.
#[derive(Default)]
struct Shared {
    view: Mutex<View>,
    /// Open form request of the supervisor (first start).
    form_tx: Mutex<Option<mpsc::Sender<Option<FormAnswer>>>>,
    /// Quit request to the supervisor (tray, log out, `paperrust-desktop stop`).
    shutdown: tokio::sync::Notify,
    /// User wants to quit – do not show errors on shutdown as an error page any more.
    quitting: AtomicBool,
    /// Supervisor thread is finished (or there is none: remote mode, first start): process may end.
    finished: AtomicBool,
    /// Allowed navigation targets (mode 1: `http://127.0.0.1:<port>`, 2a: profile origin).
    nav: Mutex<NavPolicy>,
    /// Backup requests to the running supervisor (`None` = not ready).
    backup_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<BackupReply>>>,
    backup_running: AtomicBool,
    /// Mode of this run.
    start: Mutex<Option<Start>>,
    /// Local path chosen in the first-start form (before the supervisor).
    pending_setup: Mutex<Option<SetupChoice>>,
    /// Restart after quitting (mode switch, profile switch).
    restart: AtomicBool,
    remote: Mutex<Option<RemoteCtx>>,
    /// Check again immediately (retry, WebView2 load error).
    probe_now: tokio::sync::Notify,
    /// Most recently loaded page (source for the SSO decision).
    current_url: Mutex<Option<Url>>,
    /// Single-instance lock in remote mode (locally the supervisor holds it).
    instance_lock: Mutex<Option<InstanceLock>>,
    /// Update state for the update window and the settings.
    update: Mutex<UpdateView>,
    /// Search for updates immediately ("Jetzt suchen", settings changed).
    update_now: tokio::sync::Notify,
    /// After quitting, start this (checked) installer in this program folder.
    install_update: Mutex<Option<(PathBuf, PathBuf)>>,
    /// Version already offered by itself in this run (open the window only once).
    update_offered: Mutex<Option<String>>,
    /// Mode 1: `run/` folder (login key) and base address `http://127.0.0.1:<port>/` for
    /// automatic login (docs/DESKTOP.md "Anmeldung im Desktop-Betrieb").
    local_login: Mutex<Option<(PathBuf, String)>>,
    /// Protection against login loops.
    relogin_guard: Mutex<nav::ReloginGuard>,
}

/// Mutex without panic on poisoning (a panicking thread should not drag the shell down with it).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Error of the supervisor run for the error page.
struct Failure {
    message: String,
    advice: Option<String>,
    log: &'static str,
}

impl From<paperrust_desktop::DesktopError> for Failure {
    fn from(e: paperrust_desktop::DesktopError) -> Self {
        Self { message: e.to_string(), advice: e.advice().map(str::to_string), log: e.log_file() }
    }
}

impl Failure {
    fn other(message: String) -> Self {
        Self { message, advice: None, log: "desktop.log" }
    }
}

impl Shared {
    fn set_stage(&self, s: &str) {
        lock(&self.view).stage = s.to_string();
    }

    fn set_switch(&self, c: &paperrust_desktop::supervisor::SwitchCheck) {
        lock(&self.view).switch =
            Some(SwitchView { verdict: c.verdict.clone(), summary: c.summary.clone(), has_report: c.report.is_some() });
    }

    /// Mode 1: UI ready on `http://127.0.0.1:<port>/`.
    fn set_ready(&self, url: &str) {
        if let Ok(u) = Url::parse(url) {
            *lock(&self.nav) = NavPolicy::local(u.port_or_known_default().unwrap_or(0));
        }
        let mut v = lock(&self.view);
        v.ready_url = Some(url.to_string());
        v.error = None;
    }

    fn set_error(&self, f: &Failure) {
        let mut v = lock(&self.view);
        v.error = Some(f.message.clone());
        v.advice = f.advice.clone();
        v.error_log = Some(f.log.to_string());
        v.ready_url = None;
        v.need_setup = false;
    }

    /// For "retry": state of a new run.
    fn reset(&self) {
        let mode = lock(&self.view).mode.clone();
        *lock(&self.view) = View { stage: "Wird gestartet".into(), mode, ..View::default() };
        *lock(&self.nav) = NavPolicy::none();
        *lock(&self.local_login) = None;
        self.quitting.store(false, Ordering::SeqCst);
        self.finished.store(false, Ordering::SeqCst);
    }

    fn request_shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        // Answer a waiting form, otherwise the supervisor would hang in the dialog.
        if let Some(tx) = lock(&self.form_tx).take() {
            let _ = tx.send(None);
        }
        self.shutdown.notify_one();
    }

    /// Show the form and wait for the answer (supervisor thread).
    fn ask_form(&self, app: &AppHandle, setup: SetupOptions) -> Option<FormAnswer> {
        if self.quitting.load(Ordering::SeqCst) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        *lock(&self.form_tx) = Some(tx);
        {
            let mut v = lock(&self.view);
            v.need_setup = true;
            v.can_migrate_db = setup.can_migrate_db;
        }
        show_main(app);
        // Runs on the supervisor thread (own Tokio runtime, `block_on`): blocking stops
        // only the supervisor, not Tauri.
        let answer = rx.recv().ok().flatten();
        lock(&self.view).need_setup = false;
        answer
    }

    fn start(&self) -> Option<Start> {
        *lock(&self.start)
    }
}

/// First-start dialog: environment (managed installation), preselection from the first-start form
/// (answered before the supervisor) or form on the local page.
struct FormPrompt {
    app: AppHandle,
    shared: Arc<Shared>,
}

impl AdminPrompt for FormPrompt {
    fn choose_setup(&self, options: &SetupOptions) -> Option<SetupChoice> {
        if let Some(c) = setup_from_env(|k| std::env::var(k).ok()) {
            return Some(c);
        }
        if let Some(c) = lock(&self.shared.pending_setup).take() {
            return Some(c);
        }
        match self.shared.ask_form(&self.app, *options)? {
            FormAnswer::Setup(c) => Some(c),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// IPC commands (only for the local pages, see capabilities/default.json)

#[tauri::command]
fn shell_state(shared: tauri::State<'_, Arc<Shared>>) -> View {
    lock(&shared.view).clone()
}

fn send_answer(shared: &Shared, a: FormAnswer) -> Result<(), String> {
    let tx = lock(&shared.form_tx).take().ok_or_else(|| "Es wird gerade nichts abgefragt.".to_string())?;
    tx.send(Some(a)).map_err(|_| "Einrichtung läuft nicht mehr.".to_string())
}

/// Fields of the first-start form. `mode`: `fresh` | `import` | `migrate-db`. "Start fresh"
/// has no fields any more: the local owner is created without user name/password.
#[derive(Debug, Default, serde::Deserialize)]
struct SetupForm {
    mode: String,
    /// Export folder/ZIP or source URL of the database.
    #[serde(default)]
    source: String,
    /// Old media folder (`migrate-db` only, optional).
    #[serde(default)]
    media: String,
}

fn stage_for(choice: &SetupChoice) -> &'static str {
    match choice {
        SetupChoice::Fresh(_) => "Einrichtung läuft …",
        SetupChoice::ImportExport { .. } => "Import wird vorbereitet …",
        SetupChoice::MigrateDb { .. } => "Datenbank-Umzug wird vorbereitet …",
    }
}

#[tauri::command]
fn submit_setup(form: SetupForm, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let choice = setup_choice(form)?;
    let opts = SetupOptions { can_migrate_db: lock(&shared.view).can_migrate_db };
    validate_setup(&choice, &opts)?;
    let stage = stage_for(&choice);
    send_answer(&shared, FormAnswer::Setup(choice))?;
    shared.set_stage(stage);
    Ok(())
}

/// First start, choice "Auf diesem Computer": save the mode, start the supervisor with the preselection
/// (it then does not ask again).
#[tauri::command]
fn first_start_local(form: SetupForm, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    if shared.start() != Some(Start::FirstRun) {
        return Err("Die Einrichtung läuft bereits.".into());
    }
    let choice = setup_choice(form)?;
    let opts = SetupOptions { can_migrate_db: lock(&shared.view).can_migrate_db };
    validate_setup(&choice, &opts)?;
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let mut cfg = load_config(&paths);
    cfg.mode = Mode::Local;
    save_config(&paths, &cfg)?;
    {
        let mut v = lock(&shared.view);
        v.need_first_start = false;
        v.mode = "local".into();
        v.stage = stage_for(&choice).into();
    }
    *lock(&shared.pending_setup) = Some(choice);
    *lock(&shared.start) = Some(Start::Local);
    install_tray_menu(&app, TrayMode::Local);
    shared.finished.store(false, Ordering::SeqCst);
    spawn_supervisor(app, shared.inner().clone());
    Ok(())
}

/// Form fields → choice (paths/URLs without surrounding spaces and quotes, as they arise
/// when copying from Explorer: "Als Pfad kopieren" adds `"…"`).
fn setup_choice(f: SetupForm) -> Result<SetupChoice, String> {
    let clean = |s: &str| s.trim().trim_matches('"').trim().to_string();
    match f.mode.as_str() {
        "fresh" => Ok(SetupChoice::Fresh(None)),
        "import" => {
            let s = clean(&f.source);
            if s.is_empty() {
                return Err("Bitte den Export-Ordner oder die ZIP-Datei angeben.".into());
            }
            Ok(SetupChoice::ImportExport { source: PathBuf::from(s) })
        }
        "migrate-db" => {
            let s = clean(&f.source);
            if s.is_empty() {
                return Err("Bitte die Quell-Datenbank angeben.".into());
            }
            let m = clean(&f.media);
            Ok(SetupChoice::MigrateDb { source_url: s, media_dir: (!m.is_empty()).then(|| PathBuf::from(m)) })
        }
        _ => Err("Unbekannte Auswahl.".into()),
    }
}

#[tauri::command]
fn cancel_admin(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) {
    request_quit(&app, &shared);
}

#[tauri::command]
fn quit(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) {
    request_quit(&app, &shared);
}

/// Known log files (only these names are opened, no free path from the page).
const KNOWN_LOGS: &[&str] = &[
    "desktop.log",
    "server.log",
    "paperrust.log",
    "postgresql.log",
    "manage.log",
    "migrate-db.log",
    // Verification report of the switch (HTML, in the browser), see SwitchView
    "umstieg-bericht.html",
    "pg_ctl.log",
];

/// Open the log file or the log folder.
#[tauri::command]
fn open_log(name: Option<String>) {
    let Ok(p) = AppPaths::from_env() else {
        return;
    };
    match name.as_deref().filter(|n| KNOWN_LOGS.contains(n)) {
        Some(n) if p.logs.join(n).is_file() => {
            open_external(&p.logs.join(n).display().to_string());
        }
        _ => open_external(&p.logs.display().to_string()),
    }
}

/// After a startup error: restart the supervisor or (remote) reconnect immediately.
#[tauri::command]
fn retry(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    if shared.start() == Some(Start::Remote) {
        {
            let mut v = lock(&shared.view);
            v.error = None;
            v.advice = None;
            v.stage = "Verbindung wird geprüft …".into();
        }
        shared.probe_now.notify_one();
        return Ok(());
    }
    if !shared.finished.load(Ordering::SeqCst) {
        return Err("PaperRust startet bereits.".into());
    }
    shared.reset();
    set_tooltip(&app, "PaperRust – startet …");
    spawn_supervisor(app, shared.inner().clone());
    Ok(())
}

fn request_quit(app: &AppHandle, shared: &Shared) {
    if shared.finished.load(Ordering::SeqCst) {
        finish(app, shared);
        return;
    }
    shared.set_stage("PaperRust wird beendet …");
    set_tooltip(app, "PaperRust – wird beendet …");
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
    shared.request_shutdown();
}

/// End of the run (supervisor ended or none there): install a confirmed update, otherwise restart
/// or quit. Locally, server and PostgreSQL are already stopped in an orderly way here.
fn finish(app: &AppHandle, shared: &Shared) {
    if let Some((installer, dir)) = lock(&shared.install_update).take() {
        match update::launch_installer(&installer, &dir) {
            Ok(pid) => {
                tracing::info!(pid, installer = %installer.display(), "Update-Installer gestartet, PaperRust beendet sich");
                tauri_plugin_single_instance::destroy(app);
                lock(&shared.instance_lock).take();
                app.exit(0);
            }
            Err(e) => {
                // Do not just vanish: start the app again, the error is in the update window.
                tracing::error!("Update-Installer: {e}");
                if let Some(s) = update_store() {
                    s.update_state(|st| {
                        st.installing = None;
                        st.last_error = Some(format!("Installer ließ sich nicht starten: {e}"));
                    });
                }
                relaunch(app);
            }
        }
        return;
    }
    if shared.restart.load(Ordering::SeqCst) {
        relaunch(app);
    } else {
        app.exit(0);
    }
}

/// Restart (mode switch/profile switch). A running local server is stopped in an orderly way beforehand;
/// the supervisor thread restarts afterwards.
fn restart_app(app: &AppHandle, shared: &Arc<Shared>) {
    shared.restart.store(true, Ordering::SeqCst);
    if !shared.finished.load(Ordering::SeqCst) {
        request_quit(app, shared);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Still deliver the command's answer to the page.
        tokio::time::sleep(Duration::from_millis(400)).await;
        relaunch(&app);
    });
}

/// Environment variable for the restarted process: PID of the predecessor (waits for its end).
const ENV_RESTART_AFTER: &str = "PAPERRUST_DESKTOP_RESTART_AFTER";

/// Own restart instead of `AppHandle::restart`: Tauri starts the successor as a normal child –
/// on Windows thus **in the job object** of the app (`kill_children_with_app`), which ends it immediately
/// when the predecessor ends (E2E finding P1). Here: release the single-instance lock, start the successor with
/// `CREATE_BREAKAWAY_FROM_JOB`, quit. The successor waits for the end of the predecessor
/// (WebView2 data folder, lock file), see [`wait_for_predecessor`].
fn relaunch(app: &AppHandle) {
    tauri_plugin_single_instance::destroy(app);
    if let Some(s) = app.try_state::<Arc<Shared>>() {
        lock(&s.instance_lock).take();
    }
    match std::env::current_exe() {
        Ok(exe) => {
            let mut c = std::process::Command::new(exe);
            c.args(std::env::args_os().skip(1)).env(ENV_RESTART_AFTER, std::process::id().to_string());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
                c.creation_flags(CREATE_BREAKAWAY_FROM_JOB);
            }
            match c.spawn() {
                Ok(child) => tracing::info!(pid = child.id(), "Neustart"),
                Err(e) => tracing::error!(error = %e, "Neustart fehlgeschlagen"),
            }
        }
        Err(e) => tracing::error!(error = %e, "Programmpfad für Neustart unbekannt"),
    }
    app.exit(0);
}

/// Restart: wait up to 20 s until the predecessor has ended.
fn wait_for_predecessor() {
    let Some(pid) = std::env::var(ENV_RESTART_AFTER).ok().and_then(|p| p.trim().parse::<u32>().ok()) else {
        return;
    };
    // SAFETY: variable is removed before any threads (start of `main`).
    unsafe { std::env::remove_var(ENV_RESTART_AFTER) };
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 20_000);
            let _ = CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        std::thread::sleep(Duration::from_millis(1500));
    }
}

// ---------------------------------------------------------------------------------------------
// Remote mode: connect, settings, SSO

fn load_config(paths: &AppPaths) -> AppConfig {
    match AppConfig::load(&paths.app_config_file()) {
        Ok(Some((c, notes))) => {
            for n in notes {
                tracing::warn!("{n}");
            }
            c
        }
        Ok(None) => AppConfig::default(),
        Err(e) => {
            tracing::error!("{e}");
            AppConfig::default()
        }
    }
}

/// Like [`load_config`], but an unreadable `app.toml` is an error instead of an empty
/// configuration – for changes that save afterwards (otherwise the profiles would be lost).
fn load_config_strict(paths: &AppPaths) -> Result<AppConfig, String> {
    match AppConfig::load(&paths.app_config_file()) {
        Ok(Some((c, _))) => Ok(c),
        Ok(None) => Ok(AppConfig::default()),
        Err(e) => {
            tracing::error!("{e}");
            Err("app.toml ist nicht lesbar und wird nicht überschrieben – Einzelheiten im Log.".into())
        }
    }
}

fn save_config(paths: &AppPaths, cfg: &AppConfig) -> Result<(), String> {
    paperrust_desktop::perms::create_private_dir(&paths.home).map_err(|e| format!("{}: {e}", paths.home.display()))?;
    cfg.save(&paths.app_config_file())
}

/// Tooltip during operation (on Windows 11 the tray icon is often in the overflow; menu via right click).
const TOOLTIP_READY: &str = "PaperRust – Rechtsklick für Menü";

/// Most recently chosen UI (`app.toml` `[ui] design`, docs/DESIGN2.md §6); default classic.
fn preferred_design() -> design::Design {
    AppPaths::from_env()
        .ok()
        .and_then(|p| AppConfig::load(&p.app_config_file()).ok().flatten())
        .map(|(c, _)| c.ui.design())
        .unwrap_or(design::Design::Classic)
}

/// Remember the choice of UI for the next start. An unreadable `app.toml` is not
/// overwritten (otherwise profiles would be lost).
fn remember_design(d: design::Design) {
    let Ok(paths) = AppPaths::from_env() else {
        return;
    };
    let mut cfg = match AppConfig::load(&paths.app_config_file()) {
        Ok(Some((c, _))) => c,
        Ok(None) => AppConfig::default(),
        Err(e) => {
            tracing::warn!("Oberfläche nicht gemerkt: {e}");
            return;
        }
    };
    if cfg.ui.design() == d && !cfg.ui.design.is_empty() {
        return;
    }
    cfg.ui.design = d.as_str().into();
    match save_config(&paths, &cfg) {
        Ok(()) => tracing::info!(design = d.as_str(), "Oberfläche für den nächsten Start gemerkt"),
        Err(e) => tracing::warn!("Oberfläche nicht gemerkt: {e}"),
    }
}

/// Start address for a PaperRust server (`base` = `…/`) according to the most recently chosen UI.
fn design_start_url(base: &str) -> String {
    design::start_url(base, preferred_design())
}

#[derive(Debug, Default, serde::Deserialize)]
struct ConnectForm {
    url: String,
    #[serde(default)]
    name: String,
    /// User has confirmed "connect unencrypted".
    #[serde(default)]
    allow_http: bool,
    /// User has confirmed the fingerprint (SPKI pin from `confirm_tls`).
    #[serde(default)]
    tls_pin: String,
    /// Interface: `app` = bundled PaperRust interface, `server` = the server's own (default).
    #[serde(default)]
    ui: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct ConnectResult {
    /// `ok` | `confirm_http` | `confirm_tls` | `error`
    status: &'static str,
    message: String,
    base: Option<String>,
    /// Punycode host if the input was an international name.
    idn_host: Option<String>,
    flavor: Option<&'static str>,
    version: Option<String>,
    note: Option<String>,
    cert: Option<remote::CertInfo>,
    tls_kind: Option<TlsKind>,
}

impl ConnectResult {
    fn error(message: impl Into<String>) -> Self {
        Self { status: "error", message: message.into(), ..Self::default() }
    }
}

/// Check the server address, detect it and save it as a profile (afterwards restart in remote mode).
/// Multi-step: http → confirmation; untrusted certificate → show fingerprint;
/// only with all confirmations is it saved.
#[tauri::command]
async fn server_connect(form: ConnectForm, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<ConnectResult, String> {
    let shared = shared.inner().clone();
    let su = match remote::parse_server_url(&form.url) {
        Ok(u) => u,
        Err(e) => return Ok(ConnectResult::error(e)),
    };
    let mut res = ConnectResult { base: Some(su.base.clone()), idn_host: su.idn.then(|| su.host.clone()), ..ConnectResult::default() };
    match remote::http_policy(&su) {
        Err(e) => {
            return Ok(ConnectResult { status: "error", message: e, ..res });
        }
        Ok(true) if !form.allow_http => {
            res.status = "confirm_http";
            res.message = format!(
                "{} ist unverschlüsselt (http). Passwort und Dokumente gehen im Klartext durchs Netz – nur in einem vertrauenswürdigen privaten Netz verwenden.",
                su.origin
            );
            return Ok(res);
        }
        Ok(_) => {}
    }
    let pin = form.tls_pin.trim().to_ascii_lowercase();
    let pin = (su.https && remote::valid_pin(&pin)).then_some(pin);
    let fetcher = match Fetcher::new(&su.base, pin.as_deref()) {
        Ok(f) => f,
        Err(e) => return Ok(ConnectResult::error(e)),
    };
    let detected = remote::detect(&fetcher).await;
    let det = match detected {
        Ok(d) => d,
        Err(DetectError::Fetch(FetchError::Tls(kind, msg))) if su.https && kind != TlsKind::Expired => {
            tracing::info!(url = %su.base, ?kind, "Zertifikat nicht vertrauenswürdig: {msg}");
            return Ok(match Fetcher::peer_certificate(&su.base).await {
                Ok(cert) => ConnectResult {
                    status: "confirm_tls",
                    message: match kind {
                        TlsKind::Name => "Das Zertifikat des Servers ist nicht für diesen Namen ausgestellt.".into(),
                        _ => {
                            "Das Zertifikat des Servers ist nicht von einer bekannten Stelle unterschrieben (z. B. selbst signiert).".into()
                        }
                    },
                    cert: Some(cert),
                    tls_kind: Some(kind),
                    ..res
                },
                Err(e) => ConnectResult::error(format!("Zertifikat nicht abrufbar: {e}")),
            });
        }
        Err(DetectError::Fetch(FetchError::PinMismatch { .. })) => {
            return Ok(match Fetcher::peer_certificate(&su.base).await {
                Ok(cert) => ConnectResult {
                    status: "confirm_tls",
                    message: "Der Fingerabdruck passt nicht zum Zertifikat, das der Server jetzt zeigt. Bitte erneut vergleichen.".into(),
                    cert: Some(cert),
                    tls_kind: Some(TlsKind::Untrusted),
                    ..res
                },
                Err(e) => ConnectResult::error(format!("Zertifikat nicht abrufbar: {e}")),
            });
        }
        Err(e) => return Ok(ConnectResult::error(e.to_string())),
    };
    tracing::info!(url = %su.base, flavor = ?det.flavor, version = ?det.version, "Server erkannt");
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let mut cfg = load_config_strict(&paths)?;
    let ui = uimode::ui_for_new_profile(form.ui.as_deref(), det.flavor, sync::app_dir().is_some());
    let id = cfg.upsert(Profile {
        id: String::new(),
        name: mode::clean_name(&form.name, &su.base),
        url: su.base.clone(),
        flavor: det.flavor,
        ui,
        tls_pin: pin.unwrap_or_default(),
        allow_http: !su.https,
        extra_origins: vec![],
        sync: false,
    });
    cfg.active_profile = id;
    cfg.mode = Mode::Remote;
    save_config(&paths, &cfg)?;
    res.status = "ok";
    res.message = format!("Verbunden: {}{}", det.flavor.label(), det.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default());
    res.flavor = Some(det.flavor.label());
    res.version = det.version;
    res.note = det.note;
    {
        let mut v = lock(&shared.view);
        v.need_first_start = false;
        v.need_connect = false;
        v.stage = format!("{} – PaperRust startet neu …", res.message);
    }
    restart_app(&app, &shared);
    Ok(res)
}

/// Settings: state.
#[derive(Debug, Serialize)]
struct ProfileView {
    id: String,
    name: String,
    url: String,
    flavor: &'static str,
    allow_http: bool,
    tls_pinned: bool,
    extra_origins: Vec<String>,
    active: bool,
}

#[derive(Debug, Serialize)]
struct SettingsView {
    /// Mode according to `app.toml`.
    mode: &'static str,
    /// Mode of this run.
    running: &'static str,
    profiles: Vec<ProfileView>,
    /// Local data present (`pgdata/`), stays untouched on every switch.
    local_data: bool,
    home: String,
}

fn start_str(s: Option<Start>) -> &'static str {
    match s {
        Some(Start::Local) => "local",
        Some(Start::Remote) => "remote",
        _ => "first",
    }
}

#[tauri::command]
fn settings_state(shared: tauri::State<'_, Arc<Shared>>) -> Result<SettingsView, String> {
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let cfg = load_config(&paths);
    Ok(SettingsView {
        mode: cfg.mode.as_str(),
        running: start_str(shared.start()),
        profiles: cfg
            .profiles
            .iter()
            .map(|p| ProfileView {
                id: p.id.clone(),
                name: p.name.clone(),
                url: p.url.clone(),
                flavor: p.flavor.label(),
                allow_http: p.allow_http,
                tls_pinned: !p.tls_pin.is_empty(),
                extra_origins: p.extra_origins.clone(),
                active: p.id == cfg.active_profile,
            })
            .collect(),
        local_data: paths.pgdata.exists(),
        home: paths.home.display().to_string(),
    })
}

fn mode_plan(shared: &Shared, mode: &str) -> Result<(AppPaths, AppConfig, Mode, uimode::SwitchPlan), String> {
    let m = Mode::parse(mode).ok_or("Unbekannte Betriebsart.")?;
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let cfg = load_config_strict(&paths)?;
    let env_mode = std::env::var(ENV_MODE).ok().and_then(|v| Mode::parse(&v));
    let local_data = paths.config_file.exists() || paths.pgdata.exists();
    let plan = uimode::plan_switch(&cfg, shared.start(), env_mode, m, local_data);
    Ok((paths, cfg, m, plan))
}

/// What a mode switch would do (confirmation text of the settings page).
#[tauri::command]
fn settings_mode_plan(mode: String, shared: tauri::State<'_, Arc<Shared>>) -> Result<uimode::SwitchPlan, String> {
    mode_plan(&shared, &mode).map(|(_, _, _, p)| p)
}

/// Switch mode (restarts). Only `mode` in `app.toml` changes: local data, profiles and
/// logins stay untouched; a running local server is stopped in an orderly way by the restart.
#[tauri::command]
fn settings_set_mode(mode: String, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let (paths, mut cfg, m, plan) = mode_plan(&shared, &mode)?;
    if let Some(b) = plan.blocked {
        return Err(b);
    }
    if plan.unchanged {
        return Ok(());
    }
    uimode::apply_switch(&mut cfg, m);
    save_config(&paths, &cfg)?;
    tracing::info!(mode = m.as_str(), next = plan.next, "Betriebsart gewechselt");
    restart_app(&app, shared.inner());
    Ok(())
}

fn with_profile<T>(id: &str, f: impl FnOnce(&mut AppConfig, &AppPaths) -> Result<T, String>) -> Result<T, String> {
    if !mode::valid_profile_id(id) {
        return Err("Unbekanntes Profil.".into());
    }
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let mut cfg = load_config(&paths);
    if cfg.profiles.iter().all(|p| p.id != id) {
        return Err("Unbekanntes Profil.".into());
    }
    let r = f(&mut cfg, &paths)?;
    save_config(&paths, &cfg)?;
    Ok(r)
}

/// Activate profile (and switch to remote mode) – restarts.
#[tauri::command]
fn profile_activate(id: String, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    with_profile(&id, |cfg, _| {
        cfg.active_profile = id.clone();
        cfg.mode = Mode::Remote;
        Ok(())
    })?;
    restart_app(&app, shared.inner());
    Ok(())
}

#[tauri::command]
fn profile_rename(id: String, name: String, app: AppHandle) -> Result<(), String> {
    with_profile(&id, |cfg, _| {
        if let Some(p) = cfg.profile_mut(&id) {
            p.name = mode::clean_name(&name, &p.url.clone());
        }
        Ok(())
    })?;
    if let Some(w) = app.get_webview_window(MAIN) {
        refresh_title(&app, &w);
    }
    let mode = match app.state::<Arc<Shared>>().start() {
        Some(Start::Remote) => TrayMode::Remote,
        Some(Start::Local) => TrayMode::Local,
        _ => TrayMode::FirstRun,
    };
    install_tray_menu(&app, mode);
    Ok(())
}

fn is_active_running(shared: &Shared, id: &str) -> bool {
    shared.start() == Some(Start::Remote) && lock(&shared.remote).as_ref().is_some_and(|r| r.profile.id == id)
}

/// Log out: server logout (if connected) and delete the profile's WebView data. For the active
/// profile the files are in use → delete on restart (marker), otherwise immediately.
#[tauri::command]
fn profile_logout(id: String, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let dir = with_profile(&id, |_, paths| Ok(paths.profile_dir(&id)))?;
    // P5: device token from the keychain (revocation on the server: UI via prapi /local/logout)
    sync::forget(&app, &id);
    if !is_active_running(&shared, &id) {
        let _ = std::fs::remove_dir_all(dir.join("webview"));
        tracing::info!(profile = %id, "abgemeldet (WebView-Daten gelöscht)");
        return Ok(());
    }
    paperrust_desktop::perms::create_private_dir(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(LOGOUT_MARKER), b"logout").map_err(|e| e.to_string())?;
    let (online, base) = lock(&shared.remote).as_ref().map(|r| (r.online, r.server.base.clone())).unwrap_or_default();
    tracing::info!(profile = %id, online, "Abmelden: Server-Logout, WebView-Daten beim Neustart löschen");
    if online && let Ok(u) = Url::parse(&format!("{base}accounts/logout/")) {
        // End the session on the server (paperless-ngx/PaperRust: GET → logout + redirect).
        if let Some(w) = app.get_webview_window(MAIN) {
            let _ = w.navigate(u);
        }
        let app2 = app.clone();
        let shared2 = shared.inner().clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            restart_app(&app2, &shared2);
        });
        return Ok(());
    }
    restart_app(&app, shared.inner());
    Ok(())
}

/// Remove profile (folder including WebView data). The active profile → restart.
#[tauri::command]
fn profile_remove(id: String, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let dir = with_profile(&id, |cfg, paths| {
        cfg.remove(&id);
        Ok(paths.profile_dir(&id))
    })?;
    // P5: device token from the keychain (replica/files are in the profile folder)
    sync::forget(&app, &id);
    if is_active_running(&shared, &id) {
        // Folder is in use; the next start cleans up orphaned profile folders.
        restart_app(&app, shared.inner());
    } else {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

fn local_url(page: &str) -> Option<Url> {
    let base = if cfg!(windows) { "http://tauri.localhost/" } else { "tauri://localhost/" };
    Url::parse(base).ok()?.join(page).ok()
}

fn navigate_main(app: &AppHandle, u: Url) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window(MAIN) {
            let _ = w.navigate(u);
        }
        show_main(&handle);
    });
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    if let Some(u) = local_url("settings.html") {
        navigate_main(&app, u);
    }
}

/// Close settings: back to the UI or to the start page.
#[tauri::command]
fn close_settings(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) {
    let target = lock(&shared.view).ready_url.as_deref().and_then(|u| Url::parse(u).ok()).or_else(|| local_url("index.html"));
    if let Some(u) = target {
        navigate_main(&app, u);
    }
}

/// "Add server" from the settings: connection form on the start page.
#[tauri::command]
fn begin_connect(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) {
    {
        let mut v = lock(&shared.view);
        v.need_connect = true;
        v.connect_from_settings = true;
    }
    if let Some(u) = local_url("index.html") {
        navigate_main(&app, u);
    }
}

#[tauri::command]
fn cancel_connect(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) {
    // Remote without a profile stays at the form.
    let no_profile = shared.start() == Some(Start::Remote) && lock(&shared.remote).is_none();
    let back = {
        let mut v = lock(&shared.view);
        let from_settings = v.connect_from_settings;
        v.connect_from_settings = false;
        if no_profile {
            false
        } else {
            v.need_connect = false;
            from_settings
        }
    };
    if back && let Some(u) = local_url("settings.html") {
        navigate_main(&app, u);
    }
}

/// SSO redirect: `allow` (allow origin permanently), `browser` (in the system browser), otherwise
/// cancel.
#[tauri::command]
fn sso_decide(choice: String, app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let (target, profile_id, base) = {
        let mut r = lock(&shared.remote);
        let r = r.as_mut().ok_or("Kein Server aktiv.")?;
        (r.pending_sso.take(), r.profile.id.clone(), r.server.base.clone())
    };
    lock(&shared.view).sso = None;
    let target = target.ok_or("Keine Weiterleitung offen.")?;
    let origin = target.origin().ascii_serialization();
    match choice.as_str() {
        "allow" => {
            let extra = with_profile(&profile_id, |cfg, _| {
                let p = cfg.profile_mut(&profile_id).ok_or("Unbekanntes Profil.")?;
                if !p.extra_origins.contains(&origin) {
                    p.extra_origins.push(origin.clone());
                }
                Ok(p.extra_origins.clone())
            })?;
            if let Some(r) = lock(&shared.remote).as_mut() {
                r.profile.extra_origins = extra.clone();
            }
            *lock(&shared.nav) = NavPolicy::remote(&base, &extra);
            tracing::info!(%origin, "Anmelde-Origin bestätigt");
            navigate_main(&app, target);
        }
        "browser" => {
            open_external(target.as_str());
            if let Ok(u) = Url::parse(&base) {
                navigate_main(&app, u);
            }
        }
        _ => {
            if let Ok(u) = Url::parse(&base) {
                navigate_main(&app, u);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Window and tray

/// Open a link in the system browser. `PAPERRUST_DESKTOP_E2E_OPEN_LOG=<file>` (E2E only): instead of
/// opening, append a line – the test run should not start a browser of the machine.
fn open_link(target: &str) {
    if let Some(log) = std::env::var_os("PAPERRUST_DESKTOP_E2E_OPEN_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{target}");
        }
        return;
    }
    open_external(target);
}

/// Mode 2b: link of the light UI into the classic UI (`/documents/12/notes` …) on the
/// app origin → online open on the server in the system browser (the window stays in the app; the
/// classic UI needs session + scripts, neither would work via prapi). Offline,
/// the window navigates on and the gateway shows "nur online verfügbar".
fn classic_link(app: &AppHandle, u: &Url) -> Option<String> {
    let app_origin = Url::parse(sync::APP_ORIGIN_URL).ok()?.origin();
    if u.origin() != app_origin {
        return None;
    }
    let gw = app.try_state::<prapi::PrapiState>()?.get()?;
    gw.classic_url(u.path(), u.query())
}

fn on_navigation(app: &AppHandle, u: &Url) -> bool {
    if let Some(target) = classic_link(app, u) {
        tracing::info!("klassische Oberfläche im Browser: {}", u.path());
        open_link(&target);
        return false;
    }
    let shared = app.state::<Arc<Shared>>().inner().clone();
    if let Some(target) = desktop_relogin(&shared, u) {
        tracing::info!("Anmeldeseite erreicht – automatische Anmeldung");
        navigate_main(app, target);
        return false;
    }
    let current = lock(&shared.current_url).clone();
    let decision = lock(&shared.nav).decide(current.as_ref(), u);
    match decision {
        NavDecision::Allow => {
            // `?design=…` (tray, Angular user menu "Neue Oberfläche ausprobieren"): remember the choice
            if matches!(shared.start(), Some(Start::Local | Start::Remote))
                && let Some(d) = design::from_query(u)
            {
                remember_design(d);
            }
            true
        }
        NavDecision::AskSso(origin) => {
            tracing::info!(%origin, "Anmeldung über fremde Origin – Bestätigung");
            if let Some(r) = lock(&shared.remote).as_mut() {
                r.pending_sso = Some(u.clone());
            }
            lock(&shared.view).sso = Some(origin);
            if let Some(l) = local_url("index.html") {
                navigate_main(app, l);
            }
            false
        }
        NavDecision::External => {
            // Foreign links (docs, mail links, webhooks …) in the system browser, never in the app window.
            if matches!(u.scheme(), "http" | "https" | "mailto") {
                open_link(u.as_str());
            }
            false
        }
    }
}

/// Mode 1: login page of the local server (log out, session expired) → instead of it the
/// login URL with the key from `run/` (target = `next` of the login page). `None`: no local
/// operation, no login page, no key or loop protection applies (then the
/// normal login page appears).
fn desktop_relogin(shared: &Shared, u: &Url) -> Option<Url> {
    let (run, base) = lock(&shared.local_login).clone()?;
    let port = Url::parse(&base).ok()?.port()?;
    let mut next = nav::local_login_next(u, port)?;
    // Without a target (e.g. after "Abmelden") not to "/" (= classic UI), but back to the
    // chosen UI (finding from the owner 2026-11-20: logging out switched to the classic view).
    if next == "/" {
        next = Url::parse(&design_start_url(&base)).map(|s| s.path().to_string()).unwrap_or(next);
    }
    if !lock(&shared.relogin_guard).allow(std::time::Instant::now()) {
        tracing::warn!("automatische Anmeldung mehrfach kurz hintereinander – Anmeldeseite wird gezeigt");
        return None;
    }
    let login = paperrust_desktop::autologin::login_url_for(&run, &base, &next)?;
    Url::parse(&login).ok()
}

/// Start address in mode 1: with key the login URL (target = path of `start`), otherwise `start`.
fn local_open_url(run: &std::path::Path, base: &str, start: &str) -> String {
    let next = Url::parse(start)
        .map(|u| match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        })
        .unwrap_or_else(|_| "/".into());
    paperrust_desktop::autologin::login_url_for(run, base, &next).unwrap_or_else(|| start.to_string())
}

fn window_title(shared: &Shared) -> String {
    match lock(&shared.remote).as_ref() {
        Some(r) if !r.server.https => {
            format!("PaperRust – {} – ⚠ unverschlüsselt (http)", r.profile.name)
        }
        Some(r) => format!("PaperRust – {}", r.profile.name),
        None => "PaperRust".into(),
    }
}

fn refresh_title(app: &AppHandle, w: &tauri::WebviewWindow) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    if let Some(r) = lock(&shared.remote).as_mut()
        && let Ok(paths) = AppPaths::from_env()
        && let Some(p) = load_config(&paths).profiles.into_iter().find(|p| p.id == r.profile.id)
    {
        r.profile.name = p.name;
    }
    let _ = w.set_title(&window_title(&shared));
}

/// `data_dir`: own WebView data directory (remote profile); `None` = default (mode 1, as
/// before P1).
fn create_main(app: &AppHandle, data_dir: Option<PathBuf>) -> tauri::Result<()> {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let downloads = app.path().download_dir().ok();
    let nav_app = app.clone();
    let page_shared = shared.clone();
    let mut b = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title(window_title(&shared))
        .inner_size(1280.0, 860.0)
        .min_inner_size(480.0, 600.0)
        .center()
        // Otherwise Tauri intercepts drag & drop on Windows and the PaperRust UI gets no
        // HTML5 drop events (upload by dragging into the document list).
        .disable_drag_drop_handler()
        .on_navigation(move |u| on_navigation(&nav_app, u))
        .on_page_load(move |_w, p| {
            if p.event() == PageLoadEvent::Started {
                *lock(&page_shared.current_url) = Some(p.url().clone());
            }
        })
        .on_download(move |_webview, event| {
            match event {
                DownloadEvent::Requested { url, destination } => {
                    // Only redirect relative suggestions; an absolute path chosen by the WebView
                    // (download folder of the user) stays.
                    if destination.is_relative()
                        && let Some(d) = &downloads
                    {
                        *destination = d.join(&*destination);
                    }
                    tracing::info!(%url, to = %destination.display(), "Download");
                }
                DownloadEvent::Finished { url, success, .. } => {
                    tracing::info!(%url, success, "Download beendet");
                }
                _ => {}
            }
            true
        });
    if let Some(d) = data_dir {
        b = b.data_directory(d);
    }
    let w = b.build()?;
    if shared.start() == Some(Start::Remote) {
        let cert_shared = shared.clone();
        let fail_shared = shared.clone();
        wv2::install(
            &w,
            Arc::new(move |uri: &str, pin: &str, hard: bool| {
                let r = lock(&cert_shared.remote);
                let Some(r) = r.as_ref() else { return false };
                !hard
                    && !r.profile.tls_pin.is_empty()
                    && r.profile.tls_pin == pin
                    && Url::parse(uri).is_ok_and(|u| u.origin().ascii_serialization() == r.server.origin)
            }),
            Arc::new(move || fail_shared.probe_now.notify_one()),
        );
    }
    Ok(())
}

fn show_main(app: &AppHandle) {
    match app.get_webview_window(MAIN) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        None => {
            if let Err(e) = create_main(app, None) {
                tracing::error!(error = %e, "Fenster lässt sich nicht öffnen");
            }
        }
    }
}

/// After an error/offline: window back to the local page (it shows the message). The
/// window is not recreated (D2 E2E: destroying + recreating from the supervisor thread ended
/// the whole process on Windows), but navigated back on the main thread.
fn show_error_page(app: &AppHandle) {
    let handle = app.clone();
    let r = app.run_on_main_thread(move || {
        match handle.get_webview_window(MAIN) {
            Some(w) => {
                let on_local = w.url().is_ok_and(|u| is_local_page(&u));
                if !on_local && let Some(u) = local_url("index.html") {
                    let _ = w.navigate(u);
                }
            }
            None => {
                if let Err(e) = create_main(&handle, None) {
                    tracing::error!(error = %e, "Fenster lässt sich nicht öffnen");
                }
            }
        }
        show_main(&handle);
    });
    if let Err(e) = r {
        tracing::error!(error = %e, "Fehlerseite lässt sich nicht anzeigen");
    }
}

fn set_tooltip(app: &AppHandle, text: &str) {
    if let Some(t) = app.tray_by_id(TRAY) {
        let _ = t.set_tooltip(Some(text));
    }
}

fn open_dir(dir: impl Fn(&AppPaths) -> PathBuf) {
    match AppPaths::from_env() {
        Ok(p) => open_external(&dir(&p).display().to_string()),
        Err(e) => tracing::warn!(error = %e, "Datenverzeichnis unbekannt"),
    }
}

/// Menu entry "Sicherung jetzt" (text/state during a backup).
#[derive(Default)]
struct BackupItem(Mutex<Option<MenuItem<Wry>>>);

fn set_backup_item(app: &AppHandle, text: &str, enabled: bool) {
    if let Some(item) = app.try_state::<BackupItem>()
        && let Some(i) = lock(&item.0).as_ref()
    {
        let _ = i.set_text(text);
        let _ = i.set_enabled(enabled);
    }
}

/// Tray "Sicherung jetzt": backup during operation (`pg_dump` + `document_exporter` to
/// `<home>/backups/<time>/`), afterwards the folder is opened.
fn start_backup(app: &AppHandle) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let Some(tx) = lock(&shared.backup_tx).clone() else {
        set_tooltip(app, "PaperRust – Sicherung erst nach dem Start möglich");
        return;
    };
    if shared.backup_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let (reply, rx) = tokio::sync::oneshot::channel();
    if tx.send(reply).is_err() {
        shared.backup_running.store(false, Ordering::SeqCst);
        return;
    }
    set_backup_item(app, "Sicherung läuft …", false);
    set_tooltip(app, "PaperRust – Sicherung läuft …");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let r = rx.await;
        shared.backup_running.store(false, Ordering::SeqCst);
        let ready = lock(&shared.backup_tx).is_some();
        set_backup_item(&app, BACKUP_LABEL, ready);
        match r {
            Ok(Ok(dir)) => {
                tracing::info!(dir = %dir.display(), "Sicherung fertig (Tray)");
                set_tooltip(&app, "PaperRust – Sicherung fertig");
                open_external(&dir.display().to_string());
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "Sicherung fehlgeschlagen (Tray)");
                set_tooltip(&app, "PaperRust – Sicherung fehlgeschlagen (siehe Logs)");
            }
            Err(_) => set_tooltip(&app, "PaperRust – Sicherung abgebrochen"),
        }
    });
}

/// Set the tray menu (new) – at start and after switching/renaming.
fn install_tray_menu(app: &AppHandle, mode: TrayMode) {
    let (profiles, active) = if mode == TrayMode::Remote {
        AppPaths::from_env()
            .map(|p| {
                let c = load_config(&p);
                (c.profiles.iter().map(|p| (p.id.clone(), p.name.clone())).collect::<Vec<_>>(), c.active_profile.clone())
            })
            .unwrap_or_default()
    } else {
        (Vec::new(), String::new())
    };
    // the running profile decides (not app.toml, which may already be changed before a restart)
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let app_ui = mode == TrayMode::Remote && lock(&shared.remote).as_ref().is_some_and(|r| sync::app_ui(&r.profile));
    let spec = tray::menu_spec(mode, &profiles, &active, app_ui);
    match tray::build_menu(app, &spec) {
        Ok(b) => {
            if let Some(t) = app.tray_by_id(TRAY) {
                let _ = t.set_menu(Some(b.menu));
            }
            if let Some(item) = app.try_state::<BackupItem>() {
                *lock(&item.0) = b.backup;
            }
            if let Some(item) = app.try_state::<UpdateItem>() {
                *lock(&item.0) = b.update;
            }
            refresh_update_view(app, &shared);
        }
        Err(e) => tracing::error!(error = %e, "Tray-Menü"),
    }
}

fn on_tray_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        design::MODERN_ID | design::CLASSIC_ID => {
            tray_design(app, if id == design::MODERN_ID { design::Design::Modern } else { design::Design::Classic })
        }
        uimode::SERVER_UI_ID => set_active_ui(app, mode::Ui::Server),
        uimode::MODE_ID => {
            if let Some(u) = local_url("settings.html#mode") {
                navigate_main(app, u);
            }
        }
        "backup" => start_backup(app),
        "consume" => open_dir(|p| p.consume.clone()),
        "backups" => open_dir(|p| p.backups.clone()),
        "logs" => open_dir(|p| p.logs.clone()),
        "updates" => {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            let (eff, _) = update_effective(app);
            if eff.enabled && lock(&shared.update).pending.is_none() {
                lock(&shared.update).checking = true;
                shared.update_now.notify_one();
            }
            open_update_window(app, true);
        }
        "settings" => {
            if let Some(u) = local_url("settings.html") {
                navigate_main(app, u);
            }
        }
        "quit" => {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            request_quit(app, &shared);
        }
        other => {
            if let Some(pid) = other.strip_prefix(PROFILE_PREFIX) {
                let shared = app.state::<Arc<Shared>>().inner().clone();
                if is_active_running(&shared, pid) {
                    install_tray_menu(app, TrayMode::Remote); // restore the tick mark
                    show_main(app);
                    return;
                }
                let r = with_profile(pid, |cfg, _| {
                    cfg.active_profile = pid.to_string();
                    cfg.mode = Mode::Remote;
                    Ok(())
                });
                match r {
                    Ok(()) => restart_app(app, &shared),
                    Err(e) => tracing::warn!("Profilwechsel: {e}"),
                }
            }
        }
    }
}

/// Tray "Neue/Klassische Oberfläche" in all modes: in remote mode the server kind and the
/// interface of the running profile decide ([`uimode::design_action`]); paperless-ngx has no
/// PaperRust interface of its own -> switch to the bundled one (restart).
fn tray_design(app: &AppHandle, target: design::Design) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let ctx = if shared.start() == Some(Start::Remote) { lock(&shared.remote).clone() } else { None };
    let Some(ctx) = ctx else {
        return switch_design(app, target);
    };
    match uimode::design_action(ctx.profile.flavor, ctx.profile.ui, sync::app_dir().is_some(), target) {
        uimode::DesignAction::Navigate => switch_design(app, target),
        uimode::DesignAction::ServerHome => {
            if let Ok(u) = Url::parse(&ctx.server.base) {
                navigate_main(app, u);
            }
        }
        uimode::DesignAction::UseAppUi => {
            remember_design(target);
            set_active_ui(app, mode::Ui::App);
        }
        uimode::DesignAction::Unavailable(note) => {
            tracing::warn!("Oberfläche nicht wechselbar: {note}");
            if let Some(u) = local_url("settings.html#ui") {
                navigate_main(app, u);
            }
        }
    }
}

/// Set the interface of the running remote profile (tray) and restart.
fn set_active_ui(app: &AppHandle, ui: mode::Ui) {
    let shared = app.state::<Arc<Shared>>().inner().clone();
    let Some(id) = lock(&shared.remote).as_ref().map(|r| r.profile.id.clone()) else {
        return;
    };
    let available = sync::app_dir().is_some();
    match with_profile(&id, |cfg, _| uimode::set_profile_ui(cfg, &id, ui, available)) {
        Ok(()) => {
            tracing::info!(profile = %id, app_ui = (ui == mode::Ui::App), "Oberfläche gewechselt (Tray)");
            restart_app(app, &shared);
        }
        Err(e) => {
            tracing::warn!("Oberfläche nicht gewechselt: {e}");
            if let Some(u) = local_url("settings.html#ui") {
                navigate_main(app, u);
            }
        }
    }
}

/// Tray "Neue/Klassische Oberfläche": main window to the same route of the other UI
/// (docs/DESIGN2.md §6). On local app pages (first start, error) only show the window.
fn switch_design(app: &AppHandle, target: design::Design) {
    remember_design(target);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window(MAIN)
            && let Ok(cur) = w.url()
            && let Some(u) = design::switch_url(&cur, target)
        {
            let _ = w.navigate(u);
        }
        show_main(&handle);
    });
}

fn build_tray(app: &AppHandle, mode: TrayMode) -> tauri::Result<()> {
    app.manage(BackupItem::default());
    app.manage(UpdateItem::default());
    let built = tray::build_menu(app, &tray::menu_spec(mode, &[], "", false))?;
    *lock(&app.state::<BackupItem>().0) = built.backup;
    *lock(&app.state::<UpdateItem>().0) = built.update;
    let menu = built.menu;
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("PaperRust – startet …")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| on_tray_menu(app, ev.id().as_ref()))
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    if mode == TrayMode::Remote {
        install_tray_menu(app, mode);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Supervisor thread (mode 1)

fn spawn_supervisor(app: AppHandle, shared: Arc<Shared>) {
    let for_thread = shared.clone();
    let spawned = std::thread::Builder::new().name("paperrust-supervisor".into()).spawn(move || {
        let shared = for_thread;
        let result = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt.block_on(supervise(&app, &shared)),
            Err(e) => Err(Failure::other(format!("Laufzeit: {e}"))),
        };
        *lock(&shared.backup_tx) = None;
        set_backup_item(&app, BACKUP_LABEL, false);
        shared.finished.store(true, Ordering::SeqCst);
        let ended = |app: &AppHandle| finish(app, &shared);
        match result {
            Ok(()) => ended(&app),
            Err(_) if shared.quitting.load(Ordering::SeqCst) => ended(&app),
            Err(f) => {
                tracing::error!("{}", f.message);
                shared.set_error(&f);
                set_tooltip(&app, "PaperRust – Fehler");
                show_error_page(&app);
            }
        }
    });
    if let Err(e) = spawned {
        shared.set_error(&Failure::other(format!("Supervisor-Thread: {e}")));
        shared.finished.store(true, Ordering::SeqCst);
    }
}

async fn supervise(app: &AppHandle, shared: &Arc<Shared>) -> Result<(), Failure> {
    let paths = AppPaths::from_env()?;
    paths.ensure_dirs()?;
    let stop_file = paths.stop_request_file();
    let layout = InstallLayout::from_current_exe().map_err(|e| Failure::other(format!("Programmpfad: {e}")))?;
    let shutdown = shared.shutdown.notified();
    // After a restart (mode switch) the old process may still hold the lock for a moment.
    for _ in 0..40 {
        match InstanceLock::acquire(&paths.lock_file(), &paths.instance_file()) {
            Ok(Acquire::Busy(Some(_))) | Err(_) => break, // really running (console) → supervisor shows it
            Ok(Acquire::Busy(None)) => tokio::time::sleep(Duration::from_millis(250)).await,
            Ok(Acquire::Acquired(l)) => {
                drop(l);
                break;
            }
        }
    }
    let run_dir = paths.run.clone();
    let sup = match Supervisor::open(paths, layout)? {
        Opened::Ready(s) => {
            // Stop from outside as its own task on the worker threads: it also works while
            // the supervisor waits in the first-start form (the `block_on` thread blocks there;
            // smoke test D1). Only start after `open` – `open` clears old stop requests.
            let watcher = shared.clone();
            tokio::spawn(async move {
                paperrust_desktop::lock::stop_requested(stop_file).await;
                tracing::info!("Stopp von außen angefordert");
                watcher.request_shutdown();
            });
            *s
        }
        // Another shell (e.g. console) already runs PaperRust: only display. Quitting
        // then closes only this window, not the foreign instance.
        Opened::AlreadyRunning(Some(url)) => {
            *lock(&shared.local_login) = Some((run_dir.clone(), url.clone()));
            shared.set_ready(&local_open_url(&run_dir, &url, &design_start_url(&url)));
            set_tooltip(app, "PaperRust (läuft in einem anderen Prozess)");
            shutdown.await;
            return Ok(());
        }
        Opened::AlreadyRunning(None) => {
            return Err(Failure::other(
                "PaperRust wird gerade von einem anderen Prozess gestartet. Bitte kurz warten und erneut versuchen.".into(),
            ));
        }
    };
    let log = paperrust_desktop::logs::init(&sup.paths.logs).map_err(|e| Failure::other(format!("Log öffnen: {e}")))?;
    tracing::info!(log = %log.display(), "PaperRust Desktop (Tauri) {}", env!("CARGO_PKG_VERSION"));
    for n in &sup.notices {
        tracing::warn!("{n}");
    }

    let prompt = FormPrompt { app: app.clone(), shared: shared.clone() };
    let (backup_tx, control) = control_channel();
    let on_event = |e: Event| match e {
        Event::Stage(s) => {
            shared.set_stage(s);
            set_tooltip(app, &format!("PaperRust – {s} …"));
        }
        Event::Ready { url } => {
            tracing::info!(%url, "PaperRust bereit");
            *lock(&shared.local_login) = Some((run_dir.clone(), url.clone()));
            shared.set_ready(&local_open_url(&run_dir, &url, &design_start_url(&url)));
            *lock(&shared.backup_tx) = Some(backup_tx.clone());
            if !shared.backup_running.load(Ordering::SeqCst) {
                set_backup_item(app, BACKUP_LABEL, true);
            }
            set_tooltip(app, TOOLTIP_READY);
        }
        Event::Restarting { attempt } => {
            tracing::warn!(attempt, "PaperRust-Server wird neu gestartet");
            set_tooltip(app, "PaperRust – Neustart …");
        }
        Event::SwitchCheck(c) => {
            if c.verdict != "RUNNING" {
                tracing::info!(verdict = %c.verdict, "Umstieg geprüft");
            }
            shared.set_switch(&c);
        }
        Event::BackupStarted => tracing::info!("Sicherung gestartet"),
        Event::Stopping => {
            *lock(&shared.backup_tx) = None;
            shared.set_stage("PaperRust wird beendet …");
            set_tooltip(app, "PaperRust – wird beendet …");
        }
    };
    sup.run_with_control(&prompt, &on_event, shutdown, control).await.map_err(Failure::from)
}

// ---------------------------------------------------------------------------------------------
// Remote mode (2a): start and connection monitoring

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Local time without a time zone library: the page formats itself (Unix ms as text).
fn now_label() -> String {
    now_ms().to_string()
}

/// Clean up profile folders: delete orphaned ones (removed profiles), execute logout markers.
fn cleanup_profiles(paths: &AppPaths, cfg: &AppConfig) {
    let Ok(rd) = std::fs::read_dir(paths.profiles_dir()) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        if !cfg.profiles.iter().any(|p| p.id == name) {
            tracing::info!(profile = %name, "verwaisten Profilordner entfernt");
            let _ = std::fs::remove_dir_all(&dir);
        } else if dir.join(LOGOUT_MARKER).exists() {
            let r = std::fs::remove_dir_all(dir.join("webview"));
            tracing::info!(profile = %name, ok = r.is_ok() || !dir.join("webview").exists(), "Abmelden: WebView-Daten gelöscht");
            let _ = std::fs::remove_file(dir.join(LOGOUT_MARKER));
        }
    }
}

/// Prepare remote mode (before the window): folder, lock, log, profile. Returns the
/// WebView data directory of the active profile.
fn prepare_remote(shared: &Arc<Shared>, paths: &AppPaths, cfg: &AppConfig) -> Result<Option<PathBuf>, Failure> {
    for d in [&paths.home, &paths.logs, &paths.run, &paths.profiles_dir()] {
        paperrust_desktop::perms::create_private_dir(d).map_err(|e| Failure::other(format!("Verzeichnis {} anlegen: {e}", d.display())))?;
    }
    // Single instance (also against a locally running console shell). After a restart the
    // old process may still hold the lock briefly.
    let mut acquired = None;
    for _ in 0..40 {
        match InstanceLock::acquire(&paths.lock_file(), &paths.instance_file()) {
            Ok(Acquire::Acquired(l)) => {
                acquired = Some(l);
                break;
            }
            Ok(Acquire::Busy(_)) => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e.into()),
        }
    }
    let l = acquired.ok_or_else(|| {
        Failure::other(
            "PaperRust läuft bereits in einem anderen Programm (z. B. lokal über die Konsole). Bitte dieses zuerst beenden.".into(),
        )
    })?;
    *lock(&shared.instance_lock) = Some(l);
    if let Ok(log) = paperrust_desktop::logs::init(&paths.logs) {
        tracing::info!(log = %log.display(), "PaperRust Desktop (Tauri) {} – Fern-Modus", env!("CARGO_PKG_VERSION"));
    }
    let _ = std::fs::remove_file(paths.stop_request_file());
    cleanup_profiles(paths, cfg);
    let Some(p) = cfg.active() else {
        lock(&shared.view).need_connect = true;
        return Ok(None);
    };
    let server = remote::parse_server_url(&p.url).map_err(Failure::other)?;
    *lock(&shared.nav) = NavPolicy::remote(&server.base, &p.extra_origins);
    {
        let mut v = lock(&shared.view);
        v.profile = Some(p.name.clone());
        v.insecure = !server.https;
        v.stage = format!("Verbindung zu {} …", p.name);
    }
    *lock(&shared.remote) = Some(RemoteCtx { profile: p.clone(), server, online: false, pending_sso: None });
    let dir = paths.profile_dir(&p.id);
    paperrust_desktop::perms::create_private_dir(&dir).map_err(|e| Failure::other(format!("Profilordner: {e}")))?;
    Ok(Some(dir.join("webview")))
}

fn set_offline(app: &AppHandle, shared: &Shared, error: String, last_ok: &Option<String>, wait: Duration) {
    let (name, url) = lock(&shared.remote)
        .as_mut()
        .map(|r| {
            r.online = false;
            (r.profile.name.clone(), r.server.base.clone())
        })
        .unwrap_or_default();
    {
        let mut v = lock(&shared.view);
        v.ready_url = None;
        v.error = None;
        v.offline = Some(OfflineView {
            profile: name.clone(),
            url,
            error,
            last_ok: last_ok.clone(),
            retry_at_ms: now_ms() + wait.as_millis() as u64,
        });
    }
    set_tooltip(app, &format!("PaperRust – offline ({name})"));
}

/// Store a newly detected server kind for a profile that had none (only `Unknown` is replaced).
fn remember_flavor(shared: &Shared, id: &str, flavor: mode::Flavor) {
    let r = with_profile(id, |cfg, _| {
        if let Some(p) = cfg.profile_mut(id).filter(|p| p.flavor == mode::Flavor::Unknown) {
            p.flavor = flavor;
        }
        Ok(())
    });
    if let Err(e) = r {
        tracing::warn!("Server-Art nicht gespeichert: {e}");
    }
    if let Some(c) = lock(&shared.remote).as_mut() {
        c.profile.flavor = flavor;
    }
}

/// Connection loop: first detect, then check every 15 s; offline → own page with
/// retry (15 s, doubled up to 2 min); "Erneut versuchen"/load error → immediately.
async fn remote_loop(app: AppHandle, shared: Arc<Shared>) {
    let Some(ctx) = lock(&shared.remote).clone() else {
        return;
    };
    let pin = (!ctx.profile.tls_pin.is_empty()).then(|| ctx.profile.tls_pin.clone());
    let fetcher = match Fetcher::new(&ctx.server.base, pin.as_deref()) {
        Ok(f) => f,
        Err(e) => {
            shared.set_error(&Failure::other(e));
            return;
        }
    };
    let mut attempt = 0u32;
    let mut detected = false;
    let mut last_ok: Option<String> = None;
    loop {
        let r = if detected {
            remote::probe(&fetcher).await.map_err(DetectError::Fetch)
        } else {
            remote::detect(&fetcher).await.map(|d| {
                tracing::info!(flavor = ?d.flavor, version = ?d.version, "Server erkannt");
                if d.flavor != ctx.profile.flavor && ctx.profile.flavor != mode::Flavor::Unknown {
                    tracing::warn!(was = ?ctx.profile.flavor, now = ?d.flavor, "Server-Art hat sich geändert");
                }
                // Profile without a known kind (environment, handwritten app.toml): remember it, so the
                // PaperRust interface can be chosen in the settings/tray
                if ctx.profile.flavor == mode::Flavor::Unknown && d.flavor != mode::Flavor::Unknown {
                    remember_flavor(&shared, &ctx.profile.id, d.flavor);
                }
            })
        };
        let wait = match r {
            Ok(()) => {
                detected = true;
                attempt = 0;
                last_ok = Some(now_label());
                let newly = lock(&shared.remote).as_mut().is_some_and(|c| {
                    let was = c.online;
                    c.online = true;
                    !was
                });
                if newly {
                    tracing::info!(url = %ctx.server.base, "verbunden");
                    {
                        let mut v = lock(&shared.view);
                        v.offline = None;
                        v.error = None;
                        v.advice = None;
                        // PaperRust server: most recently chosen UI (paperless-ngx knows only `/`)
                        v.ready_url = Some(if ctx.profile.flavor == mode::Flavor::Paperrust {
                            design_start_url(&ctx.server.base)
                        } else {
                            ctx.server.base.clone()
                        });
                    }
                    set_tooltip(&app, &format!("PaperRust – {} · Rechtsklick für Menü", ctx.profile.name));
                }
                Duration::from_secs(15)
            }
            Err(DetectError::Fetch(e @ (FetchError::PinMismatch { .. } | FetchError::Tls(..)))) => {
                // Certificate does not match (any more): no automatic continuing.
                tracing::error!("{e}");
                if let Some(c) = lock(&shared.remote).as_mut() {
                    c.online = false;
                }
                lock(&shared.view).offline = None;
                shared.set_error(&Failure {
                    message: e.to_string(),
                    advice: Some("Hat der Server ein neues Zertifikat bekommen, das Profil in den Einstellungen entfernen und neu verbinden (der neue Fingerabdruck wird dann angezeigt).".into()),
                    log: "desktop.log",
                });
                set_tooltip(&app, "PaperRust – Zertifikat abgelehnt");
                show_error_page(&app);
                Duration::from_secs(120)
            }
            Err(e) => {
                let wait = remote::backoff(attempt);
                attempt = attempt.saturating_add(1);
                let was_online = lock(&shared.remote).as_ref().is_some_and(|c| c.online);
                tracing::warn!(attempt, was_online, "Server nicht erreichbar: {e}");
                set_offline(&app, &shared, e.to_string(), &last_ok, wait);
                show_error_page(&app);
                wait
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shared.probe_now.notified() => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Automatic updates (Z4, [`update`], DESKTOP.md §23)

const UPDATE_WIN: &str = "update";
/// First search after the start – not while PostgreSQL/server are starting up.
const UPDATE_START_DELAY: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Serialize)]
struct PendingView {
    version: String,
    notes: String,
    pub_date: Option<String>,
    channel: &'static str,
    size: u64,
}

/// Update state for `update.html` and the settings (queried via `update_state`).
#[derive(Debug, Clone, Default, Serialize)]
struct UpdateView {
    current: String,
    enabled: bool,
    /// Why disabled (environment, no key, settings, invalid address).
    reason: Option<String>,
    settings_enabled: bool,
    channel: &'static str,
    channel_from_env: bool,
    interval_hours: u64,
    endpoint: String,
    endpoint_from_env: bool,
    checking: bool,
    last_check_ms: u64,
    last_error: Option<String>,
    pending: Option<PendingView>,
    /// Newer version found, but installable only by hand (portable/developer version).
    available: Option<String>,
    /// Installed with the installer (can update itself).
    installable: bool,
    /// Result of the last installer run.
    note: Option<String>,
    installing: bool,
}

fn update_store() -> Option<update::Store> {
    AppPaths::from_env().ok().map(|p| update::Store::new(&p.home))
}

fn update_effective(app: &AppHandle) -> (update::Effective, update::UpdateSettings) {
    let settings = AppPaths::from_env().map(|p| load_config(&p).updates).unwrap_or_default();
    let eff = update::effective(&settings, |k| std::env::var(k).ok(), &update::pubkey(app));
    (eff, settings)
}

fn update_installable() -> bool {
    update::install_dir().is_some_and(|d| update::installable(&d))
}

/// Rebuild the display from settings + store (running state like `checking` stays).
fn refresh_update_view(app: &AppHandle, shared: &Shared) {
    let (eff, settings) = update_effective(app);
    let store = update_store();
    let st = store.as_ref().map(|s| s.load_state()).unwrap_or_default();
    let pending = store.as_ref().and_then(|s| s.load_pending());
    let mut v = lock(&shared.update);
    v.current = app.package_info().version.to_string();
    v.enabled = eff.enabled;
    v.reason = eff.reason.clone();
    v.settings_enabled = settings.enabled;
    v.channel = eff.channel.as_str();
    v.channel_from_env = eff.channel_from_env;
    v.interval_hours = eff.interval.as_secs() / 3600;
    v.endpoint = eff.endpoint.clone();
    v.endpoint_from_env = eff.endpoint_from_env;
    v.last_check_ms = st.last_check_ms;
    v.last_error = st.last_error;
    v.installable = update_installable();
    v.pending = pending.filter(|_| eff.enabled).map(|p| PendingView {
        version: p.version,
        notes: p.notes,
        pub_date: p.pub_date,
        channel: p.channel.as_str(),
        size: p.size,
    });
    if v.pending.is_some() || !eff.enabled {
        v.available = None;
    }
    let label = match &v.pending {
        Some(p) => format!("Update auf {} installieren …", p.version),
        None => tray::UPDATE_LABEL.to_string(),
    };
    drop(v);
    set_update_item(app, &label);
}

/// Menu entry "Nach Updates suchen …" / "Update auf X installieren …".
#[derive(Default)]
struct UpdateItem(Mutex<Option<MenuItem<Wry>>>);

fn set_update_item(app: &AppHandle, text: &str) {
    if let Some(item) = app.try_state::<UpdateItem>()
        && let Some(i) = lock(&item.0).as_ref()
    {
        let _ = i.set_text(text);
    }
}

/// Open the update window (local page `update.html`, own small window – the
/// PaperRust UI in the main window stays as it is). Built on a
/// background thread: `build()` waits for the event loop – on the main thread (tray click,
/// synchronous command, `run_on_main_thread`) this blocked on Windows (E2E finding Z4).
fn open_update_window(app: &AppHandle, focus: bool) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(w) = handle.get_webview_window(UPDATE_WIN) {
            let _ = w.unminimize();
            let _ = w.show();
            if focus {
                let _ = w.set_focus();
            }
            return;
        }
        let built = WebviewWindowBuilder::new(&handle, UPDATE_WIN, WebviewUrl::App("update.html".into()))
            .title("PaperRust – Update")
            .inner_size(580.0, 600.0)
            .min_inner_size(420.0, 380.0)
            .center()
            .focused(focus)
            .on_navigation(|u| {
                if is_local_page(u) {
                    return true;
                }
                if matches!(u.scheme(), "http" | "https") {
                    open_external(u.as_str());
                }
                false
            })
            .build();
        match built {
            Ok(_) => tracing::info!("Update-Fenster geöffnet"),
            Err(e) => tracing::error!(error = %e, "Update-Fenster lässt sich nicht öffnen"),
        }
    });
}

/// Offer by itself (once per run and version).
fn offer_update(app: &AppHandle, shared: &Shared, version: &str) {
    let mut o = lock(&shared.update_offered);
    if o.as_deref() == Some(version) || shared.quitting.load(Ordering::SeqCst) {
        return;
    }
    *o = Some(version.to_string());
    drop(o);
    open_update_window(app, false);
}

/// Background: evaluate the result of an installer run, offer a ready update, search at
/// start (after [`UPDATE_START_DELAY`]) and then at the interval.
async fn update_loop(app: AppHandle, shared: Arc<Shared>) {
    let Some(store) = update_store() else {
        return;
    };
    let current = app.package_info().version.clone();
    let start_note = store.reconcile(&current);
    lock(&shared.update).note = match &start_note {
        update::StartNote::Installed(v) => Some(format!("PaperRust wurde auf {v} aktualisiert.")),
        update::StartNote::InstallFailed(v) => {
            Some(format!("Das Update auf {v} wurde nicht eingespielt (installiert ist weiterhin {current})."))
        }
        update::StartNote::None => None,
    };
    refresh_update_view(&app, &shared);
    let (eff, _) = update_effective(&app);
    if eff.enabled && store.load_pending().is_some() {
        let failed = store.load_state().failed_version;
        match update::verify_pending(&app, &store) {
            Ok((p, _)) if failed.as_deref() != Some(p.version.as_str()) => {
                offer_update(&app, &shared, &p.version);
            }
            Ok(_) => {}
            Err(e) => store.update_state(|s| s.last_error = Some(e)),
        }
        refresh_update_view(&app, &shared);
    }
    let mut forced = tokio::select! {
        _ = tokio::time::sleep(UPDATE_START_DELAY) => false,
        _ = shared.update_now.notified() => true,
    };
    // Only log now: locally the supervisor sets up the log on startup.
    match &start_note {
        update::StartNote::Installed(v) => tracing::info!("Update auf {v} eingespielt"),
        update::StartNote::InstallFailed(v) => {
            tracing::warn!("Update auf {v} wurde nicht eingespielt (Version weiterhin {current})")
        }
        update::StartNote::None => {}
    }
    let mut first = true;
    loop {
        let (eff, _) = update_effective(&app);
        let last = store.load_state().last_check_ms;
        if eff.enabled && (forced || update::due(first, last, now_ms(), eff.interval)) {
            first = false;
            lock(&shared.update).checking = true;
            let o = update::check(&app, &store, &eff, &current, update_installable(), now_ms()).await;
            store.update_state(|s| {
                s.last_check_ms = now_ms();
                s.last_error = match &o {
                    update::Outcome::Failed(e) => Some(e.clone()),
                    _ => None,
                };
                match &o {
                    update::Outcome::Available(r) => s.latest_seen = Some(r.version.to_string()),
                    update::Outcome::Ready(p) => s.latest_seen = Some(p.version.clone()),
                    _ => {}
                }
            });
            let failed = store.load_state().failed_version;
            {
                let mut v = lock(&shared.update);
                v.checking = false;
                v.available = match &o {
                    update::Outcome::Available(r) => Some(r.version.to_string()),
                    _ => None,
                };
            }
            match &o {
                update::Outcome::UpToDate => {
                    tracing::info!(channel = eff.channel.as_str(), "Kein Update verfügbar")
                }
                update::Outcome::Available(r) => tracing::info!(
                    version = %r.version,
                    "Neue Version verfügbar (diese Installation aktualisiert sich nicht selbst)"
                ),
                update::Outcome::Ready(p) => {
                    tracing::info!(version = %p.version, "Update bereit");
                    if forced || failed.as_deref() != Some(p.version.as_str()) {
                        offer_update(&app, &shared, &p.version);
                    }
                }
                update::Outcome::Failed(e) => tracing::warn!("Update-Suche: {e}"),
            }
        } else {
            lock(&shared.update).checking = false;
        }
        refresh_update_view(&app, &shared);
        // Check for being due at least hourly (wall clock: after standby the schedule is right).
        let wait = if eff.enabled {
            let since = now_ms().saturating_sub(store.load_state().last_check_ms);
            let left = (eff.interval.as_millis() as u64).saturating_sub(since);
            Duration::from_millis(left.clamp(60_000, 3_600_000))
        } else {
            Duration::from_secs(3600)
        };
        forced = tokio::select! {
            _ = tokio::time::sleep(wait) => false,
            _ = shared.update_now.notified() => true,
        };
    }
}

#[tauri::command]
fn update_state(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> UpdateView {
    if lock(&shared.update).current.is_empty() {
        refresh_update_view(&app, &shared);
    }
    lock(&shared.update).clone()
}

/// "Jetzt suchen".
#[tauri::command]
fn update_check(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let (eff, _) = update_effective(&app);
    if let Some(r) = eff.reason {
        return Err(format!("Updates sind abgeschaltet: {r}"));
    }
    {
        let mut v = lock(&shared.update);
        v.checking = true;
        v.note = None;
    }
    shared.update_now.notify_one();
    Ok(())
}

/// "Jetzt neu starten und installieren": check the file again, then quit in an orderly way (locally:
/// stop server + PostgreSQL) and start the installer ([`finish`]).
#[tauri::command]
fn update_install(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Result<(), String> {
    let (eff, _) = update_effective(&app);
    if let Some(r) = eff.reason {
        return Err(format!("Updates sind abgeschaltet: {r}"));
    }
    let dir = update::install_dir()
        .filter(|d| update::installable(d))
        .ok_or("Diese Installation kann sich nicht selbst aktualisieren (portable oder Entwickler-Version). Bitte die neue Version von Hand installieren.")?;
    if shared.backup_running.load(Ordering::SeqCst) {
        return Err("Eine Sicherung läuft gerade. Bitte danach erneut versuchen.".into());
    }
    if lock(&shared.install_update).is_some() {
        return Err("Das Update wird bereits eingespielt.".into());
    }
    let store = update_store().ok_or("Datenordner unbekannt.")?;
    let (p, installer) = match update::verify_pending(&app, &store) {
        Ok(x) => x,
        Err(e) => {
            tracing::error!("Update: {e}");
            store.update_state(|s| s.last_error = Some(e.clone()));
            refresh_update_view(&app, &shared);
            return Err(e);
        }
    };
    let current = app.package_info().version.to_string();
    store.update_state(|s| {
        s.installing = Some(update::Installing { version: p.version.clone(), from: current.clone(), started_ms: now_ms() });
        s.last_error = None;
    });
    tracing::info!(version = %p.version, from = %current, "Update wird eingespielt – PaperRust wird beendet");
    *lock(&shared.install_update) = Some((installer, dir));
    lock(&shared.update).installing = true;
    let app2 = app.clone();
    let shared2 = shared.inner().clone();
    tauri::async_runtime::spawn(async move {
        // Still deliver the command's answer to the page.
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(w) = app2.get_webview_window(UPDATE_WIN) {
            let _ = w.hide();
        }
        request_quit(&app2, &shared2);
    });
    Ok(())
}

/// "Später": close the window; asked again at the next start.
#[tauri::command]
fn update_later(app: AppHandle) {
    if let Some(w) = app.get_webview_window(UPDATE_WIN) {
        let _ = w.close();
    }
}

/// Settings → updates (`app.toml` `[updates]`).
#[tauri::command]
fn update_settings_save(
    enabled: bool,
    channel: String,
    interval_hours: u32,
    app: AppHandle,
    shared: tauri::State<'_, Arc<Shared>>,
) -> Result<(), String> {
    if shared.start() == Some(Start::FirstRun) {
        return Err("Bitte zuerst die Einrichtung abschließen.".into());
    }
    let ch = update::Channel::parse(&channel).ok_or("Unbekannter Kanal.")?;
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let mut cfg = load_config(&paths);
    cfg.updates.enabled = enabled;
    cfg.updates.channel = ch;
    cfg.updates.interval_hours = interval_hours.clamp(1, 720);
    save_config(&paths, &cfg)?;
    tracing::info!(enabled, channel = ch.as_str(), interval_hours, "Update-Einstellungen gespeichert");
    refresh_update_view(&app, &shared);
    if update_effective(&app).0.enabled {
        lock(&shared.update).checking = true;
        shared.update_now.notify_one();
    }
    Ok(())
}

#[tauri::command]
fn open_update(app: AppHandle) {
    open_update_window(&app, true);
}

/// Starting point per mode (in Tauri's `setup`, before the window).
fn begin(app: &AppHandle, shared: &Arc<Shared>) -> tauri::Result<()> {
    let paths = AppPaths::from_env();
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut cfg_opt = paths.as_ref().ok().and_then(|p| {
        AppConfig::load(&p.app_config_file())
            .unwrap_or_else(|e| {
                eprintln!("PaperRust: {e}");
                None
            })
            .map(|(c, _)| c)
    });
    // Without a form: server address from the environment → create/activate profile.
    let env_url = get(ENV_SERVER_URL);
    let env_mode = get(ENV_MODE).and_then(|m| Mode::parse(&m)).or(env_url.as_ref().map(|_| Mode::Remote));
    if let (Some(url), Ok(p)) = (&env_url, &paths)
        && env_mode == Some(Mode::Remote)
    {
        match env_profile(url, cfg_opt.clone().unwrap_or_default()) {
            Ok(c) => {
                if let Err(e) = save_config(p, &c) {
                    eprintln!("PaperRust: {e}");
                }
                cfg_opt = Some(c);
            }
            Err(e) => eprintln!("PaperRust: {ENV_SERVER_URL}: {e}"),
        }
    }
    // `setup_from_env` also covers `PAPERRUST_DESKTOP_ADMIN_*`.
    let managed_local = setup_from_env(|k| std::env::var(k).ok()).is_some();
    let local_data = paths.as_ref().is_ok_and(|p| p.config_file.exists() || p.pgdata.exists());
    let start = mode::decide(cfg_opt.as_ref(), env_mode, managed_local, local_data);
    *lock(&shared.start) = Some(start);
    let can_migrate_db = InstallLayout::from_current_exe().is_ok_and(|l| l.migrate_db_bin.is_some());
    lock(&shared.view).app_ui_available = sync::app_dir().is_some();
    match start {
        Start::Local => {
            lock(&shared.view).mode = "local".into();
            build_tray(app, TrayMode::Local)?;
            create_main(app, None)?;
            spawn_supervisor(app.clone(), shared.clone());
        }
        Start::FirstRun => {
            {
                let mut v = lock(&shared.view);
                v.mode = "first".into();
                v.need_first_start = true;
                v.can_migrate_db = can_migrate_db;
                v.stage = "Willkommen".into();
            }
            shared.finished.store(true, Ordering::SeqCst);
            build_tray(app, TrayMode::FirstRun)?;
            set_tooltip(app, "PaperRust – Einrichtung");
            create_main(app, None)?;
        }
        Start::Remote => {
            lock(&shared.view).mode = "remote".into();
            shared.finished.store(true, Ordering::SeqCst);
            build_tray(app, TrayMode::Remote)?;
            let data_dir = match paths
                .map_err(Failure::from)
                .and_then(|p| prepare_remote(shared, &p, &cfg_opt.clone().unwrap_or_default()).map(|d| (d, p)))
            {
                Ok((d, p)) => {
                    let stop_file = p.stop_request_file();
                    let watcher = shared.clone();
                    let app2 = app.clone();
                    tauri::async_runtime::spawn(async move {
                        paperrust_desktop::lock::stop_requested(stop_file).await;
                        tracing::info!("Stopp von außen angefordert");
                        request_quit(&app2, &watcher);
                    });
                    d
                }
                Err(f) => {
                    shared.set_error(&f);
                    None
                }
            };
            create_main(app, data_dir)?;
            if lock(&shared.remote).is_some() {
                let ctx = lock(&shared.remote).clone();
                if ctx.as_ref().is_some_and(|c| sync::app_ui(&c.profile)) {
                    // P5/2b: app UI (light UI) via prapi – online/offline is decided by
                    // the client core, no server page in the window (REMOTE-APP §1/§7)
                    set_tooltip(app, "PaperRust – App-Oberfläche");
                    *lock(&shared.nav) = NavPolicy::remote(sync::APP_ORIGIN_URL, &[]);
                    lock(&shared.view).ready_url = Some(uimode::app_start_url(sync::APP_ORIGIN_URL, preferred_design()));
                } else {
                    set_tooltip(app, "PaperRust – verbindet …");
                    tauri::async_runtime::spawn(remote_loop(app.clone(), shared.clone()));
                }
                // P5: client core for profiles with `sync = true` / `ui = "app"` (PaperRust)
                if let (Some(ctx), Ok(p)) = (ctx, AppPaths::from_env()) {
                    sync::start(app, &p, &ctx.profile);
                }
            } else {
                set_tooltip(app, "PaperRust – kein Server eingerichtet");
            }
        }
    }
    tauri::async_runtime::spawn(update_loop(app.clone(), shared.clone()));
    Ok(())
}

/// Profile from `PAPERRUST_DESKTOP_SERVER_URL` (http only with `PAPERRUST_DESKTOP_ALLOW_HTTP=1` and
/// only on the own network; self-signed certificates need the dialog).
fn env_profile(url: &str, mut cfg: AppConfig) -> Result<AppConfig, String> {
    let su = remote::parse_server_url(url)?;
    if remote::http_policy(&su)? && std::env::var(ENV_ALLOW_HTTP).ok().as_deref() != Some("1") {
        return Err(format!("http nur mit {ENV_ALLOW_HTTP}=1"));
    }
    if let Some(p) = cfg.profiles.iter().find(|p| p.url == su.base) {
        cfg.active_profile = p.id.clone();
    } else {
        let id = cfg.upsert(Profile {
            id: String::new(),
            name: mode::clean_name("", &su.base),
            url: su.base.clone(),
            flavor: mode::Flavor::Unknown,
            ui: mode::Ui::Server,
            tls_pin: String::new(),
            allow_http: !su.https,
            extra_origins: vec![],
            sync: false,
        });
        cfg.active_profile = id;
    }
    cfg.mode = Mode::Remote;
    Ok(cfg)
}

/// Own data folder (`PAPERRUST_DESKTOP_HOME`: portable, tests) → own identifier for
/// single instance and WebView default folder (`%LOCALAPPDATA%\<identifier>\EBWebView`). Otherwise a
/// portable installation or a test run would only bring the window of the installed app to the front
/// (same identifier = same single-instance lock) and share its WebView data.
/// Without override: unchanged `ch.paperrust.desktop`.
fn identifier_suffix(home: Option<&str>) -> Option<String> {
    let h = home?.trim();
    if h.is_empty() {
        return None;
    }
    let norm = h.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    Some(format!(".h{}", &remote::sha256_hex(norm.as_bytes())[..10]))
}

fn main() {
    // Helper when installing an update (own copy under `updates/`): no Tauri, no job.
    if let Some(code) = update::helper_main() {
        std::process::exit(code);
    }
    wait_for_predecessor();
    // Child processes (PostgreSQL, server, WebView2) end with the app – also on a crash.
    if let Err(e) = paperrust_desktop::job::kill_children_with_app() {
        eprintln!("PaperRust: Job-Objekt: {e}");
    }
    let shared = Arc::new(Shared::default());
    shared.set_stage("Wird gestartet");
    let app = tauri::Builder::default()
        // Must be the first plugin: a second start ends here and brings the window to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        // Updater: only as a library (signature check, download); its IPC commands are not enabled in any
        // capability. Public key: tauri.conf.json → plugins.updater.pubkey.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(shared.clone())
        // P5: client core + `prapi://` (own pages only, check in prapi::precheck)
        .manage(prapi::PrapiState::default())
        .manage(sync::SyncState::default())
        .register_asynchronous_uri_scheme_protocol(prapi::SCHEME, prapi::handler)
        .invoke_handler(tauri::generate_handler![
            shell_state,
            submit_setup,
            first_start_local,
            cancel_admin,
            quit,
            open_log,
            retry,
            server_connect,
            settings_state,
            settings_mode_plan,
            settings_set_mode,
            profile_activate,
            profile_rename,
            profile_logout,
            profile_remove,
            open_settings,
            close_settings,
            begin_connect,
            cancel_connect,
            sso_decide,
            update_state,
            update_check,
            update_install,
            update_later,
            update_settings_save,
            open_update,
            // P5: sync card of the settings page (rest via prapi …/local/…)
            sync::sync_profile_state,
            sync::sync_profile_set,
            sync::sync_profile_set_ui
        ])
        .on_window_event(|window, event| {
            // Closing = hiding: PaperRust keeps running in the tray (inbox folder, mail fetch).
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            begin(app.handle(), &shared)?;
            Ok(())
        })
        .build({
            let mut ctx = tauri::generate_context!();
            if let Some(sfx) = identifier_suffix(std::env::var(paperrust_desktop::paths::HOME_ENV).ok().as_deref()) {
                ctx.config_mut().identifier.push_str(&sfx);
            }
            ctx
        });
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            eprintln!("PaperRust: Fenster/Tray konnten nicht starten: {e}");
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        if let RunEvent::ExitRequested { code, api, .. } = event {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            if code.is_none() {
                // Last window closed (rarely happens because of "hiding"): keep running in the tray.
                api.prevent_exit();
                return;
            }
            if !shared.finished.load(Ordering::SeqCst) {
                // First stop server and PostgreSQL in an orderly way; the supervisor thread afterwards calls
                // `exit` itself.
                api.prevent_exit();
                request_quit(app, &shared);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_per_home_override() {
        assert_eq!(identifier_suffix(None), None, "installierte App: Kennung unverändert");
        assert_eq!(identifier_suffix(Some(" ")), None);
        let a = identifier_suffix(Some(r"C:\PaperRust\data")).unwrap();
        assert_eq!(a, identifier_suffix(Some("c:/paperrust/data/")).unwrap());
        assert_ne!(a, identifier_suffix(Some(r"D:\other")).unwrap());
        assert!(a.starts_with(".h") && a.len() == 12);
    }

    #[test]
    fn shutdown_answers_open_form() {
        let s = Shared::default();
        let (tx, rx) = mpsc::channel();
        *lock(&s.form_tx) = Some(tx);
        s.request_shutdown();
        assert!(rx.recv().unwrap().is_none());
        assert!(s.quitting.load(Ordering::SeqCst));
    }

    #[test]
    fn ready_sets_port_and_reset_clears() {
        let s = Shared::default();
        let u = |s: &str| Url::parse(s).unwrap();
        s.set_ready("http://127.0.0.1:58080/");
        assert!(lock(&s.nav).allows(&u("http://127.0.0.1:58080/dashboard")));
        assert!(!lock(&s.nav).allows(&u("http://127.0.0.1:58081/")));
        assert_eq!(lock(&s.view).ready_url.as_deref(), Some("http://127.0.0.1:58080/"));
        s.set_error(&Failure::other("kaputt".into()));
        assert_eq!(lock(&s.view).error_log.as_deref(), Some("desktop.log"));
        s.finished.store(true, Ordering::SeqCst);
        lock(&s.view).mode = "local".into();
        s.reset();
        let v = lock(&s.view).clone();
        assert!(v.error.is_none() && v.ready_url.is_none());
        assert_eq!(v.mode, "local", "Betriebsart bleibt");
        assert!(!lock(&s.nav).allows(&u("http://127.0.0.1:58080/")));
        assert!(!s.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn form_fields_to_choice() {
        let f = |mode: &str, source: &str, media: &str| SetupForm { mode: mode.into(), source: source.into(), media: media.into() };
        let c = setup_choice(f("import", " \"C:\\Export\\paperless.zip\" ", "")).unwrap();
        assert!(matches!(c, SetupChoice::ImportExport { ref source } if source == &PathBuf::from("C:\\Export\\paperless.zip")));
        let c = setup_choice(f("migrate-db", "sqlite:///C:/p/db.sqlite3", " ")).unwrap();
        assert!(matches!(c, SetupChoice::MigrateDb { media_dir: None, .. }));
        assert!(setup_choice(f("import", "", "")).is_err());
        assert!(setup_choice(f("x", "", "")).is_err());
        // "Start fresh" without user name/password: owner automatic
        let c = setup_choice(f("fresh", "", "")).unwrap();
        assert!(matches!(c, SetupChoice::Fresh(None)));
    }

    #[test]
    fn local_start_and_relogin_use_the_token() {
        let dir = std::env::temp_dir().join(format!("paperrust-shell-login-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = "http://127.0.0.1:58080/";
        // without key: normal start address, no automatic login
        assert_eq!(local_open_url(&dir, base, base), base);
        let token = paperrust_desktop::autologin::generate_token();
        paperrust_desktop::autologin::write_token(&dir, &token).unwrap();
        assert_eq!(
            local_open_url(&dir, base, "http://127.0.0.1:58080/rs/app/m/"),
            format!("http://127.0.0.1:58080/rs/desktop/login?token={token}&next=%2Frs%2Fapp%2Fm%2F")
        );
        let s = Shared::default();
        let u = |x: &str| Url::parse(x).unwrap();
        let login = u("http://127.0.0.1:58080/accounts/login/?next=/rs/app/documents");
        // before "ready" (or remote mode): intercept nothing
        assert!(desktop_relogin(&s, &login).is_none());
        *lock(&s.local_login) = Some((dir.clone(), base.into()));
        let t = desktop_relogin(&s, &login).unwrap();
        assert_eq!(t.path(), "/rs/desktop/login");
        assert!(t.query().unwrap().contains("next=%2Frs%2Fapp%2Fdocuments"));
        // other pages/origins not
        assert!(desktop_relogin(&s, &u("http://127.0.0.1:58080/rs/app/")).is_none());
        assert!(desktop_relogin(&s, &u("https://docs.example.ch/accounts/login/")).is_none());
        // loop protection: at most 3 in a short time
        assert!(desktop_relogin(&s, &login).is_some());
        assert!(desktop_relogin(&s, &login).is_some());
        assert!(desktop_relogin(&s, &login).is_none());
        // "Erneut versuchen" forgets the local operation
        s.reset();
        assert!(lock(&s.local_login).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failure_from_error_has_log_and_advice() {
        let f = Failure::from(paperrust_desktop::DesktopError::Postgres("kaputt".into()));
        assert_eq!(f.log, "postgresql.log");
        assert!(f.advice.is_some());
        let f = Failure::from(paperrust_desktop::DesktopError::Server("manage migrate fehlgeschlagen; Details in logs/manage.log".into()));
        assert_eq!(f.log, "manage.log");
    }

    #[test]
    fn pending_setup_answers_supervisor_once() {
        // First-start form before the supervisor: its question is answered without a second form.
        let s = Arc::new(Shared::default());
        *lock(&s.pending_setup) = Some(SetupChoice::ImportExport { source: PathBuf::from("C:\\x.zip") });
        let taken = lock(&s.pending_setup).take();
        assert!(matches!(taken, Some(SetupChoice::ImportExport { .. })));
        assert!(lock(&s.pending_setup).is_none());
    }

    #[test]
    fn env_profile_rules() {
        let c = env_profile("https://docs.example.ch", AppConfig::default()).unwrap();
        assert_eq!(c.mode, Mode::Remote);
        assert_eq!(c.active().unwrap().url, "https://docs.example.ch/");
        // same address → no second profile
        let c2 = env_profile("https://docs.example.ch/", c.clone()).unwrap();
        assert_eq!(c2.profiles.len(), 1);
        assert!(env_profile("http://example.ch", AppConfig::default()).is_err(), "öffentlich nie");
        // private without ALLOW_HTTP → rejected (variable is not set in the test run)
        if std::env::var(ENV_ALLOW_HTTP).is_err() {
            assert!(env_profile("http://192.168.1.2:8000", AppConfig::default()).is_err());
        }
    }

    #[test]
    fn window_title_warns_on_http() {
        let s = Shared::default();
        assert_eq!(window_title(&s), "PaperRust");
        let server = remote::parse_server_url("http://192.168.1.20:8000").unwrap();
        *lock(&s.remote) = Some(RemoteCtx {
            profile: Profile {
                id: "a".into(),
                name: "NAS".into(),
                url: server.base.clone(),
                flavor: mode::Flavor::Ngx,
                ui: mode::Ui::Server,
                tls_pin: String::new(),
                allow_http: true,
                extra_origins: vec![],
                sync: false,
            },
            server,
            online: false,
            pending_sso: None,
        });
        assert!(window_title(&s).contains("unverschlüsselt"));
    }

    #[test]
    fn profile_cleanup_removes_orphans_and_logs_out() {
        let dir = std::env::temp_dir().join(format!("p1-clean-{}", std::process::id()));
        let paths = AppPaths::under(&dir);
        let keep = paths.profile_dir("keep");
        let out = paths.profile_dir("out");
        let orphan = paths.profile_dir("orphan");
        for d in [&keep, &out, &orphan] {
            std::fs::create_dir_all(d.join("webview")).unwrap();
        }
        std::fs::write(out.join(LOGOUT_MARKER), "x").unwrap();
        let mk = |id: &str| Profile {
            id: id.into(),
            name: id.into(),
            url: "https://a.example/".into(),
            flavor: mode::Flavor::Ngx,
            ui: mode::Ui::Server,
            tls_pin: String::new(),
            allow_http: false,
            extra_origins: vec![],
            sync: false,
        };
        let cfg = AppConfig {
            mode: Mode::Remote,
            active_profile: "keep".into(),
            profiles: vec![mk("keep"), mk("out")],
            updates: Default::default(),
            ui: Default::default(),
        };
        cleanup_profiles(&paths, &cfg);
        assert!(keep.join("webview").is_dir());
        assert!(out.is_dir() && !out.join("webview").exists() && !out.join(LOGOUT_MARKER).exists());
        assert!(!orphan.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
