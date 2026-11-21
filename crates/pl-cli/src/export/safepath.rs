//! Path containment for export/import (PaperRust stricter than upstream, review security-wave3
//! H2/M4/M5/C-N3/C-N4).
//!
//! upstream checks nothing here (`document_importer.py:557-674` `self.source / doc_file`,
//! `models.py:465-483` `Document.source_path` without containment); only `ShareLinkBundle.absolute_file_path`
//! (`models.py:1225-1237`) checks `relative_to(bundle_dir)` after `resolve()`. Valid exports
//! (relative paths with `/`) behave unchanged; see docs/contract/pl-cli.md
//! "Known differences".

use std::path::{Component, Path, PathBuf};

use crate::{CliError, CliResult};

/// Stored/exported relative path: not empty, no NUL, not absolute, no
/// drive/prefix, no `..` component. `\` counts as a separator on **all** platforms: upstream
/// on Windows stores `filename` as `str(Path)` with `\` (e.g. `ACME Corp\2019\x.pdf`), such
/// exports stay valid; `..\x` or `\x` are rejected on Linux too, though.
pub fn is_safe_relative(value: &str) -> bool {
    if value.is_empty() || value.contains('\0') || value.starts_with(['/', '\\']) {
        return false;
    }
    // "C:x" is a drive-relative path on Windows – reject everywhere so that exports stay portable
    if has_drive(value) {
        return false;
    }
    if value.split(['/', '\\']).any(|part| part == "..") {
        return false;
    }
    let p = Path::new(value);
    if p.is_absolute() || p.has_root() {
        return false;
    }
    !p.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
}

fn has_drive(part: &str) -> bool {
    let b = part.as_bytes();
    b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
}

/// Target `root / rel` only after checking: `rel` safely relative and (after resolving symlinks like
/// `Path.resolve()`) inside `root` (`validate_path_in_root`).
pub fn target_in(root: &Path, rel: &str, what: &str) -> CliResult<PathBuf> {
    if !is_safe_relative(rel) {
        return Err(CliError::Command(format!(
            "Refusing to import {what} with unsafe path \"{rel}\" (must be relative, without \"..\" or a drive)"
        )));
    }
    let target = root.join(rel);
    pl_templating::validate_path_in_root(&target, root).map_err(|e| CliError::Command(format!("Refusing to import {what}: {e}")))?;
    Ok(target)
}

/// Source file `dir / rel` from the import directory (or unpacked ZIP): safely relative, no
/// symlink file, canonically inside `dir`. `Ok(None)` = file missing (the caller reports the
/// upstream text); a path that was only checked lexically is never returned
/// (security review final K-N3: previously `Ok(p)` for a missing file).
pub fn source_in(dir: &Path, rel: &str) -> CliResult<Option<PathBuf>> {
    let refuse = || {
        CliError::Command(format!("The manifest file refers to \"{rel}\" which is outside of the source directory or not a regular file."))
    };
    if !is_safe_relative(rel) {
        return Err(refuse());
    }
    let p = dir.join(rel);
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => return Err(refuse()),
        Ok(_) => {}
        // not present: the caller reports "does not appear to be in the source directory"
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(refuse()),
    }
    let root = std::fs::canonicalize(dir).map_err(|_| refuse())?;
    let real = std::fs::canonicalize(&p).map_err(|_| refuse())?;
    if !real.starts_with(&root) {
        return Err(refuse());
    }
    Ok(Some(p))
}

/// [`source_in`] for a file that must exist (copying): if it is missing, the same
/// `FileNotFoundError` as when copying in upstream.
pub fn existing_source_in(dir: &Path, rel: &str) -> CliResult<PathBuf> {
    source_in(dir, rel)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("No such file or directory: '{rel}'")).into())
}

/// Archive name → path under `root` (directory export): normal components only.
pub fn arc_target(root: &Path, arcname: &str) -> CliResult<PathBuf> {
    let mut p = root.to_path_buf();
    for part in arcname.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        // `\` is a separator on Windows: check the parts individually
        let bad = part.contains('\0')
            || part.starts_with('\\')
            || part.split('\\').any(|sub| sub == ".." || has_drive(sub))
            || !matches!(Path::new(part).components().next(), Some(Component::Normal(_)));
        if bad {
            return Err(CliError::Command(format!("Refusing to export to unsafe path \"{arcname}\"")));
        }
        p.push(part);
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative() {
        for ok in ["0000001.pdf", "2024/Firma/Titel.pdf", "a/./b.pdf", "a..b.pdf", "..x/y", "ACME Corp\\2019\\Letter (edited).pdf"] {
            assert!(is_safe_relative(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a\\..\\x",
            "..\\x",
            "\\foo",
            "\\\\srv\\share",
            "C:\\foo",
            "C:foo",
            "a\0b",
            "//srv/share",
        ] {
            assert!(!is_safe_relative(bad), "{bad}");
        }
    }

    #[test]
    fn target_in_rejects_escape() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("originals");
        std::fs::create_dir_all(&root).unwrap();
        assert!(target_in(&root, "x/y.pdf", "original").is_ok());
        for bad in ["/home/paperless/.ssh/authorized_keys", "../escape.pdf", "..\\escape.pdf", "\\escape.pdf", "C:\\escape.pdf"] {
            let e = target_in(&root, bad, "original").unwrap_err().to_string();
            assert!(e.contains("Refusing to import original"), "{bad}: {e}");
        }
    }

    #[test]
    fn arc_target_filters_dotdot() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(arc_target(d.path(), "a/./b.pdf").unwrap(), d.path().join("a").join("b.pdf"));
        for bad in ["../x", "a/../../x", "a/..\\x", "C:/x", "a/\\x"] {
            assert!(arc_target(d.path(), bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_in_rejects_symlink() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("secret"), b"x").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::os::unix::fs::symlink(d.path().join("secret"), src.join("doc.pdf")).unwrap();
        assert!(source_in(&src, "doc.pdf").is_err());
    }

    #[test]
    fn source_in_rejects_outside() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("secret"), b"x").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("doc.pdf"), b"%PDF").unwrap();
        assert!(matches!(source_in(&src, "doc.pdf"), Ok(Some(_))));
        assert!(source_in(&src, "../secret").is_err());
        let abs = d.path().join("secret").to_string_lossy().into_owned();
        assert!(source_in(&src, &abs).is_err());
        // K-N3: missing file → `None` (no unchecked path), the caller reports the upstream text
        assert!(matches!(source_in(&src, "missing.pdf"), Ok(None)));
        assert!(existing_source_in(&src, "missing.pdf").is_err());
        assert!(existing_source_in(&src, "doc.pdf").is_ok());
    }
}
