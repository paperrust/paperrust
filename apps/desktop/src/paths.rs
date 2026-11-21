//! Data storage per operating system (docs/DESKTOP.md, "Datenablage").
//!
//! | OS | Default |
//! |---|---|
//! | Windows | `%LOCALAPPDATA%\PaperRust` (not `%APPDATA%` = roaming: DB and documents do not belong in the server-mirrored profile) |
//! | macOS | `~/Library/Application Support/PaperRust` |
//! | Linux/BSD | `$XDG_DATA_HOME/paperrust`, otherwise `~/.local/share/paperrust` |
//!
//! Override for all: `PAPERRUST_DESKTOP_HOME` (e.g. portable installation, tests).

use std::path::PathBuf;

use crate::{DesktopError, Result, io_err};

/// Target platform (for testable path resolution independent of the build target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Unix,
}

impl Os {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Unix
        }
    }
}

pub const HOME_ENV: &str = "PAPERRUST_DESKTOP_HOME";

/// Root directory of the desktop data from environment values (`get`) for `os`.
pub fn resolve_home(os: Os, get: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let non_empty = |k: &str| get(k).filter(|v| !v.trim().is_empty());
    if let Some(h) = non_empty(HOME_ENV) {
        return Ok(PathBuf::from(h));
    }
    let missing =
        |what: &str| DesktopError::Other(format!("Datenverzeichnis nicht bestimmbar: {what} nicht gesetzt (oder {HOME_ENV} setzen)"));
    match os {
        Os::Windows => {
            if let Some(local) = non_empty("LOCALAPPDATA") {
                Ok(PathBuf::from(local).join("PaperRust"))
            } else if let Some(profile) = non_empty("USERPROFILE") {
                Ok(PathBuf::from(profile).join("AppData").join("Local").join("PaperRust"))
            } else {
                Err(missing("LOCALAPPDATA/USERPROFILE"))
            }
        }
        Os::MacOs => non_empty("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support").join("PaperRust"))
            .ok_or_else(|| missing("HOME")),
        Os::Unix => {
            // XDG: only absolute paths count, relative ones are ignored (basedir specification).
            // (`starts_with('/')` instead of `Path::is_absolute`, so the test also holds on Windows.)
            if let Some(x) = non_empty("XDG_DATA_HOME").filter(|x| x.starts_with('/')) {
                Ok(PathBuf::from(x).join("paperrust"))
            } else {
                non_empty("HOME").map(|h| PathBuf::from(h).join(".local").join("share").join("paperrust")).ok_or_else(|| missing("HOME"))
            }
        }
    }
}

/// All directories/files below `home`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub home: PathBuf,
    /// `desktop.toml` (ports, DB password, SECRET_KEY) – readable only by the user.
    pub config_file: PathBuf,
    /// PostgreSQL cluster (`initdb -D`).
    pub pgdata: PathBuf,
    /// `PAPERLESS_DATA_DIR` (index, classifier, …).
    pub data: PathBuf,
    /// `PAPERLESS_MEDIA_ROOT` (originals, archive, thumbnails).
    pub media: PathBuf,
    /// `PAPERLESS_CONSUMPTION_DIR` (inbox folder).
    pub consume: PathBuf,
    pub logs: PathBuf,
    /// Runtime: lock file, `instance.toml`, working directory of the server.
    pub run: PathBuf,
    pub backups: PathBuf,
    /// `PAPERLESS_SCRATCH_DIR` (working copies during ingestion). The upstream default would be
    /// `%TEMP%\paperless` – shared with every other instance on the machine; here a separate
    /// directory on the same drive as `media/` (rename instead of copy).
    pub scratch: PathBuf,
}

impl AppPaths {
    pub fn under(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            config_file: home.join("desktop.toml"),
            pgdata: home.join("pgdata"),
            data: home.join("data"),
            media: home.join("media"),
            consume: home.join("consume"),
            logs: home.join("logs"),
            run: home.join("run"),
            backups: home.join("backups"),
            scratch: home.join("tmp"),
            home,
        }
    }

    pub fn from_env() -> Result<Self> {
        resolve_home(Os::current(), |k| std::env::var(k).ok()).map(Self::under)
    }

    /// Creates all directories except `pgdata` (`initdb` creates that itself, it must be empty).
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            &self.home,
            &self.data,
            &self.media,
            &self.consume,
            &self.logs,
            &self.run,
            &self.backups,
            &self.scratch,
            // like upstream docker `init-folders`
            &self.media.join("documents").join("originals"),
            &self.media.join("documents").join("thumbnails"),
        ] {
            // Unix 0700 (M4); Windows: ACL inherited from the root directory (perms::protect_home).
            crate::perms::create_private_dir(d).map_err(io_err(format!("Verzeichnis {} anlegen", d.display())))?;
        }
        Ok(())
    }

    pub fn lock_file(&self) -> PathBuf {
        self.run.join("instance.lock")
    }

    /// Info for a second instance (URL, PID). Separate from the lock file, because Windows also blocks
    /// reading of a locked file.
    pub fn instance_file(&self) -> PathBuf {
        self.run.join("instance.toml")
    }

    /// Stop request from outside (`paperrust-desktop stop`, installer/updater): create the file, the
    /// running instance shuts down in an orderly way and deletes it.
    pub fn stop_request_file(&self) -> PathBuf {
        self.run.join("stop.request")
    }

    /// Backup request to the running instance (`paperrust-desktop backup` with the app open,
    /// task scheduler): create the file, the instance backs up as via the tray and writes the
    /// result to [`Self::backup_result_file`].
    pub fn backup_request_file(&self) -> PathBuf {
        self.run.join("backup.request")
    }

    /// Result of the most recently requested backup: `ok <folder>` or `error <message>`.
    pub fn backup_result_file(&self) -> PathBuf {
        self.run.join("backup.result")
    }

    /// Full-text index (upstream `INDEX_DIR = DATA_DIR / "index"`). **Not** created by
    /// [`Self::ensure_dirs`]: if it is missing, the supervisor creates it and builds it with
    /// `document_index reindex` (first start, import of a DB without an index).
    pub fn index_dir(&self) -> PathBuf {
        self.data.join("index")
    }

    /// `app.toml` of the Tauri shell: mode (local/remote) and server profiles (docs/REMOTE-APP.md
    /// §2). Separate from `desktop.toml`, which only the local supervisor reads. No secrets.
    pub fn app_config_file(&self) -> PathBuf {
        self.home.join("app.toml")
    }

    /// Root of all server profiles (`profiles/<id>/`).
    pub fn profiles_dir(&self) -> PathBuf {
        self.home.join("profiles")
    }

    /// Folder of a server profile (`webview/` = own WebView data directory, later
    /// `replica.sqlite`, `files/`). `id` comes from `app.toml` and is checked by the caller
    /// (only `[0-9a-z]`), so no path leads out of the configuration.
    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn windows_uses_localappdata() {
        let h =
            resolve_home(Os::Windows, env(&[("LOCALAPPDATA", r"C:\Users\a\AppData\Local"), ("APPDATA", r"C:\Users\a\AppData\Roaming")]))
                .unwrap();
        assert_eq!(h, PathBuf::from(r"C:\Users\a\AppData\Local").join("PaperRust"));
        let h = resolve_home(Os::Windows, env(&[("USERPROFILE", r"C:\Users\a")])).unwrap();
        assert!(h.ends_with(Path::new("AppData").join("Local").join("PaperRust")));
        assert!(resolve_home(Os::Windows, env(&[])).is_err());
    }

    #[test]
    fn macos_and_xdg() {
        let h = resolve_home(Os::MacOs, env(&[("HOME", "/Users/a")])).unwrap();
        assert_eq!(h, PathBuf::from("/Users/a/Library/Application Support/PaperRust"));
        let h = resolve_home(Os::Unix, env(&[("HOME", "/home/a"), ("XDG_DATA_HOME", "/data/x")])).unwrap();
        assert_eq!(h, PathBuf::from("/data/x/paperrust"));
        // relative XDG_DATA_HOME is ignored
        let h = resolve_home(Os::Unix, env(&[("HOME", "/home/a"), ("XDG_DATA_HOME", "rel")])).unwrap();
        assert_eq!(h, PathBuf::from("/home/a/.local/share/paperrust"));
        assert!(resolve_home(Os::Unix, env(&[])).is_err());
    }

    #[test]
    fn override_wins_everywhere() {
        for os in [Os::Windows, Os::MacOs, Os::Unix] {
            let h = resolve_home(os, env(&[(HOME_ENV, "/portable"), ("HOME", "/home/a"), ("LOCALAPPDATA", "C:/x")])).unwrap();
            assert_eq!(h, PathBuf::from("/portable"));
        }
        // empty counts as unset
        let h = resolve_home(Os::Unix, env(&[(HOME_ENV, " "), ("HOME", "/home/a")])).unwrap();
        assert_eq!(h, PathBuf::from("/home/a/.local/share/paperrust"));
    }

    #[test]
    fn layout_under_home() {
        let p = AppPaths::under("/h");
        assert_eq!(p.pgdata, PathBuf::from("/h/pgdata"));
        assert_eq!(p.config_file, PathBuf::from("/h/desktop.toml"));
        assert_ne!(p.lock_file(), p.instance_file());
        assert_eq!(p.app_config_file(), PathBuf::from("/h/app.toml"));
        assert_ne!(p.app_config_file(), p.config_file);
        assert_eq!(p.profile_dir("a1b2c3"), PathBuf::from("/h/profiles/a1b2c3"));
        let dir = tempfile::tempdir().unwrap();
        let p = AppPaths::under(dir.path().join("home"));
        p.ensure_dirs().unwrap();
        assert!(p.media.is_dir() && p.logs.is_dir() && p.run.is_dir());
        assert!(!p.pgdata.exists(), "initdb braucht ein nicht existierendes/leeres Verzeichnis");
    }
}
