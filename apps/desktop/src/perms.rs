//! Permissions of the desktop data (security review M4).
//!
//! The data directory contains all originals, the database, `desktop.toml` (DB password,
//! `SECRET_KEY`) and logs. On multi-user machines only the owning user may read it:
//!
//! - **Unix:** all directories `0700`, private files `0600` (independent of the umask).
//! - **Windows:** once, a protected ACL on the root directory – inheritance from the
//!   parent folder is cut, the only entry is "current user: full access", inherited by all
//!   subfolders/files (`icacls /inheritance:r /grant:r *<SID>:(OI)(CI)F`). Files
//!   below it (also `desktop.toml`, `run/initdb.pw`, logs, `pgdata`) inherit that. PostgreSQL runs with
//!   a restricted token of the **same** user (pg_ctl) and thereby keeps access
//!   (smoke test D1). Administrators can take ownership, as anywhere on Windows.
//! - **Location:** if `PAPERRUST_DESKTOP_HOME` lies outside the user's own profile (`%LOCALAPPDATA%`,
//!   `%USERPROFILE%` or `$HOME`), a warning is issued – shared permissions are more likely there.

use std::path::{Path, PathBuf};

use crate::paths::Os;

/// Creates `dir` (recursively); on Unix with `0700` – even if it already exists.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Warning if `home` lies outside the user's own profile (`None` = fine).
pub fn location_warning(os: Os, home: &Path, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let roots: Vec<PathBuf> = match os {
        Os::Windows => {
            ["LOCALAPPDATA", "USERPROFILE"].iter().filter_map(|k| get(k)).filter(|v| !v.trim().is_empty()).map(PathBuf::from).collect()
        }
        Os::MacOs | Os::Unix => get("HOME").filter(|v| !v.trim().is_empty()).map(PathBuf::from).into_iter().collect(),
    };
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if os == Os::Windows { s.to_lowercase() } else { s }
    };
    let h = norm(home);
    let inside = roots.iter().any(|r| {
        let r = norm(r);
        !r.is_empty() && (h == r || h.starts_with(&format!("{r}/")))
    });
    (!inside).then(|| {
        format!(
            "Datenverzeichnis {} liegt außerhalb des eigenen Benutzerprofils – andere Konten auf diesem Rechner könnten Dokumente, Datenbank und Schlüssel lesen. Empfohlen: Standardort (ohne PAPERRUST_DESKTOP_HOME).",
            home.display()
        )
    })
}

/// SID of the logged-in user from `whoami /user /fo csv /nh` (`"machine\name","S-1-5-21-…"`).
pub fn parse_whoami_sid(out: &str) -> Option<String> {
    out.lines()
        .flat_map(|l| l.split(','))
        .map(|f| f.trim().trim_matches('"'))
        .find(|f| f.starts_with("S-1-") && f.len() > 4 && f.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'S'))
        .map(str::to_string)
}

/// `icacls` arguments for "current user only, inherited, inheritance from above cut".
pub fn icacls_args(dir: &Path, sid: &str) -> Vec<std::ffi::OsString> {
    vec![dir.as_os_str().to_owned(), "/inheritance:r".into(), "/grant:r".into(), format!("*{sid}:(OI)(CI)F").into(), "/Q".into()]
}

#[cfg(windows)]
fn hidden(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Make the root directory accessible only to the current user (Windows: ACL, Unix: 0700).
pub fn protect_home(home: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::io::Error;
        use std::process::{Command, Stdio};
        let mut who = Command::new("whoami");
        who.args(["/user", "/fo", "csv", "/nh"]).stdin(Stdio::null()).stderr(Stdio::null());
        hidden(&mut who);
        let out = who.output()?;
        let sid = parse_whoami_sid(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| Error::other("SID des Benutzers nicht lesbar"))?;
        let mut ic = Command::new("icacls");
        ic.args(icacls_args(home, &sid)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        hidden(&mut ic);
        let out = ic.output()?;
        if !out.status.success() {
            return Err(Error::other(format!("icacls ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim())));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        create_private_dir(home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn warns_outside_profile() {
        let w = env(&[("LOCALAPPDATA", r"C:\Users\a\AppData\Local"), ("USERPROFILE", r"C:\Users\a")]);
        assert_eq!(location_warning(Os::Windows, Path::new(r"C:\Users\a\AppData\Local\PaperRust"), &w), None);
        assert_eq!(location_warning(Os::Windows, Path::new(r"c:\users\A\Dokumente\pr"), &w), None);
        assert!(location_warning(Os::Windows, Path::new(r"D:\PaperRust"), &w).is_some());
        assert!(location_warning(Os::Windows, Path::new(r"C:\Users\ab\x"), &w).is_some());
        assert!(location_warning(Os::Windows, Path::new(r"C:\Users\Public\PaperRust"), &w).is_some());
        let u = env(&[("HOME", "/home/a")]);
        assert_eq!(location_warning(Os::Unix, Path::new("/home/a/.local/share/paperrust"), &u), None);
        assert!(location_warning(Os::Unix, Path::new("/srv/paperrust"), &u).is_some());
        assert!(location_warning(Os::MacOs, Path::new("/tmp/x"), env(&[])).is_some());
    }

    #[test]
    fn whoami_and_icacls() {
        assert_eq!(parse_whoami_sid("\"pc\\jane\",\"S-1-5-21-1-2-3-1001\"\r\n").as_deref(), Some("S-1-5-21-1-2-3-1001"));
        assert_eq!(parse_whoami_sid("Zugriff verweigert"), None);
        let a: Vec<String> = icacls_args(Path::new(r"C:\d"), "S-1-5-21-9").iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(a[1..], ["/inheritance:r", "/grant:r", "*S-1-5-21-9:(OI)(CI)F", "/Q"]);
    }

    /// Real permissions: Unix 0700/0600; Windows exactly one (inheritable) entry for the user.
    #[test]
    fn protect_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        create_private_dir(&home.join("media")).unwrap();
        protect_home(&home).unwrap();
        crate::config::write_private(&home.join("secret.toml"), b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home), 0o700);
            assert_eq!(mode(&home.join("media")), 0o700);
            assert_eq!(mode(&home.join("secret.toml")), 0o600);
        }
        #[cfg(windows)]
        {
            for p in [home.clone(), home.join("media"), home.join("secret.toml")] {
                let out = std::process::Command::new("icacls").arg(&p).output().unwrap();
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                // Output: "<path> <ACE>\n<indentation><ACE>…\n\n<status line>" – ACE lines contain ":(".
                let aces: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
                assert_eq!(aces.len(), 1, "{p:?}: {text}");
                assert!(aces[0].contains("(F)"), "{text}");
            }
        }
    }
}
