// upstream: src/paperless/parsers/utils.py@v3.4.2 (post_process_text, read_file_handle_unicode_errors,
//           PDF_TEXT_MIN_LENGTH, is_born_digital_text)
//
// Deliberately an own (small) copy instead of a dependency on pl-parsers (no cross-dependencies
// between domain crates, docs/ARCHITECTURE.md). Same rules as pl-parsers::pdf::post_process_text.

use std::path::Path;

/// `PDF_TEXT_MIN_LENGTH`.
pub const PDF_TEXT_MIN_LENGTH: usize = 50;

/// Python `str.isspace()` / Regex `\s` (Unicode).
pub fn py_isspace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}'
            | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
            | '\u{205f}' | '\u{3000}'
    )
}

fn ws_not_nl(c: char) -> bool {
    py_isspace(c) && c != '\r' && c != '\n'
}

/// `post_process_text`: `None` for `None`/empty/whitespace only.
pub fn post_process_text(text: Option<&str>) -> Option<String> {
    let text = text?;
    if text.is_empty() {
        return None;
    }
    // 1. ([^\S\r\n]+) -> " "
    let mut a = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if ws_not_nl(c) {
            if !in_ws {
                a.push(' ');
                in_ws = true;
            }
        } else {
            a.push(c);
            in_ws = false;
        }
    }
    // 2. ([\n\r]+)([^\S\n\r]+) -> \1   (after step 1 every whitespace run is a " ")
    let mut b = String::with_capacity(a.len());
    let mut prev_nl = false;
    for c in a.chars() {
        if c == ' ' && prev_nl {
            continue;
        }
        prev_nl = c == '\n' || c == '\r';
        b.push(c);
    }
    // 3. ([^\S\n\r]+)$ -> ""   ($ = end or before a trailing \n)
    if b.ends_with(' ') {
        let t = b.trim_end_matches(' ').len();
        b.truncate(t);
    } else if let Some(body) = b.strip_suffix('\n')
        && body.ends_with(' ')
    {
        let t = body.trim_end_matches(' ').len();
        b = format!("{}\n", &body[..t]);
    }
    // 4. strip() + \0 -> " "
    let result = b.trim_matches(py_isspace).replace('\0', " ");
    if result.is_empty() { None } else { Some(result) }
}

/// `read_file_handle_unicode_errors`: BOM detection, UTF-8 with replacement characters, NUL removed.
pub fn decode_text_bytes(raw: &[u8]) -> String {
    let text = if raw.starts_with(&[0xFF, 0xFE]) || raw.starts_with(&[0xFE, 0xFF]) {
        let le = raw[0] == 0xFF;
        let units: Vec<u16> = raw[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        match String::from_utf16(&units) {
            Ok(s) if raw.len().is_multiple_of(2) => s,
            _ => {
                tracing::warn!("Unicode error during text reading, continuing");
                String::from_utf8_lossy(raw).into_owned()
            }
        }
    } else {
        let body = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw);
        match std::str::from_utf8(body) {
            Ok(s) => s.to_string(),
            Err(e) => {
                tracing::warn!("Unicode error during text reading, continuing: {e}");
                // Python: raw.decode("utf-8", errors="replace") – incl. BOM as U+FEFF
                String::from_utf8_lossy(raw).into_owned()
            }
        }
    };
    text.replace('\0', "")
}

pub fn read_file_handle_unicode_errors(path: &Path) -> std::io::Result<String> {
    Ok(decode_text_bytes(&std::fs::read(path)?))
}

/// `is_born_digital_text(text, path)` with a predetermined tag status.
pub fn is_born_digital_text(text: Option<&str>, tagged: impl FnOnce() -> bool) -> bool {
    match text {
        None | Some("") => false,
        Some(t) => tagged() || t.chars().count() > PDF_TEXT_MIN_LENGTH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_process() {
        assert_eq!(post_process_text(None), None);
        assert_eq!(post_process_text(Some(" \x0c \n ")), None);
        assert_eq!(
            post_process_text(Some("  a\t\tb  \n   c  \n")).as_deref(),
            // `$` without MULTILINE: spaces before inner line breaks stay (like Python)
            Some("a b \nc")
        );
        assert_eq!(post_process_text(Some("x\0y")).as_deref(), Some("x y"));
        assert_eq!(post_process_text(Some("a\x0cb")).as_deref(), Some("a b"));
    }

    #[test]
    fn decode_bom() {
        assert_eq!(decode_text_bytes(b"\xef\xbb\xbfhi\0"), "hi");
        assert_eq!(decode_text_bytes(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode_text_bytes(b"a\xffb"), "a\u{fffd}b");
    }

    #[test]
    fn born_digital() {
        assert!(!is_born_digital_text(None, || true));
        assert!(is_born_digital_text(Some("short"), || true));
        assert!(!is_born_digital_text(Some("short"), || false));
        assert!(is_born_digital_text(Some(&"x".repeat(51)), || false));
        assert!(!is_born_digital_text(Some(&"x".repeat(50)), || false));
    }
}
