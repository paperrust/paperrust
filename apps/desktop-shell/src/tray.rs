//! Tray menu per mode (REMOTE-APP §6). The entries are described as data
//! ([`menu_spec`], testable), [`build_menu`] turns them into the Tauri menu.

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Wry};

pub const BACKUP_LABEL: &str = "Sicherung jetzt";
/// Without a ready update; otherwise "Update auf X installieren …" (Z4).
pub const UPDATE_LABEL: &str = "Nach Updates suchen …";
/// Prefix of the menu IDs for "Profil wechseln ▸ <name>".
pub const PROFILE_PREFIX: &str = "profile:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMode {
    /// Before the choice (first start).
    FirstRun,
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item {
        id: String,
        label: String,
        enabled: bool,
    },
    Separator,
    /// (ID, Name, aktiv)
    Profiles(Vec<(String, String, bool)>),
}

fn item(id: &str, label: &str) -> Entry {
    Entry::Item { id: id.into(), label: label.into(), enabled: true }
}

/// `profiles` = (ID, name), `active` = ID of the active profile.
pub fn menu_spec(mode: TrayMode, profiles: &[(String, String)], active: &str) -> Vec<Entry> {
    match mode {
        TrayMode::Local => vec![
            item("open", "PaperRust öffnen"),
            item(crate::design::MODERN_ID, crate::design::MODERN_LABEL),
            item(crate::design::CLASSIC_ID, crate::design::CLASSIC_LABEL),
            Entry::Item { id: "backup".into(), label: BACKUP_LABEL.into(), enabled: false },
            Entry::Separator,
            item("consume", "Eingangsordner öffnen"),
            item("backups", "Sicherungen öffnen"),
            item("logs", "Logs öffnen"),
            Entry::Separator,
            item("updates", UPDATE_LABEL),
            item("settings", "Einstellungen …"),
            item("quit", "Beenden"),
        ],
        TrayMode::Remote => {
            let mut v = vec![
                item("open", "PaperRust öffnen"),
                item(crate::design::MODERN_ID, crate::design::MODERN_LABEL),
                item(crate::design::CLASSIC_ID, crate::design::CLASSIC_LABEL),
            ];
            if !profiles.is_empty() {
                v.push(Entry::Profiles(
                    profiles.iter().map(|(id, name)| (format!("{PROFILE_PREFIX}{id}"), name.clone(), id == active)).collect(),
                ));
            }
            v.extend([
                item("updates", UPDATE_LABEL),
                item("settings", "Einstellungen …"),
                item("logs", "Logs öffnen"),
                Entry::Separator,
                item("quit", "Beenden"),
            ]);
            v
        }
        TrayMode::FirstRun => {
            vec![item("open", "PaperRust öffnen"), item("logs", "Logs öffnen"), Entry::Separator, item("quit", "Beenden")]
        }
    }
}

/// Built menu with the entries that change later.
pub struct Built {
    pub menu: Menu<Wry>,
    /// "Sicherung jetzt" (mode 1, is (de)activated).
    pub backup: Option<MenuItem<Wry>>,
    /// "Nach Updates suchen …" (text changes when an update is ready).
    pub update: Option<MenuItem<Wry>>,
}

/// Build the menu.
pub fn build_menu(app: &AppHandle, spec: &[Entry]) -> tauri::Result<Built> {
    let mut b = MenuBuilder::new(app);
    let mut backup = None;
    let mut update = None;
    for e in spec {
        b = match e {
            Entry::Item { id, label, enabled } => {
                let it = MenuItemBuilder::with_id(id.as_str(), label.as_str()).enabled(*enabled).build(app)?;
                if id == "backup" {
                    backup = Some(it.clone());
                }
                if id == "updates" {
                    update = Some(it.clone());
                }
                b.item(&it)
            }
            Entry::Separator => b.separator(),
            Entry::Profiles(list) => {
                let mut sub = SubmenuBuilder::new(app, "Profil wechseln");
                for (id, name, active) in list {
                    let c = CheckMenuItemBuilder::with_id(id.as_str(), name.as_str()).checked(*active).build(app)?;
                    sub = sub.item(&c);
                }
                b.item(&sub.build()?)
            }
        };
    }
    Ok(Built { menu: b.build()?, backup, update })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(spec: &[Entry]) -> Vec<String> {
        spec.iter()
            .map(|e| match e {
                Entry::Item { id, .. } => id.clone(),
                Entry::Separator => "-".into(),
                Entry::Profiles(p) => format!("profiles[{}]", p.len()),
            })
            .collect()
    }

    #[test]
    fn local_menu_keeps_mode1_entries() {
        let s = menu_spec(TrayMode::Local, &[], "");
        assert_eq!(
            ids(&s),
            ["open", "design-modern", "design-classic", "backup", "-", "consume", "backups", "logs", "-", "updates", "settings", "quit"]
        );
        assert!(matches!(&s[3], Entry::Item { enabled: false, .. }), "Sicherung erst nach dem Start");
    }

    #[test]
    fn remote_menu_without_local_functions() {
        let profiles = vec![("a1".to_string(), "Büro".to_string()), ("b2".to_string(), "NAS".to_string())];
        let s = menu_spec(TrayMode::Remote, &profiles, "b2");
        assert_eq!(ids(&s), ["open", "design-modern", "design-classic", "profiles[2]", "updates", "settings", "logs", "-", "quit"]);
        let Entry::Profiles(p) = &s[3] else { unreachable!() };
        assert_eq!(p[0], ("profile:a1".to_string(), "Büro".to_string(), false));
        assert!(p[1].2, "aktives Profil angehakt");
        for forbidden in ["backup", "consume", "backups"] {
            assert!(!ids(&s).contains(&forbidden.to_string()));
        }
        // without a profile no submenu
        assert_eq!(
            ids(&menu_spec(TrayMode::Remote, &[], "")),
            ["open", "design-modern", "design-classic", "updates", "settings", "logs", "-", "quit"]
        );
        assert_eq!(ids(&menu_spec(TrayMode::FirstRun, &[], "")), ["open", "logs", "-", "quit"]);
    }
}
