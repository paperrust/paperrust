//! Python string semantics that upstream uses implicitly (`str.isspace`, `str.strip`, `re` `\s`,
//! `bytes.decode(enc, errors)`, codec names).
//!
//! upstream: CPython 3.12 `Objects/unicodectype.c` (`_PyUnicode_IsWhitespace`), `codecs`.

use encoding_rs::{DecoderResult, Encoding};

/// `str.isspace()` / `re` `\s` for str patterns (CPython: bidi class WS/B/S or category Zs).
pub fn py_isspace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `str.strip()` without argument.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// `str.isprintable()` for one character (CPython: not in Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs except ' ').
/// Approximation without full Unicode tables: control characters, separators, format characters
/// and surrogates/private use.
pub fn py_isprintable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() {
        return false;
    }
    let u = c as u32;
    // Zs (except ' '), Zl, Zp
    if matches!(c, '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}') {
        return false;
    }
    // Cf (common format characters)
    if matches!(
        u,
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e | 0x200b..=0x200f | 0x202a..=0x202e
            | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb
    ) {
        return false;
    }
    // Co / Cn (Private Use, Noncharacters)
    if matches!(u, 0xe000..=0xf8ff | 0xf0000..=0x10ffff | 0xfdd0..=0xfdef) || (u & 0xfffe) == 0xfffe {
        return false;
    }
    true
}

/// `re.sub(r"\s+", " ", text)`.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if py_isspace(c) {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// `bytes.decode("utf-8", "ignore")`: invalid sequences are dropped.
pub fn decode_utf8_ignore(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // valid_up_to is guaranteed to be a valid boundary
                out.push_str(&String::from_utf8_lossy(&bytes[..valid]));
                match e.error_len() {
                    Some(n) => bytes = &bytes[valid + n..],
                    None => return out,
                }
            }
        }
    }
}

/// Error handling when decoding (Python `errors=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errors {
    Ignore,
    Replace,
}

/// Python codec for a name (`codecs.lookup`), as far as relevant for mail/text.
#[derive(Debug, Clone, Copy)]
pub enum PyCodec {
    Utf8,
    Ascii,
    Latin1,
    Utf16,
    Utf16Le,
    Utf16Be,
    Other(&'static Encoding),
}

/// Normalizes like `encodings.normalize_encoding` + aliases; `None` = `LookupError`.
pub fn lookup_codec(label: &str) -> Option<PyCodec> {
    let norm: String = label.trim().to_ascii_lowercase().chars().map(|c| if c == '_' || c == ' ' { '-' } else { c }).collect();
    let compact: String = norm.chars().filter(|c| *c != '-').collect();
    match compact.as_str() {
        "utf8" | "u8" | "utf" | "cp65001" | "utf8sig" => return Some(PyCodec::Utf8),
        "ascii" | "usascii" | "646" | "us" | "ansix3.41968" | "iso646us" | "cp367" | "ibm367" => {
            return Some(PyCodec::Ascii);
        }
        "latin1" | "latin" | "l1" | "iso88591" | "8859" | "cp819" | "iso885911987" | "ibm819" => {
            return Some(PyCodec::Latin1);
        }
        "utf16" | "u16" => return Some(PyCodec::Utf16),
        "utf16le" => return Some(PyCodec::Utf16Le),
        "utf16be" => return Some(PyCodec::Utf16Be),
        "unknown8bit" | "x" | "" => return None,
        _ => {}
    }
    // Python knows "windows-1252" == cp1252 etc.; encoding_rs (WHATWG) knows the usual labels.
    Encoding::for_label(norm.as_bytes()).map(PyCodec::Other)
}

/// `bytes.decode(codec, errors)`.
pub fn decode_with(codec: PyCodec, bytes: &[u8], errors: Errors) -> String {
    match codec {
        PyCodec::Utf8 => match errors {
            Errors::Ignore => decode_utf8_ignore(bytes),
            Errors::Replace => String::from_utf8_lossy(bytes).into_owned(),
        },
        PyCodec::Ascii => bytes
            .iter()
            .filter_map(|&b| {
                if b < 0x80 {
                    Some(b as char)
                } else if errors == Errors::Replace {
                    Some('\u{fffd}')
                } else {
                    None
                }
            })
            .collect(),
        PyCodec::Latin1 => bytes.iter().map(|&b| b as char).collect(),
        PyCodec::Utf16 => {
            if bytes.starts_with(&[0xfe, 0xff]) {
                decode_enc(encoding_rs::UTF_16BE, &bytes[2..], errors)
            } else if bytes.starts_with(&[0xff, 0xfe]) {
                decode_enc(encoding_rs::UTF_16LE, &bytes[2..], errors)
            } else {
                decode_enc(encoding_rs::UTF_16LE, bytes, errors)
            }
        }
        PyCodec::Utf16Le => decode_enc(encoding_rs::UTF_16LE, bytes, errors),
        PyCodec::Utf16Be => decode_enc(encoding_rs::UTF_16BE, bytes, errors),
        PyCodec::Other(enc) => decode_enc(enc, bytes, errors),
    }
}

fn decode_enc(enc: &'static Encoding, bytes: &[u8], errors: Errors) -> String {
    let mut dec = enc.new_decoder_without_bom_handling();
    let cap = dec.max_utf8_buffer_length_without_replacement(bytes.len()).unwrap_or(bytes.len().saturating_mul(3)).saturating_add(16);
    let mut out = String::with_capacity(cap.min(64 * 1024 * 1024));
    let mut input = bytes;
    loop {
        if out.capacity() - out.len() < 16 {
            out.reserve(1024);
        }
        let (res, read) = dec.decode_to_string_without_replacement(input, &mut out, true);
        input = &input[read..];
        match res {
            DecoderResult::InputEmpty => return out,
            DecoderResult::OutputFull => out.reserve(input.len().saturating_mul(3).max(64)),
            DecoderResult::Malformed(_, _) => {
                if errors == Errors::Replace {
                    out.push('\u{fffd}');
                }
            }
        }
    }
}

/// `value.decode(encoding or 'utf-8', 'ignore')` falling back to UTF-8 for an unknown codec
/// (imap_tools `decode_value`).
pub fn decode_value(bytes: &[u8], encoding: Option<&str>) -> String {
    let codec = match encoding {
        Some(e) => lookup_codec(e).unwrap_or(PyCodec::Utf8),
        None => PyCodec::Utf8,
    };
    decode_with(codec, bytes, Errors::Ignore)
}

/// Python `str.splitlines()` (without `keepends`).
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let is_break = matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if is_break {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r'
                && let Some(&(j, '\n')) = it.peek()
            {
                it.next();
                end = j + 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_set() {
        assert!(py_isspace('\u{1c}'));
        assert!(py_isspace('\u{85}'));
        assert!(!py_isspace('\u{200b}'));
        assert_eq!(collapse_whitespace("a \t\n b\u{a0}c"), "a b c");
        assert_eq!(py_strip("\u{1f} x \u{3000}"), "x");
    }

    #[test]
    fn utf8_ignore() {
        assert_eq!(decode_utf8_ignore(b"a\xffb\xc3"), "ab");
        assert_eq!(decode_utf8_ignore("ä".as_bytes()), "ä");
    }

    #[test]
    fn codecs() {
        assert_eq!(decode_value(b"\xfc", Some("ISO-8859-1")), "ü");
        assert_eq!(decode_value(b"\x80", Some("latin1")), "\u{80}");
        assert_eq!(decode_value(b"\x80", Some("windows-1252")), "€");
        assert_eq!(decode_value(b"a\xfcb", Some("us-ascii")), "ab");
        assert_eq!(decode_value(b"\xc3\xbc", Some("x-unknown-foo")), "ü");
        assert_eq!(decode_value(b"\xc3\xbc", None), "ü");
    }

    #[test]
    fn splitlines() {
        assert_eq!(py_splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
        assert_eq!(py_splitlines("a"), vec!["a"]);
        assert_eq!(py_splitlines(""), Vec::<&str>::new());
    }
}
