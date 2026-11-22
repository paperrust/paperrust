//! Interface choice in remote mode and the "change mode" plan (docs/DESKTOP.md §26).
//!
//! * In remote mode the window shows either the **server's own interface** (mode 2a, `ui = "server"`)
//!   or the **bundled PaperRust interface** served by the app via `prapi` (mode 2b, `ui = "app"`,
//!   talks to the server's REST API through the client core). A paperless-ngx server has no
//!   PaperRust interface of its own, so "new interface" there means: switch to the bundled one.
//! * "Change mode" (settings page, tray): local ↔ connect to a server. Only `mode` in `app.toml`
//!   changes; profiles, the local data (`pgdata/`, media) and the UI preference stay untouched.
//!
//! Pure functions only (unit-tested); the Tauri glue lives in `main.rs`.

use serde::Serialize;

use crate::design::Design;
use crate::mode::{AppConfig, Flavor, Mode, Start, Ui};

/// Menu ID / label: back from the bundled interface to the server's own one.
pub const SERVER_UI_ID: &str = "ui-server";
pub const SERVER_UI_LABEL: &str = "Oberfläche des Servers";
/// Menu ID / label: settings page, scrolled to the mode card.
pub const MODE_ID: &str = "mode";
pub const MODE_LABEL: &str = "Betriebsart wechseln …";

/// Note when the package lacks the bundled interface (`light\`).
pub const NO_APP_UI: &str = "Die PaperRust-Oberfläche ist in diesem Paket nicht enthalten (Ordner light\\ fehlt).";
/// Note when the server kind is not known yet.
pub const UNKNOWN_FLAVOR: &str = "Server-Art noch unbekannt – bitte einmal verbinden, dann erneut versuchen.";

/// What the tray entries "Neue Oberfläche" / "Klassische Oberfläche" do in the running remote mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignAction {
    /// Main window to the same route of the other design ([`crate::design::switch_url`]), on the
    /// server (PaperRust) or inside the bundled interface (prapi).
    Navigate,
    /// Main window to the start page of the server (paperless-ngx: its own UI is the classic one).
    ServerHome,
    /// Switch the active profile to the bundled interface (`ui = "app"`) and restart.
    UseAppUi,
    /// Not possible; show the note (settings page).
    Unavailable(&'static str),
}

/// Decide the tray design switch for the active profile.
pub fn design_action(flavor: Flavor, ui: Ui, app_available: bool, target: Design) -> DesignAction {
    match (ui, flavor) {
        // bundled interface: both designs are local routes (`/rs/app/…`, `/rs/app/m/…`)
        (Ui::App, _) => DesignAction::Navigate,
        // PaperRust server serves both designs itself
        (Ui::Server, Flavor::Paperrust) => DesignAction::Navigate,
        // paperless-ngx: classic = its own UI; the new design only exists in the app
        (Ui::Server, Flavor::Ngx) => match target {
            Design::Classic => DesignAction::ServerHome,
            Design::Modern if app_available => DesignAction::UseAppUi,
            Design::Modern => DesignAction::Unavailable(NO_APP_UI),
        },
        (Ui::Server, Flavor::Unknown) => DesignAction::Unavailable(UNKNOWN_FLAVOR),
    }
}

/// Start address of the bundled interface on the prapi origin (`…/`) according to the design:
/// classic light UI `rs/app/`, PaperRust design `rs/app/m/`.
pub fn app_start_url(origin: &str, design: Design) -> String {
    let base = if origin.ends_with('/') { origin.to_string() } else { format!("{origin}/") };
    match design {
        Design::Classic => format!("{base}rs/app/"),
        Design::Modern => format!("{base}rs/app/m/"),
    }
}

/// Set the interface of a profile (settings card, tray). Checks that the bundled interface
/// exists and the server kind is known; other profile fields stay unchanged.
pub fn set_profile_ui(cfg: &mut AppConfig, id: &str, ui: Ui, app_available: bool) -> Result<(), String> {
    let p = cfg.profiles.iter_mut().find(|p| p.id == id).ok_or("Unbekanntes Profil.")?;
    if ui == Ui::App {
        if !app_available {
            return Err(NO_APP_UI.into());
        }
        if p.flavor == Flavor::Unknown {
            return Err(UNKNOWN_FLAVOR.into());
        }
    }
    p.ui = ui;
    Ok(())
}

/// Interface for a newly connected profile (connect form): `"app"` only if the package has it and
/// the server kind is known, otherwise the server's own interface.
pub fn ui_for_new_profile(requested: Option<&str>, flavor: Flavor, app_available: bool) -> Ui {
    match requested {
        Some("app") if app_available && flavor != Flavor::Unknown => Ui::App,
        _ => Ui::Server,
    }
}

/// What a mode switch will do (settings page: confirmation text before applying).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SwitchPlan {
    /// Target mode (`local` | `remote`).
    pub target: &'static str,
    /// Already in this mode (nothing to do).
    pub unchanged: bool,
    /// Blocked (e.g. fixed by the environment) – reason; the switch is refused.
    pub blocked: Option<String>,
    /// After the restart: `local` (open local data), `local_setup` (no local data yet → first-start
    /// choice), `remote` (connect to the active profile), `connect` (no profile → address form).
    pub next: &'static str,
    /// Name of the profile used after switching to remote.
    pub profile: Option<String>,
    /// Text for the confirmation dialog.
    pub confirm: String,
}

/// Plan a switch to `target`. `running` = mode of this run, `env_mode` =
/// `PAPERRUST_DESKTOP_MODE` (wins over `app.toml`, so a switch would have no effect),
/// `local_data` = `desktop.toml`/`pgdata/` present.
pub fn plan_switch(cfg: &AppConfig, running: Option<Start>, env_mode: Option<Mode>, target: Mode, local_data: bool) -> SwitchPlan {
    let running_mode = match running {
        Some(Start::Local) => Some(Mode::Local),
        Some(Start::Remote) => Some(Mode::Remote),
        _ => None,
    };
    let unchanged = running_mode == Some(target) && cfg.mode == target;
    let blocked = env_mode
        .filter(|m| *m != target)
        .map(|m| format!("Die Betriebsart ist durch die Umgebungsvariable PAPERRUST_DESKTOP_MODE={} festgelegt.", m.as_str()));
    let profile = cfg.active().map(|p| p.name.clone());
    let (next, confirm) = match target {
        Mode::Local if local_data => (
            "local",
            "Zu „Auf diesem Computer“ wechseln?\n\nPaperRust startet neu und öffnet deine lokalen Daten. \
             Die Server-Profile bleiben gespeichert – zurück geht es jederzeit über Einstellungen → Betriebsart."
                .to_string(),
        ),
        Mode::Local => (
            "local_setup",
            "Zu „Auf diesem Computer“ wechseln?\n\nAuf diesem Computer gibt es noch keine Daten: Nach dem Neustart fragt \
             PaperRust, wie du beginnen möchtest. Die Server-Profile bleiben gespeichert."
                .to_string(),
        ),
        Mode::Remote => {
            let keep = if local_data {
                " Deine lokalen Daten (Datenbank, Dokumente) bleiben unverändert auf diesem Computer – sie werden nur \
                 nicht gestartet."
            } else {
                ""
            };
            match &profile {
                Some(name) => (
                    "remote",
                    format!(
                        "Zu „Mit einem Server verbinden“ ({name}) wechseln?\n\nPaperRust startet neu.{keep} \
                         Zurück geht es jederzeit über Einstellungen → Betriebsart."
                    ),
                ),
                None => (
                    "connect",
                    format!(
                        "Zu „Mit einem Server verbinden“ wechseln?\n\nPaperRust startet neu und fragt nach der Adresse des \
                         Servers.{keep} Zurück geht es jederzeit über Einstellungen → Betriebsart."
                    ),
                ),
            }
        }
    };
    SwitchPlan { target: target.as_str(), unchanged, blocked, next, profile, confirm }
}

/// Apply the switch to the configuration: only `mode` changes (profiles, active profile, UI
/// preference and updates stay as they are – nothing is deleted).
pub fn apply_switch(cfg: &mut AppConfig, target: Mode) {
    cfg.mode = target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::{Profile, UiPrefs};

    fn profile(id: &str, flavor: Flavor, ui: Ui) -> Profile {
        Profile {
            id: id.into(),
            name: "Büro".into(),
            url: "https://docs.example.ch/".into(),
            flavor,
            ui,
            tls_pin: String::new(),
            allow_http: false,
            extra_origins: vec![],
            sync: false,
        }
    }

    #[test]
    fn tray_design_switch_per_server_kind() {
        use DesignAction::*;
        // PaperRust server: both designs on the server
        assert_eq!(design_action(Flavor::Paperrust, Ui::Server, true, Design::Modern), Navigate);
        assert_eq!(design_action(Flavor::Paperrust, Ui::Server, false, Design::Classic), Navigate);
        // paperless-ngx: new design only from the app, classic = ngx UI
        assert_eq!(design_action(Flavor::Ngx, Ui::Server, true, Design::Modern), UseAppUi);
        assert_eq!(design_action(Flavor::Ngx, Ui::Server, false, Design::Modern), Unavailable(NO_APP_UI));
        assert_eq!(design_action(Flavor::Ngx, Ui::Server, true, Design::Classic), ServerHome);
        // bundled interface: always local routes
        assert_eq!(design_action(Flavor::Ngx, Ui::App, true, Design::Classic), Navigate);
        assert_eq!(design_action(Flavor::Paperrust, Ui::App, true, Design::Modern), Navigate);
        assert_eq!(design_action(Flavor::Unknown, Ui::Server, true, Design::Modern), Unavailable(UNKNOWN_FLAVOR));
    }

    #[test]
    fn app_start_follows_design() {
        assert_eq!(app_start_url("http://prapi.localhost/", Design::Modern), "http://prapi.localhost/rs/app/m/");
        assert_eq!(app_start_url("http://prapi.localhost", Design::Classic), "http://prapi.localhost/rs/app/");
        assert_eq!(app_start_url("prapi://localhost/", Design::Modern), "prapi://localhost/rs/app/m/");
        // the target is a page of the gateway (loads as a document without origin)
        let u = tauri::Url::parse(&app_start_url("http://prapi.localhost/", Design::Modern)).unwrap();
        assert!(u.path().starts_with("/rs/app"));
    }

    #[test]
    fn profile_ui_switch_checks() {
        let mut cfg = AppConfig {
            profiles: vec![profile("a1", Flavor::Ngx, Ui::Server), profile("b2", Flavor::Unknown, Ui::Server)],
            active_profile: "a1".into(),
            ..AppConfig::default()
        };
        assert_eq!(set_profile_ui(&mut cfg, "a1", Ui::App, false), Err(NO_APP_UI.into()));
        assert_eq!(cfg.profiles[0].ui, Ui::Server, "unchanged on error");
        set_profile_ui(&mut cfg, "a1", Ui::App, true).unwrap();
        assert_eq!(cfg.profiles[0].ui, Ui::App);
        assert_eq!(set_profile_ui(&mut cfg, "b2", Ui::App, true), Err(UNKNOWN_FLAVOR.into()));
        // back to the server UI is always possible
        set_profile_ui(&mut cfg, "a1", Ui::Server, false).unwrap();
        assert_eq!(cfg.profiles[0].ui, Ui::Server);
        assert!(set_profile_ui(&mut cfg, "zz", Ui::Server, true).is_err());
    }

    #[test]
    fn new_profile_ui() {
        assert_eq!(ui_for_new_profile(Some("app"), Flavor::Ngx, true), Ui::App);
        assert_eq!(ui_for_new_profile(Some("app"), Flavor::Paperrust, true), Ui::App);
        assert_eq!(ui_for_new_profile(Some("app"), Flavor::Ngx, false), Ui::Server, "package without light\\");
        assert_eq!(ui_for_new_profile(Some("app"), Flavor::Unknown, true), Ui::Server);
        assert_eq!(ui_for_new_profile(Some("server"), Flavor::Paperrust, true), Ui::Server);
        assert_eq!(ui_for_new_profile(None, Flavor::Paperrust, true), Ui::Server, "older pages send nothing");
    }

    #[test]
    fn switch_plan_texts_and_targets() {
        let with_profile = AppConfig {
            mode: Mode::Remote,
            active_profile: "a1".into(),
            profiles: vec![profile("a1", Flavor::Ngx, Ui::Server)],
            ..AppConfig::default()
        };
        // remote → local with local data
        let p = plan_switch(&with_profile, Some(Start::Remote), None, Mode::Local, true);
        assert!(!p.unchanged && p.blocked.is_none());
        assert_eq!((p.target, p.next), ("local", "local"));
        assert!(p.confirm.contains("Server-Profile bleiben"), "{}", p.confirm);
        // remote → local without local data: first-start choice
        assert_eq!(plan_switch(&with_profile, Some(Start::Remote), None, Mode::Local, false).next, "local_setup");
        // local → remote with profile: name and "local data stay"
        let mut local = with_profile.clone();
        local.mode = Mode::Local;
        let p = plan_switch(&local, Some(Start::Local), None, Mode::Remote, true);
        assert_eq!(p.next, "remote");
        assert_eq!(p.profile.as_deref(), Some("Büro"));
        assert!(p.confirm.contains("(Büro)") && p.confirm.contains("bleiben unverändert"), "{}", p.confirm);
        // local → remote without profile: address form
        let bare = AppConfig::default();
        let p = plan_switch(&bare, Some(Start::Local), None, Mode::Remote, true);
        assert_eq!(p.next, "connect");
        assert!(p.confirm.contains("Adresse"));
        // nothing to do
        assert!(plan_switch(&with_profile, Some(Start::Remote), None, Mode::Remote, true).unchanged);
        // app.toml says remote but this run is local (env) → not "unchanged"
        assert!(!plan_switch(&with_profile, Some(Start::Local), None, Mode::Remote, true).unchanged);
        // fixed by the environment
        let p = plan_switch(&with_profile, Some(Start::Remote), Some(Mode::Remote), Mode::Local, true);
        assert!(p.blocked.as_deref().is_some_and(|b| b.contains("PAPERRUST_DESKTOP_MODE=remote")));
        assert!(plan_switch(&with_profile, Some(Start::Remote), Some(Mode::Local), Mode::Local, true).blocked.is_none());
    }

    #[test]
    fn switching_keeps_profiles_and_preferences() {
        let mut cfg = AppConfig {
            mode: Mode::Remote,
            active_profile: "a1".into(),
            profiles: vec![profile("a1", Flavor::Paperrust, Ui::App), profile("b2", Flavor::Ngx, Ui::Server)],
            ui: UiPrefs { design: "modern".into() },
            ..AppConfig::default()
        };
        let before = cfg.clone();
        apply_switch(&mut cfg, Mode::Local);
        assert_eq!(cfg.mode, Mode::Local);
        assert_eq!(cfg.profiles, before.profiles);
        assert_eq!(cfg.active_profile, before.active_profile);
        assert_eq!(cfg.ui, before.ui);
        // and back: identical to the start
        apply_switch(&mut cfg, Mode::Remote);
        assert_eq!(cfg, before);
        // survives the file round trip (restart)
        let (back, notes) = AppConfig::parse(&cfg.to_toml().unwrap()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(back, before);
        assert_eq!(crate::mode::decide(Some(&back), None, false, true), Start::Remote);
        let mut local = back.clone();
        apply_switch(&mut local, Mode::Local);
        let (back, _) = AppConfig::parse(&local.to_toml().unwrap()).unwrap();
        assert_eq!(crate::mode::decide(Some(&back), None, false, false), Start::Local);
    }
}
