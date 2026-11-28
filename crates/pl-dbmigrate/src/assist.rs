//! Helpers for the switchover wizard: free space, folder size, backup with `pg_dump`,
//! copying with progress.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Total size of all files under `p` (symlinks not followed). File → its size.
pub fn dir_size(p: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return (0, 0) };
    if meta.is_file() {
        return (meta.len(), 1);
    }
    let (mut bytes, mut files) = (0, 0);
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => {
                    bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                    files += 1;
                }
                _ => {}
            }
        }
    }
    (bytes, files)
}

/// Free space (bytes) on the drive of `p`; `p` need not exist yet (the nearest
/// existing parent folder counts).
pub fn free_space(p: &Path) -> Option<u64> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if c.exists() {
            return free_space_at(c);
        }
        cur = c.parent();
    }
    free_space_at(Path::new("."))
}

#[cfg(unix)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: A zeroed struct is a valid output parameter for statvfs.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid, NUL-terminated path; s is a valid pointer.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    #[allow(clippy::unnecessary_cast)]
    (rc == 0).then(|| s.f_frsize as u64 * s.f_bavail as u64)
}

#[cfg(windows)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: NUL-terminated string and valid output pointers.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some(avail)
}

#[cfg(not(any(unix, windows)))]
fn free_space_at(_: &Path) -> Option<u64> {
    None
}

pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

/// Absolute, normalized form of `p` that also works for paths that do not exist yet: the
/// nearest existing ancestor is canonicalized, the rest is appended with `.`/`..` resolved.
fn resolve_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // lexical normalization first (`.`/`..`), then canonicalize the nearest existing ancestor
    let mut norm = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    let mut base = norm.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(mut c) = std::fs::canonicalize(base) {
            for name in rest.iter().rev() {
                c.push(name);
            }
            return c;
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                base = parent;
            }
            _ => return norm,
        }
    }
}

/// `to` is `from` itself or lies below it (review finding 3: the copy would descend into its own
/// target). Compared after resolving symlinks and `..`; `to` may not exist yet.
pub fn target_inside_source(from: &Path, to: &Path) -> bool {
    resolve_path(to).starts_with(resolve_path(from))
}

/// Result of [`copy_tree_progress`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// newly copied
    pub copied: u64,
    /// already present with the same size, left untouched
    pub kept: u64,
    /// present with a different size (remainder of an interrupted copy), replaced
    pub replaced: u64,
}

/// Copy one file via a temporary file in the target folder and rename it afterwards, so an
/// interruption never leaves a partial file under the final name (review finding 4).
fn copy_file_atomic(src: &Path, dst: &Path) -> std::io::Result<()> {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{name}.paperrust-part"));
    if let Err(e) = std::fs::copy(src, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // flush to disk before the rename makes the file visible under its final name
    if let Err(e) = std::fs::OpenOptions::new().write(true).open(&tmp).and_then(|f| f.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, dst).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Copy a folder recursively with progress display (every 2 s on stderr), symlinks are
/// skipped. An existing target file with the same size is kept (no overwriting); one with a
/// different size is the remainder of an interrupted run and gets replaced. Every file is
/// written to a temporary name first and renamed afterwards. A target inside the source is
/// refused before anything is created.
pub fn copy_tree_progress(from: &Path, to: &Path, total_files: u64, quiet: bool) -> std::io::Result<CopyStats> {
    struct St {
        stats: CopyStats,
        total: u64,
        last: Instant,
        quiet: bool,
    }
    fn walk(from: &Path, to: &Path, st: &mut St) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let ty = e.file_type()?;
            let dst = to.join(e.file_name());
            if ty.is_dir() {
                walk(&e.path(), &dst, st)?;
            } else if ty.is_file() {
                match std::fs::symlink_metadata(&dst) {
                    Ok(m) if m.is_file() && m.len() == e.metadata()?.len() => st.stats.kept += 1,
                    Ok(m) if m.is_file() => {
                        copy_file_atomic(&e.path(), &dst)?;
                        st.stats.replaced += 1;
                    }
                    Ok(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!("{} ist im Ziel keine Datei", dst.display()),
                        ));
                    }
                    Err(_) => {
                        copy_file_atomic(&e.path(), &dst)?;
                        st.stats.copied += 1;
                    }
                }
                if !st.quiet && st.last.elapsed() >= Duration::from_secs(2) {
                    st.last = Instant::now();
                    let done = st.stats.copied + st.stats.kept + st.stats.replaced;
                    let pct = if st.total > 0 { (done as f64 / st.total as f64 * 100.0).min(100.0) } else { 0.0 };
                    eprintln!("[....] Dateien kopieren: {done} von {} ({pct:.0} %)", st.total);
                }
            }
        }
        Ok(())
    }
    if target_inside_source(from, to) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Ziel-Medienordner {} liegt im Quell-Medienordner {} – bitte einen Ordner außerhalb wählen",
                to.display(),
                from.display()
            ),
        ));
    }
    let mut st = St { stats: CopyStats::default(), total: total_files, last: Instant::now(), quiet };
    walk(from, to, &mut st)?;
    Ok(st.stats)
}

/// Find `pg_dump`: `PAPERRUST_PG_BIN`, `PAPERRUST_DESKTOP_PG_BIN`, next to this program,
/// `pgsql/bin` next to it (desktop package), otherwise PATH.
pub fn find_pg_dump(get: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let name = format!("pg_dump{}", std::env::consts::EXE_SUFFIX);
    let mut dirs: Vec<PathBuf> = Vec::new();
    for k in ["PAPERRUST_PG_BIN", "PAPERRUST_DESKTOP_PG_BIN"] {
        if let Some(d) = get(k).filter(|s| !s.is_empty()) {
            dirs.push(PathBuf::from(d));
        }
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("pgsql").join("bin"));
        dirs.push(exe_dir);
    }
    if let Some(path) = get("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

/// Backup of the database `url` with `pg_dump -Fc` to `<dir>/paperless-vor-umstieg-<time>.pgdump`.
/// The password goes via `PGPASSWORD`, not via the command line.
pub async fn pg_dump_backup(pg_dump: &Path, url: &str, dir: &Path) -> Result<PathBuf> {
    let mut u = url::Url::parse(url).context("Datenbank-URL ungültig")?;
    let pass = u.password().map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned());
    let _ = u.set_password(None);
    std::fs::create_dir_all(dir).with_context(|| format!("Ordner {} anlegen", dir.display()))?;
    let file = dir.join(format!("paperless-vor-umstieg-{}.pgdump", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let mut c = tokio::process::Command::new(pg_dump);
    c.arg("-Fc").arg("--no-password").arg("-f").arg(&file).arg("-d").arg(u.as_str());
    if let Some(p) = pass {
        c.env("PGPASSWORD", p);
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    let out = c.output().await.with_context(|| format!("{} lässt sich nicht starten", pg_dump.display()))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&file);
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!("pg_dump fehlgeschlagen: {}", msg.lines().next().unwrap_or("").trim());
    }
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        bail!("pg_dump hat eine leere Datei geschrieben");
    }
    Ok(file)
}

/// Backup without `pg_dump`: complete copy of the database on the same server
/// (`CREATE DATABASE <name>_vor_umstieg_<time> TEMPLATE <name>`). Needs the CREATEDB privilege (in
/// Docker the database user is usually a superuser) and no other open connections to the
/// database (paperless-ngx stopped). Returns the name of the copy.
pub async fn template_copy(url: &str) -> Result<String> {
    use sqlx::Connection;
    let u = url::Url::parse(url).context("Datenbank-URL ungültig")?;
    let db = percent_encoding::percent_decode_str(u.path().trim_start_matches('/')).decode_utf8_lossy().into_owned();
    if db.is_empty() {
        bail!("Datenbankname fehlt in der URL");
    }
    let copy = format!("{}_vor_umstieg_{}", db.chars().take(30).collect::<String>(), chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let mut admin = u.clone();
    admin.set_path("/postgres");
    let mut c = match crate::migrate::connect_target(admin.as_str()).await {
        Ok(c) => c,
        Err(_) => {
            admin.set_path("/template1");
            crate::migrate::connect_target(admin.as_str()).await.context("Verbindung zur Verwaltungsdatenbank (postgres/template1)")?
        }
    };
    let others: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()")
        .bind(&db)
        .fetch_one(&mut c)
        .await
        .unwrap_or(0);
    if others > 0 {
        let _ = c.close().await;
        bail!("es sind noch {others} andere Verbindungen zur Datenbank offen – bitte paperless-ngx ganz anhalten");
    }
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let r = sqlx::raw_sql(&format!("CREATE DATABASE {} TEMPLATE {}", q(&copy), q(&db))).execute(&mut c).await;
    let _ = c.close().await;
    r.context("Kopie der Datenbank auf dem Server (CREATE DATABASE … TEMPLATE) nicht möglich")?;
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_space_copy() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("x.pdf"), b"12345").unwrap();
        std::fs::write(src.join("sub").join("y.pdf"), b"123").unwrap();
        assert_eq!(dir_size(&src), (8, 2));
        assert!(free_space(&d.path().join("noch/nicht/da")).unwrap_or(1) > 0);
        let dst = d.path().join("b");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("x.pdf"), b"alt").unwrap();
        // same size as the source file: kept, never overwritten
        std::fs::write(dst.join("x.pdf"), b"alt45").unwrap();
        assert_eq!(copy_tree_progress(&src, &dst, 2, true).unwrap(), CopyStats { copied: 1, kept: 1, replaced: 0 });
        assert_eq!(std::fs::read(dst.join("x.pdf")).unwrap(), b"alt45", "nie überschreiben");
        assert_eq!(human(2_500_000_000), "2.5 GB");
    }

    /// Review finding 3: a target folder below the source must be refused, not copied into
    /// itself.
    #[test]
    fn copy_refuses_target_inside_source() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("media");
        std::fs::create_dir_all(src.join("documents")).unwrap();
        std::fs::write(src.join("documents").join("x.pdf"), b"12345").unwrap();
        let dst = src.join("paperrust");
        assert!(copy_tree_progress(&src, &dst, 1, true).is_err());
        assert!(!dst.join("paperrust").exists(), "copied into itself");
        // also via a not yet existing deeper path and with `..` in it
        let dst2 = src.join("a").join("..").join("b").join("c");
        assert!(copy_tree_progress(&src, &dst2, 1, true).is_err());
        assert!(!src.join("b").exists());
        assert!(target_inside_source(&src, &src));
        assert!(!target_inside_source(&src, &d.path().join("media2")), "only a name prefix, not inside");
        assert!(!target_inside_source(&src.join("documents"), &src), "source inside target is allowed");
    }

    /// Review finding 4: a partial file from an interrupted copy is repaired on the next run.
    #[test]
    fn copy_repairs_partial_file_on_retry() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub").join("y.pdf"), b"0123456789").unwrap();
        let dst = d.path().join("b");
        std::fs::create_dir_all(dst.join("sub")).unwrap();
        // what an interrupted run left behind: the first bytes only
        std::fs::write(dst.join("sub").join("y.pdf"), b"0123").unwrap();
        assert_eq!(copy_tree_progress(&src, &dst, 1, true).unwrap(), CopyStats { copied: 0, kept: 0, replaced: 1 });
        assert_eq!(std::fs::read(dst.join("sub").join("y.pdf")).unwrap(), b"0123456789");
        // second retry: now complete, kept
        assert_eq!(copy_tree_progress(&src, &dst, 1, true).unwrap(), CopyStats { copied: 0, kept: 1, replaced: 0 });
        let names: Vec<_> = std::fs::read_dir(dst.join("sub")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("y.pdf")], "no temp files left");
    }

    #[test]
    fn pg_dump_lookup_env() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join(format!("pg_dump{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, b"").unwrap();
        let dir = d.path().display().to_string();
        let found = find_pg_dump(&|k| (k == "PAPERRUST_PG_BIN").then(|| dir.clone()));
        assert_eq!(found.as_deref(), Some(exe.as_path()));
    }
}
