// upstream: src/documents/models.py@v3.4.2 (Document.source_path / archive_path:
//           `settings.ORIGINALS_DIR / Path(fname)`), pathlib (PurePosixPath / PureWindowsPath)
//
//! Shared `safe_join` for stored relative file names (security review final C-N1).
//!
//! Used by pl-files (download/preview/bundles), pl-documents-write (write paths) and
//! pl-consumer (worker). Previously each crate had its own version with slightly different
//! handling of `\`; now the same rules apply everywhere:
//!
//! * Separators like the platform's `pathlib.Path`: `/` always, `\` **only on Windows** (there the
//!   reference stores names like `ACME Corp\2019\a.pdf`; on Linux `\` is an ordinary character as in
//!   `PosixPath`, `a\..\b.pdf` is a single file name there).
//! * Lexical normalization (`.`/empty parts are dropped, `..` removes the previous component),
//!   without resolving symlinks – symlinked media directories keep working.
//! * PaperRust additionally (upstream has no containment): `None` for an absolute path, drive/
//!   prefix (`C:`, `\\srv`), NUL or a `..` that leads beyond the root; on Windows also
//!   for components consisting only of dots/spaces (Win32 shortens `.. ` / `...` to `.`/`..`).
//! * On Windows additionally `None` for `:` in a part (`name:stream`), device names (`CON`,
//!   `nul.pdf`, `COM1` …) and a dot/space at the end of a part (security review 2026-11-19
//!   F12). On Linux these are ordinary names (PosixPath) and stay valid.

use std::path::{Component, Path, PathBuf};

/// Path separator like the platform's `pathlib.Path` (`\` only on Windows).
pub const PATH_SEPS: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };

/// `(root / rel)` lexically normalized and confined to `root`; rules see module header.
/// `rel == ""` yields `root` itself (Python: `DIR / Path("") == DIR`).
pub fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains('\0') || rel.starts_with(PATH_SEPS) {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split(PATH_SEPS) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            x => {
                // exactly one normal component (no "C:", no prefix, no root part)
                let mut comps = Path::new(x).components();
                // Security review 2026-11-19 F12: on Windows also no `name:stream`, no
                // device name, no dot/space at the end (also covers `...`/`.. `).
                let win_bad = cfg!(windows) && is_windows_reserved_component(x);
                match (comps.next(), comps.next()) {
                    (Some(Component::Normal(_)), None) if !win_bad => parts.push(x),
                    _ => return None,
                }
            }
        }
    }
    let mut p = root.to_path_buf();
    for x in parts {
        p.push(x);
    }
    Some(p)
}

/// Windows device name, also with an extension or spaces before the dot (`CON`, `nul.pdf`,
/// `COM1.txt`, `lpt5 .txt`, `CONIN$`). Security review 2026-11-19 F12.
pub fn is_windows_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        s => {
            let mut it = s.chars();
            let head: String = it.by_ref().take(3).collect();
            let tail: Vec<char> = it.collect();
            (head == "COM" || head == "LPT")
                && matches!(tail.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
        }
    }
}

/// Name part that Win32 reinterprets: contains `:` (drive/alternate data stream `name:stream`), is
/// a device name or ends with a dot/space (Win32 silently shortens these). Security review
/// 2026-11-19 F12.
pub fn is_windows_reserved_component(part: &str) -> bool {
    part.contains(':') || part.ends_with(['.', ' ']) || is_windows_device_name(part)
}

/// Make a single name part harmless on all platforms (instead of an error): `:`, `/`,
/// `\`, control characters and characters forbidden on Windows → `_`, trailing dots/spaces removed,
/// device names get a `_` prefix; empty or `.`/`..` → `fallback`. The extension is kept.
/// Shared for the working copy in the consumer (F4) and other callers (security review
/// 2026-11-19 F12).
pub fn sanitize_component(part: &str, fallback: &str) -> String {
    let mapped: String = part
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '/' | '\\' | '<' | '>' | '"' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let mut s = mapped.trim_end_matches([' ', '.']).to_string();
    if is_windows_device_name(&s) {
        s.insert(0, '_');
    }
    if s.is_empty() || s == "." || s == ".." {
        s = fallback.to_string();
    }
    s
}

/// Same relative names after [`safe_join`] normalization (platform separators).
pub fn same_rel(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split(PATH_SEPS).filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/");
    norm(a) == norm(b)
}

/// Django `MultiPartParser.sanitize_file_name` (django/http/multipartparser.py): `html.unescape`,
/// base name (`/` and `\`), printable characters only; `""`, `.`, `..` → no name (part skipped).
/// Shared for pl-server (uploads) and pl-documents-write (`update_version`), security review
/// final W-N4.
pub fn sanitize_file_name(name: &str) -> Option<String> {
    let unescaped = html_unescape(name);
    let base = unescaped.rsplit('/').next().unwrap_or("");
    let base = base.rsplit('\\').next().unwrap_or("");
    let s: String = base.chars().filter(|c| py_isprintable(*c)).collect();
    if s.is_empty() || s == "." || s == ".." { None } else { Some(s) }
}

/// Python `str.isprintable` (approximation: not Cc/Cf/Zl/Zp/Zs except the space).
fn py_isprintable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    !(c.is_control()
        || c.is_whitespace()
        || matches!(c as u32, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB))
}

/// `html.unescape` for the common entities (named basics and numeric references).
fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|e| *e <= 12);
        let decoded = end.and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W-N4: like Django incl. `html.unescape` (this was previously missing in pl-documents-write).
    #[test]
    fn sanitize_file_name_like_django() {
        assert_eq!(sanitize_file_name("C:\\x\\a&amp;b.pdf").as_deref(), Some("a&b.pdf"));
        assert_eq!(sanitize_file_name("Rechnung &#228;&#x41;.pdf").as_deref(), Some("Rechnung äA.pdf"));
        assert_eq!(sanitize_file_name("a&unknown;b").as_deref(), Some("a&unknown;b"));
        assert_eq!(sanitize_file_name("../.."), None);
        assert_eq!(sanitize_file_name("x/&#46;&#46;"), None);
        assert_eq!(sanitize_file_name("dir/file\u{0}.txt").as_deref(), Some("file.txt"));
    }

    #[test]
    fn normalizes_and_stays_in_root() {
        let r = Path::new("/m/originals");
        assert_eq!(safe_join(r, "a//b/./c.pdf"), Some(r.join("a").join("b").join("c.pdf")));
        assert_eq!(safe_join(r, "2020/./a/../b.pdf"), Some(r.join("2020").join("b.pdf")));
        assert_eq!(safe_join(r, ""), Some(r.to_path_buf()));
        for bad in ["../x", "a/../../x", "/etc/passwd", "x\0y", "../../etc/passwd"] {
            assert_eq!(safe_join(r, bad), None, "{bad:?}");
        }
    }

    /// C-N1: `\` is a separator exactly on Windows, like `pathlib`; legacy names from the
    /// Windows reference remain findable.
    #[test]
    fn backslash_platform_like_pathlib() {
        let r = Path::new("/m/originals");
        if cfg!(windows) {
            assert_eq!(safe_join(r, "ACME Corp\\2019\\a.pdf"), Some(PathBuf::from("/m/originals/ACME Corp/2019/a.pdf")));
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(PathBuf::from("/m/originals/b.pdf")));
            for bad in ["..\\x", "\\\\srv\\x", "C:\\x", "a/C:x", "a\\.. \\..\\x", "a/.../b"] {
                assert_eq!(safe_join(r, bad), None, "{bad:?}");
            }
            assert!(same_rel("a\\b.pdf", "a/./b.pdf"));
        } else {
            // PosixPath: a single file name, no reinterpretation within the root
            assert_eq!(safe_join(r, "a\\..\\b.pdf"), Some(r.join("a\\..\\b.pdf")));
            assert_eq!(safe_join(r, "..\\x"), Some(r.join("..\\x")));
            assert!(!same_rel("a\\b.pdf", "a/./b.pdf"));
        }
    }

    /// Security review 2026-11-19 F12: parts that Win32 reinterprets (stream `name:stream`, drive
    /// in the middle of the path, device names also with an extension, dot/space at the end) are rejected
    /// by `safe_join` on Windows. On Linux these are ordinary names (PosixPath, upstream parity).
    #[test]
    fn windows_reserved_components() {
        let r = Path::new("/m/originals");
        let cases = [
            "a/x.pdf:strom",
            "x.pdf:Zone.Identifier:$DATA",
            "a/b:c/d.pdf",
            "CON",
            "a/nul.pdf",
            "Com1.txt",
            "LPT9",
            "aux .tar.gz",
            "conin$",
            "a/b./c.pdf",
            "a/b /c.pdf",
            "x.pdf.",
            "x.pdf ",
        ];
        for bad in cases {
            let got = safe_join(r, bad);
            if cfg!(windows) {
                assert_eq!(got, None, "{bad:?}");
            } else {
                assert!(got.is_some_and(|p| p.starts_with(r)), "{bad:?}");
            }
        }
        // normal names that merely look similar stay valid
        for ok in ["console.pdf", "COM10.pdf", "nuller/a.pdf", "LPT.pdf", "a.b/c d.pdf", ".hidden"] {
            assert!(safe_join(r, ok).is_some(), "{ok:?}");
        }
    }

    #[test]
    fn device_names_and_sanitize() {
        for d in ["CON", "con.pdf", "PRN", "AUX", "NUL.tar.gz", "COM1", "com9.x", "LPT1", "lpt5 .txt", "CONOUT$"] {
            assert!(is_windows_device_name(d), "{d:?}");
        }
        for n in ["CONS", "COM10", "COMA", "LPT", "xnul", "nul_", ""] {
            assert!(!is_windows_device_name(n), "{n:?}");
        }
        assert_eq!(sanitize_component("C:start.bat", "document"), "C_start.bat");
        assert_eq!(sanitize_component("x.pdf:strom", "document"), "x.pdf_strom");
        assert_eq!(sanitize_component("a/b\\c", "document"), "a_b_c");
        assert_eq!(sanitize_component("CON.pdf", "document"), "_CON.pdf");
        assert_eq!(sanitize_component("ok. ", "document"), "ok");
        assert_eq!(sanitize_component("...", "x"), "x");
        assert_eq!(sanitize_component("Rechnung März.pdf", "d"), "Rechnung März.pdf");
        // Result is a single, inconspicuous part everywhere
        for n in ["C:start.bat", "D:..\\x", "\\\\server\\share\\f.pdf", "/etc/passwd", "nul", "a:b", " . "] {
            let s = sanitize_component(n, "document");
            assert!(!is_windows_reserved_component(&s), "{n:?} -> {s:?}");
            assert!(safe_join(r_root(), &s).is_some_and(|p| p.parent() == Some(r_root())), "{n:?} -> {s:?}");
        }
    }

    fn r_root() -> &'static Path {
        Path::new("/m/originals")
    }
}
