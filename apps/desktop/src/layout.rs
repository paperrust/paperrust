//! Installation layout: where the bundled programs live (next to `paperrust-desktop`).
//!
//! ```text
//! <install>/
//!   paperrust-desktop(.exe)     Supervisor (dieses Crate)
//!   paperrust(.exe)             Server/Worker (crates/pl-server)
//!   routes.toml                 all routes = rust (standalone)
//!   static/                     Angular frontend + Django static (PAPERLESS_STATICDIR)
//!   pgsql/bin/                  initdb, pg_ctl, postgres, pg_isready, createdb, pg_dump, pg_restore
//!   pgsql/{lib,share}/          as in the PostgreSQL archive
//!   paperrust-migrate-db(.exe) optional: database migration (crates/pl-dbmigrate), first-start import
//!   tesseract/                  optional: tesseract(.exe) + tessdata/
//!   pdfium/                     optional: pdfium.dll / libpdfium.so / libpdfium.dylib
//!   fonts/LiberationSerif-Regular.ttf  optional: font for text thumbnails
//! ```
//!
//! Every path can be overridden through the environment (development: programs from `target/debug`
//! and a locally unpacked PostgreSQL).

use std::path::{Path, PathBuf};

pub const EXE: &str = std::env::consts::EXE_SUFFIX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallLayout {
    pub root: PathBuf,
    pub server_bin: PathBuf,
    pub pg_bin: PathBuf,
    pub routes_file: Option<PathBuf>,
    pub static_dir: Option<PathBuf>,
    pub tesseract: Option<PathBuf>,
    pub tessdata: Option<PathBuf>,
    pub pdfium: Option<PathBuf>,
    /// Font for text thumbnails (`PAPERLESS_THUMBNAIL_FONT_NAME`); the upstream default
    /// `/usr/share/fonts/liberation/…` does not exist on Windows/macOS.
    pub thumbnail_font: Option<PathBuf>,
    /// `paperrust-migrate-db` (optional): first start "adopt database".
    pub migrate_db_bin: Option<PathBuf>,
}

/// Bundled font for text thumbnails (upstream default, Liberation 2.x, SIL OFL 1.1).
pub const THUMBNAIL_FONT_FILE: &str = "LiberationSerif-Regular.ttf";

pub fn pdfium_file_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

impl InstallLayout {
    /// Layout under `root`; `get` = environment, `exists` = file check (both injectable for tests).
    pub fn resolve(
        root: &Path,
        get: impl Fn(&str) -> Option<String>,
        exists: impl Fn(&Path) -> bool,
    ) -> Self {
        let over = |k: &str| get(k).filter(|v| !v.trim().is_empty()).map(PathBuf::from);
        let opt =
            |k: &str, default: PathBuf| over(k).or_else(|| exists(&default).then_some(default));
        let tesseract_dir = root.join("tesseract");
        Self {
            server_bin: over("PAPERRUST_DESKTOP_SERVER_BIN")
                .unwrap_or_else(|| root.join(format!("paperrust{EXE}"))),
            pg_bin: over("PAPERRUST_DESKTOP_PG_BIN")
                .unwrap_or_else(|| root.join("pgsql").join("bin")),
            routes_file: opt("PAPERRUST_DESKTOP_ROUTES", root.join("routes.toml")),
            static_dir: opt("PAPERRUST_DESKTOP_STATIC", root.join("static")),
            tesseract: opt(
                "PAPERRUST_TESSERACT_PATH",
                tesseract_dir.join(format!("tesseract{EXE}")),
            ),
            tessdata: opt("PAPERRUST_TESSDATA_DIR", tesseract_dir.join("tessdata")),
            pdfium: opt(
                "PAPERRUST_PDFIUM_PATH",
                root.join("pdfium").join(pdfium_file_name()),
            ),
            thumbnail_font: opt(
                "PAPERLESS_THUMBNAIL_FONT_NAME",
                root.join("fonts").join(THUMBNAIL_FONT_FILE),
            ),
            migrate_db_bin: opt(
                "PAPERRUST_DESKTOP_MIGRATE_DB_BIN",
                root.join(format!("paperrust-migrate-db{EXE}")),
            ),
            root: root.to_path_buf(),
        }
    }

    /// Layout next to the running program.
    pub fn from_current_exe() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        let root = exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self::resolve(
            &root,
            |k| std::env::var(k).ok(),
            Path::exists,
        ))
    }

    /// Missing required parts (for an understandable message before startup).
    pub fn missing(&self, exists: impl Fn(&Path) -> bool) -> Vec<String> {
        let mut m = Vec::new();
        if !exists(&self.server_bin) {
            m.push(format!("Server-Programm {}", self.server_bin.display()));
        }
        for tool in crate::pg::REQUIRED_TOOLS {
            let p = self.pg_bin.join(format!("{tool}{EXE}"));
            if !exists(&p) {
                m.push(format!("PostgreSQL-Werkzeug {}", p.display()));
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_optional_parts() {
        let root = Path::new("/opt/paperrust");
        let l = InstallLayout::resolve(root, |_| None, |_| false);
        assert_eq!(l.server_bin, root.join(format!("paperrust{EXE}")));
        assert_eq!(l.pg_bin, root.join("pgsql").join("bin"));
        assert_eq!(l.tesseract, None);
        assert_eq!(l.pdfium, None);
        assert_eq!(l.routes_file, None);
        assert_eq!(l.migrate_db_bin, None);
        let all = InstallLayout::resolve(root, |_| None, |_| true);
        assert_eq!(
            all.pdfium,
            Some(root.join("pdfium").join(pdfium_file_name()))
        );
        assert_eq!(all.tessdata, Some(root.join("tesseract").join("tessdata")));
        assert_eq!(l.thumbnail_font, None);
        assert_eq!(
            all.thumbnail_font,
            Some(root.join("fonts").join("LiberationSerif-Regular.ttf"))
        );
        assert_eq!(all.missing(|_| true), Vec::<String>::new());
        assert_eq!(
            l.missing(|_| false).len(),
            1 + crate::pg::REQUIRED_TOOLS.len()
        );
    }

    #[test]
    fn overrides() {
        let l = InstallLayout::resolve(
            Path::new("/x"),
            |k| match k {
                "PAPERRUST_DESKTOP_PG_BIN" => Some("/usr/lib/postgresql/17/bin".into()),
                "PAPERRUST_TESSERACT_PATH" => Some("/usr/bin/tesseract".into()),
                "PAPERLESS_THUMBNAIL_FONT_NAME" => Some("/f/Serif.ttf".into()),
                "PAPERRUST_DESKTOP_SERVER_BIN" => Some(" ".into()),
                _ => None,
            },
            |_| false,
        );
        assert_eq!(l.pg_bin, PathBuf::from("/usr/lib/postgresql/17/bin"));
        assert_eq!(l.tesseract, Some(PathBuf::from("/usr/bin/tesseract")));
        assert_eq!(l.thumbnail_font, Some(PathBuf::from("/f/Serif.ttf")));
        assert_eq!(
            l.server_bin,
            Path::new("/x").join(format!("paperrust{EXE}"))
        );
    }
}
