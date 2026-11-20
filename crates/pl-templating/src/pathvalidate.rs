//! `pathvalidate.sanitize_filename(value, replacement_text=...)` (pathvalidate 3.3.1, platform
//! "universal", file system encoding UTF-8, max_len 255 bytes).
//!
//! upstream: pathvalidate/_filename.py@3.3.1 (FileNameSanitizer.sanitize, FileNameValidator.validate),
//! pathvalidate/_base.py (_validate_reserved_keywords), pathvalidate/handler.py.

use crate::pystr::py_strip;

const MAX_LEN: usize = 255;

/// Characters that are replaced on the "universal" platform (`_RE_INVALID_WIN_FILENAME`).
fn is_invalid_char(c: char) -> bool {
    let u = c as u32;
    // unprintable_ascii_chars: all ASCII characters except string.printable (\t\n\r\x0b\x0c are
    // printable, but are in _INVALID_WIN_PATH_CHARS) -> all control characters 0..=0x1f and 0x7f
    u < 0x20 || u == 0x7f || matches!(c, '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\')
}

pub(crate) fn is_reserved(name: &str) -> bool {
    let up = name.to_uppercase();
    matches!(up.as_str(), "CON" | "PRN" | "AUX" | "CLOCK$" | "NUL" | ":")
        || ["COM", "LPT"].iter().any(|p| {
            up.strip_prefix(p).is_some_and(|rest| {
                matches!(rest, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}")
            })
        })
}

/// `__extract_root_name`.
fn root_name(path: &str) -> &str {
    if path == "." || path == ".." {
        return path;
    }
    if path.starts_with("...") {
        return path;
    }
    // re.match(r"([^\.]+)", basename) – basename == path (no "/" left)
    let end = path.find('.').unwrap_or(path.len());
    &path[..end]
}

/// Truncate UTF-8 bytes, discard an incomplete last character (`truncate_str`).
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `pathvalidate.sanitize_filename(value, replacement_text=replacement)`.
pub fn sanitize_filename(value: &str, replacement: &str) -> String {
    // validate_pathtype: empty or whitespace-only values -> NULL_NAME -> ""
    if py_strip(value).is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(value.len());
    for c in value.chars() {
        if is_invalid_char(c) {
            s.push_str(replacement);
        } else {
            s.push(c);
        }
    }
    let mut s = truncate_bytes(&s, MAX_LEN).to_string();

    // self._validator.validate(sanitized) – only the first error is handled
    if py_strip(&s).is_empty() {
        return String::new(); // NULL_NAME -> null_value_handler -> ""
    }
    // (absolute paths, length and invalid characters are no longer possible after the replacement)
    let root = root_name(&s).to_string();
    if is_reserved(&root) || is_reserved(&s) {
        // ReservedNameHandler.add_trailing_underscore; re.sub(re.escape(root), root + "_", s)
        if root != "." && root != ".." && !root.is_empty() {
            s = s.replace(&root, &format!("{root}_"));
        } else if root.is_empty() {
            // re.sub with an empty pattern inserts "_" at every position
            let mut out = String::from("_");
            for c in s.chars() {
                out.push(c);
                out.push('_');
            }
            s = out;
        }
        return s;
    }
    if s == "." || s == ".." {
        return s;
    }
    let ends_bad = s.ends_with(' ') || s.ends_with('.');
    let starts_bad = s.starts_with(' ');
    if ends_bad || starts_bad {
        let mut t = s.trim_start_matches(' ').trim_end_matches(' ').to_string();
        if t != "." && t != ".." {
            t = t.trim_end_matches([' ', '.']).to_string();
        }
        return t;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename as s;

    /// Expected values generated with pathvalidate 3.3.1.
    #[test]
    fn matches_pathvalidate() {
        assert_eq!(s("This. is the title.", "-"), "This. is the title");
        assert_eq!(s("my\\invalid/../title:yay", "-"), "my-invalid-..-title-yay");
        assert_eq!(s(".-none-.", "-"), ".-none-");
        assert_eq!(s("CON", "-"), "CON_");
        assert_eq!(s("con.txt", "-"), "con_.txt");
        assert_eq!(s("..", "-"), "..");
        assert_eq!(s(".", "-"), ".");
        assert_eq!(s(" a ", "-"), "a");
        assert_eq!(s("a\tb", "-"), "a-b");
        assert_eq!(s("", "-"), "");
        assert_eq!(s(&"x".repeat(300), "-").len(), 255);
        assert_eq!(s("ä".repeat(200).as_str(), "-").len(), 254);
        for (i, o) in [
            ("nul", "nul_"),
            ("NUL.tar.gz", "NUL_.tar.gz"),
            ("COM1", "COM1_"),
            ("com\u{b9}", "com\u{b9}_"),
            ("lpt9.x", "lpt9_.x"),
            ("a.", "a"),
            ("a. ", "a"),
            (" .a", ".a"),
            ("...", ""),
            ("....x", "....x"),
            ("a/b", "a-b"),
            ("x:y*z?", "x-y-z-"),
            ("\x7fdel", "-del"),
            ("CLOCK$", "CLOCK$_"),
            ("aux.aux", "aux_.aux_"),
            ("Aux", "Aux_"),
            ("   ", ""),
            ("\u{a0}x\u{a0}", "\u{a0}x\u{a0}"),
            ("Prn.", "Prn_."),
            (".CON", ".CON"),
            ("conx", "conx"),
        ] {
            assert_eq!(s(i, "-"), o, "{i:?}");
        }
    }
}
