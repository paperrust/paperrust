//! PaperRust system settings from the UI (docs/SETTINGS.md, WP1–WP3).
//!
//! * [`schema`] – schema registry of all keys (inventory §1, classes A/B/C/D/X).
//! * [`help_en`] – English short descriptions for the registry (API field `help_en`).
//! * [`value`] – validation with the same parsers as `pl_core::config`, env string ↔ JSON.
//! * [`layer`] – layering environment/DB/file/default, origin, snapshot [`Settings`].
//! * [`appconfig`] – ApplicationConfiguration: precedence rule (Python `or`) and shared reading/writing.
//! * [`crypto`] – encryption of the secrets (XChaCha20-Poly1305, HKDF from `SECRET_KEY`).
//! * [`store`] – table `paperrust_setting` + history, atomic changes, NOTIFY, export/import.
//! * [`reload`] – live reload (`ArcSwap`, LISTEN/NOTIFY, 30 s fallback), `restart_pending`.
//! * [`restart`] – restart via exit code 75.
//! * [`logo`] – check/store app logo (v1 `/api/config/` and v2 `/settings/app_logo`).

pub mod appconfig;
pub mod crypto;
pub mod desktop;
pub mod help_en;
pub mod layer;
pub mod logo;
pub mod migration;
pub mod reload;
pub mod restart;
pub mod schema;
pub mod store;
pub mod transfer;
pub mod value;

pub use layer::{Entry, Settings, Source};
pub use reload::SettingsHandle;
pub use schema::{SCHEMA, SettingDef};

/// Migrations of this crate (registration in `pl_cli::migrate::paperrust_migrations`).
pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[crate::store::MIGRATION_0001];
}
