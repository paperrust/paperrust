//! Small Python built-in semantics needed for parity (`int(str)`, `str.rstrip()`, text-mode
//! line reading).

/// `str.isspace()` for one character (Python counts U+001C..U+001F as whitespace, Rust not).
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `s.rstrip()` without arguments.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_py_space)
}

/// `int(s)` for a `str` argument (base 10): surrounding whitespace, optional sign, ASCII
/// digits with single underscores between digits. Unicode digits are not accepted
/// (Python would accept e.g. Arabic-Indic digits – practically irrelevant for query strings).
pub fn int(s: &str) -> Option<i128> {
    let t = s.trim_matches(is_py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for b in digits.bytes() {
        match b {
            b'_' => {}
            b'0'..=b'9' => {
                v = v.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
            }
            _ => return None,
        }
    }
    Some(if neg { -v } else { v })
}

/// Lines of a file read in Python text mode with universal newlines, each `rstrip()`ed
/// (`[line.rstrip() for line in f.readlines()]`). `Err` = not valid UTF-8.
pub fn text_lines(bytes: &[u8]) -> Result<Vec<String>, std::str::Utf8Error> {
    let text = std::str::from_utf8(bytes)?;
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        // universal newlines: "\r\n", "\r" and "\n" all end a line
        let end = rest.find(['\n', '\r']);
        match end {
            None => {
                out.push(rstrip(rest).to_string());
                break;
            }
            Some(i) => {
                out.push(rstrip(&rest[..i]).to_string());
                let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
                rest = &rest[i + skip..];
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_like_python() {
        assert_eq!(int(" 5 "), Some(5));
        assert_eq!(int("+5"), Some(5));
        assert_eq!(int("-5"), Some(-5));
        assert_eq!(int("1_000"), Some(1000));
        assert_eq!(int("007"), Some(7));
        for bad in ["", "abc", "1.0", "1__0", "_1", "1_", "- 1", "0x10"] {
            assert_eq!(int(bad), None, "{bad}");
        }
    }

    #[test]
    fn lines_like_python() {
        assert_eq!(text_lines(b"test\ntest2\n").unwrap(), vec!["test", "test2"]);
        assert_eq!(text_lines(b"a\r\nb\rc").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(text_lines(b"a  \t\n\n").unwrap(), vec!["a", ""]);
        assert_eq!(text_lines(b"x\x1f\n").unwrap(), vec!["x"]);
        assert!(text_lines(b"").unwrap().is_empty());
        assert!(text_lines(b"\xff").is_err());
    }
}
