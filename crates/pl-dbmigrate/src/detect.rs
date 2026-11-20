//! Detect the source of a switchover: export (folder/ZIP), data folder with SQLite (+ media folder)
//! or database URL. File system/URL checks only, no connection.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Folder with manifest.json
    ExportDir,
    /// ZIP file (`document_exporter --zip`)
    ExportZip,
    /// Data folder/SQLite file (`data/db.sqlite3`), media folder next to it
    SqliteFolder,
    /// sqlite://, postgres://, mysql://, mariadb://
    DatabaseUrl,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::ExportDir => "export-dir",
            Kind::ExportZip => "export-zip",
            Kind::SqliteFolder => "sqlite-folder",
            Kind::DatabaseUrl => "database-url",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Kind::ExportDir => "paperless-ngx-Export (Ordner)",
            Kind::ExportZip => "paperless-ngx-Export (ZIP)",
            Kind::SqliteFolder => "paperless-ngx-Datenordner mit SQLite",
            Kind::DatabaseUrl => "paperless-ngx-Datenbank (URL)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub kind: Kind,
    /// Export: path; otherwise database URL (with password – never output unfiltered)
    pub export: Option<PathBuf>,
    pub url: Option<String>,
    /// Media folder of the source (`MEDIA_ROOT`, contains `documents/`)
    pub media: Option<PathBuf>,
    pub notes: Vec<String>,
}

impl Detection {
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "label": self.kind.label(),
            "export": self.export.as_ref().map(|p| p.display().to_string()),
            "url": self.url.as_deref().map(crate::source::redact),
            "media": self.media.as_ref().map(|p| p.display().to_string()),
            "notes": self.notes,
        })
    }
}

/// Media folder of paperless-ngx: contains `documents/` (originals, archive, thumbnails).
pub fn is_media_dir(p: &Path) -> bool {
    p.join("documents").is_dir()
}

fn is_sqlite_file(p: &Path) -> bool {
    let mut b = [0u8; 16];
    use std::io::Read;
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
        && &b == b"SQLite format 3\0"
}

pub fn sqlite_url(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("sqlite://{s}")
    } else {
        format!("sqlite:///{s}")
    }
}

/// Candidates for the media folder next to a data folder (standard layout of paperless-ngx:
/// `<root>/data`, `<root>/media`; Docker volumes `data`/`media`).
fn guess_media(data_dir: &Path) -> Option<PathBuf> {
    let mut c = vec![data_dir.join("media")];
    if let Some(parent) = data_dir.parent() {
        c.push(parent.join("media"));
    }
    c.into_iter().find(|p| is_media_dir(p))
}

/// Detects the source. `media` = media folder given by the user (takes precedence).
pub fn detect(input: &str, media: Option<&Path>) -> Result<Detection> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Keine Quelle angegeben (Export-Ordner/ZIP, Datenordner oder Datenbank-URL)");
    }
    let mut notes = vec![];
    if let Some(m) = media
        && !is_media_dir(m)
    {
        bail!(
            "{} ist kein paperless-ngx-Medienordner (Unterordner documents/ fehlt)",
            m.display()
        );
    }
    let lower = input.to_ascii_lowercase();
    if [
        "sqlite:",
        "postgres://",
        "postgresql://",
        "mysql://",
        "mariadb://",
    ]
    .iter()
    .any(|s| lower.starts_with(s))
    {
        let mut media = media.map(Path::to_path_buf);
        if let Some(p) = crate::source::sqlite_path(input) {
            let p = PathBuf::from(p);
            if !p.is_file() {
                bail!("SQLite-Datei {} nicht gefunden", p.display());
            }
            if media.is_none() {
                media = p.parent().and_then(guess_media);
            }
        }
        if media.is_none() {
            notes.push(
                "Kein Medienordner angegeben – Dateien werden nicht kopiert und nur im Ziel geprüft (--media <MEDIA_ROOT>)".into(),
            );
        }
        return Ok(Detection {
            kind: Kind::DatabaseUrl,
            export: None,
            url: Some(input.to_string()),
            media,
            notes,
        });
    }
    if lower.contains("://") {
        bail!(
            "Unbekanntes URL-Schema: erwartet sqlite:///…, postgres://…, mysql://… oder mariadb://…"
        );
    }
    let p = PathBuf::from(input);
    if p.is_file() {
        if crate::zipread::looks_like_zip(&p) {
            return Ok(Detection {
                kind: Kind::ExportZip,
                export: Some(p),
                url: None,
                media: None,
                notes,
            });
        }
        if is_sqlite_file(&p) {
            let media = media
                .map(Path::to_path_buf)
                .or_else(|| p.parent().and_then(guess_media));
            return Ok(Detection {
                kind: Kind::SqliteFolder,
                url: Some(sqlite_url(&p)),
                export: None,
                media,
                notes,
            });
        }
        bail!(
            "{} ist weder ein ZIP-Export noch eine SQLite-Datenbank",
            p.display()
        );
    }
    if p.is_dir() {
        if p.join("manifest.json").is_file() {
            return Ok(Detection {
                kind: Kind::ExportDir,
                export: Some(p),
                url: None,
                media: None,
                notes,
            });
        }
        for (db, data) in [
            (p.join("db.sqlite3"), p.clone()),
            (p.join("data").join("db.sqlite3"), p.join("data")),
        ] {
            if db.is_file() {
                let media = media.map(Path::to_path_buf).or_else(|| guess_media(&data));
                if media.is_none() {
                    notes.push(format!(
                        "Neben {} wurde kein Medienordner gefunden – bitte mit --media angeben",
                        data.display()
                    ));
                }
                return Ok(Detection {
                    kind: Kind::SqliteFolder,
                    url: Some(sqlite_url(&db)),
                    export: None,
                    media,
                    notes,
                });
            }
        }
        if is_media_dir(&p) {
            bail!(
                "{} ist ein Medienordner. Bitte den Datenordner (mit db.sqlite3) bzw. die Datenbank-URL als Quelle \
                 und diesen Ordner mit --media angeben",
                p.display()
            );
        }
        bail!(
            "{} enthält weder manifest.json (Export) noch db.sqlite3 bzw. data/db.sqlite3 (paperless-ngx mit SQLite)",
            p.display()
        );
    }
    bail!("{input} nicht gefunden")
}

/// Target URL from the paperless environment (`PAPERLESS_DBHOST` …, Docker compose), if set.
pub fn target_from_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let v = |n: &str| {
        get(&format!("PAPERRUST_{n}"))
            .or_else(|| get(&format!("PAPERLESS_{n}")))
            .filter(|s| !s.is_empty())
    };
    let host = v("DBHOST")?;
    if let Some(e) = v("DBENGINE")
        && !e.eq_ignore_ascii_case("postgresql")
        && !e.eq_ignore_ascii_case("postgres")
    {
        return None;
    }
    let enc = |s: String| percent_encode(&s);
    let port = v("DBPORT").unwrap_or_else(|| "5432".into());
    let name = v("DBNAME").unwrap_or_else(|| "paperless".into());
    let user = enc(v("DBUSER").unwrap_or_else(|| "paperless".into()));
    let pass = enc(v("DBPASS").unwrap_or_else(|| "paperless".into()));
    let mut u = format!("postgres://{user}:{pass}@{host}:{port}/{name}");
    if let Some(m) = v("DBSSLMODE") {
        u.push_str(&format!("?sslmode={m}"));
    }
    Some(u)
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Same database? (host case-insensitive, default port, database name) – then there is nothing to
/// copy (Docker: just swap the image).
pub fn same_database(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> Option<(String, u16, String)> {
        let u = url::Url::parse(s).ok()?;
        if !matches!(u.scheme(), "postgres" | "postgresql") {
            return None;
        }
        let host = u.host_str()?.to_ascii_lowercase();
        let host = if host == "localhost" || host == "[::1]" || host == "::1" {
            "127.0.0.1".to_string()
        } else {
            host
        };
        Some((
            host,
            u.port().unwrap_or(5432),
            u.path().trim_start_matches('/').to_string(),
        ))
    };
    matches!((norm(a), norm(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(root: &Path) -> PathBuf {
        let m = root.join("media");
        std::fs::create_dir_all(m.join("documents/originals")).unwrap();
        m
    }

    #[test]
    fn detects_export_dir_zip_and_sqlite_layouts() {
        let d = tempfile::tempdir().unwrap();
        let exp = d.path().join("export");
        std::fs::create_dir_all(&exp).unwrap();
        std::fs::write(exp.join("manifest.json"), "[]").unwrap();
        assert_eq!(
            detect(exp.to_str().unwrap(), None).unwrap().kind,
            Kind::ExportDir
        );

        let zip = d.path().join("e.zip");
        std::fs::write(
            &zip,
            crate::zipread::testzip::build(&[("manifest.json", b"[]", false)]),
        )
        .unwrap();
        assert_eq!(
            detect(zip.to_str().unwrap(), None).unwrap().kind,
            Kind::ExportZip
        );

        // <root>/data/db.sqlite3 + <root>/media
        let root = d.path().join("paperless");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/db.sqlite3"), b"SQLite format 3\0rest").unwrap();
        let m = media(&root);
        for input in [
            root.clone(),
            root.join("data"),
            root.join("data/db.sqlite3"),
        ] {
            let det = detect(input.to_str().unwrap(), None).unwrap();
            assert_eq!(det.kind, Kind::SqliteFolder, "{input:?}");
            assert_eq!(det.media.as_deref(), Some(m.as_path()));
            let url = det.url.unwrap();
            assert!(
                url.starts_with("sqlite://") && url.ends_with("/data/db.sqlite3"),
                "{url}"
            );
            assert_eq!(
                crate::source::sqlite_path(&url).map(PathBuf::from).unwrap(),
                std::path::absolute(root.join("data/db.sqlite3")).unwrap()
            );
        }
        // Media folder given directly → hint
        assert!(
            detect(m.to_str().unwrap(), None)
                .unwrap_err()
                .to_string()
                .contains("Medienordner")
        );
        // wrong media folder
        assert!(detect(exp.to_str().unwrap(), Some(&exp)).is_err());
        // any file
        let txt = d.path().join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(detect(txt.to_str().unwrap(), None).is_err());
        assert!(detect(d.path().join("fehlt").to_str().unwrap(), None).is_err());
        assert!(
            detect(d.path().to_str().unwrap(), None).is_err(),
            "leerer Ordner"
        );
    }

    #[test]
    fn detects_urls() {
        let det = detect("postgres://u:p@db:5432/paperless", None).unwrap();
        assert_eq!(det.kind, Kind::DatabaseUrl);
        assert!(det.media.is_none() && !det.notes.is_empty());
        assert_eq!(det.to_json()["url"], "postgres://u:***@db:5432/paperless");
        assert_eq!(
            detect("MariaDB://u@h/p", None).unwrap().kind,
            Kind::DatabaseUrl
        );
        assert!(detect("sqlite:///C:/gibt/es/nicht.sqlite3", None).is_err());
        assert!(detect("http://x", None).is_err());
        assert!(detect("  ", None).is_err());
    }

    #[test]
    fn target_env_and_same_db() {
        let env = |k: &str| match k {
            "PAPERLESS_DBHOST" => Some("db".to_string()),
            "PAPERLESS_DBPASS" => Some("p@ss/w".to_string()),
            _ => None,
        };
        assert_eq!(
            target_from_env(env).unwrap(),
            "postgres://paperless:p%40ss%2Fw@db:5432/paperless"
        );
        assert_eq!(target_from_env(|_| None), None);
        let sqlite = |k: &str| match k {
            "PAPERLESS_DBHOST" => Some("db".to_string()),
            "PAPERLESS_DBENGINE" => Some("sqlite".to_string()),
            _ => None,
        };
        assert_eq!(target_from_env(sqlite), None);
        assert!(same_database(
            "postgres://a:b@LOCALHOST/paperless",
            "postgresql://c@127.0.0.1:5432/paperless"
        ));
        assert!(!same_database("postgres://a@h/x", "postgres://a@h/y"));
        assert!(!same_database("sqlite:///x", "sqlite:///x"));
    }
}
