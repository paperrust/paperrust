// upstream: Python stdlib `re` semantics as used by src/documents/matching.py@v3.4.2
//
// Helpers that reproduce the parts of CPython's `re` module (str patterns, Unicode)
// that matching.py relies on:
//   - `\w`  = str.isalnum() or "_"   -> approximated by [\p{L}\p{N}_] (identical up to
//              Unicode-version differences, CPython 3.12 uses Unicode 15.0)
//   - `\s`  = str.isspace()          -> \p{White_Space} plus U+001C..U+001F
//   - `\b`  = boundary between a `\w` and a non-`\w` char (or text edge)
//   - re.escape / normspace / findall used by `_split_match`
//   - textwrap.shorten(text, width=80, placeholder="…") used in log messages

use std::sync::LazyLock;

use regex::Regex;

static PY_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").unwrap());

/// Python `re` `\w` for a single char (str pattern, Unicode).
pub fn is_py_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    let mut buf = [0u8; 4];
    PY_WORD.is_match(c.encode_utf8(&mut buf))
}

/// Python `str.isspace()` / `re` `\s` for a single char.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Regex fragment equal to Python `re` `\s`.
pub const PY_SPACE_CLASS: &str = r"[\s\x1C-\x1F]";

/// Python `re` `\b` at byte offset `pos` of `text`.
pub fn py_word_boundary(text: &str, pos: usize) -> bool {
    let before = text[..pos].chars().next_back().is_some_and(is_py_word);
    let after = text[pos..].chars().next().is_some_and(is_py_word);
    before != after
}

/// A Python-`re` search for `\b<inner>\b` where `inner` is a pattern built from
/// escaped literal pieces joined by `\s+` (as produced by `_split_match` or
/// `re.escape`). For such patterns every start position has at most one possible
/// end position, so checking the boundaries after a regex-crate match is exact.
pub struct BoundedSearch {
    inner: Regex,
}

impl BoundedSearch {
    pub fn new(inner_pattern: &str, ignore_case: bool) -> Result<Self, regex::Error> {
        let inner = regex::RegexBuilder::new(inner_pattern)
            .case_insensitive(ignore_case)
            .size_limit(64 * 1024 * 1024)
            .build()?;
        Ok(Self { inner })
    }

    /// Equivalent of `bool(re.search(rf"\b{inner}\b", text, flags))`.
    pub fn is_match(&self, text: &str) -> bool {
        let mut pos = 0;
        while pos <= text.len() {
            let Some(m) = self.inner.find_at(text, pos) else {
                return false;
            };
            if py_word_boundary(text, m.start()) && py_word_boundary(text, m.end()) {
                return true;
            }
            // try the next start position (Python tries every start position)
            pos = m.start() + text[m.start()..].chars().next().map_or(1, char::len_utf8);
        }
        false
    }
}

/// Escape a literal for the regex crate (semantically equal to Python `re.escape`).
pub fn escape(s: &str) -> String {
    regex::escape(s)
}

/// `re.compile(r"\s+").sub(" ", s)` with Python whitespace.
pub fn normspace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if is_py_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Python `str.strip()` (strips Unicode whitespace as `str.isspace`).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `re.compile(r'"([^"]+)"|(\S+)').findall(s)` -> list of (group1, group2).
/// `\S` is the Python non-whitespace class.
pub fn findterms(s: &str) -> Vec<(String, String)> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if c == '"' {
            // try "([^"]+)"
            let mut j = i + 1;
            while j < chars.len() && chars[j].1 != '"' {
                j += 1;
            }
            if j < chars.len() && j > i + 1 {
                let start = chars[i + 1].0;
                let end = chars[j].0;
                out.push((s[start..end].to_string(), String::new()));
                i = j + 1;
                continue;
            }
            // fall through to (\S+) alternative
        }
        if !is_py_space(c) {
            let mut j = i;
            while j < chars.len() && !is_py_space(chars[j].1) {
                j += 1;
            }
            let start = chars[i].0;
            let end = if j < chars.len() { chars[j].0 } else { s.len() };
            out.push((String::new(), s[start..end].to_string()));
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// `textwrap.shorten(text, width, placeholder="…")`.
pub fn shorten(text: &str, width: usize) -> String {
    let placeholder = "…";
    let words: Vec<&str> = text.split(is_py_space).filter(|w| !w.is_empty()).collect();
    let collapsed = words.join(" ");
    if collapsed.chars().count() <= width {
        return collapsed;
    }
    let max = width.saturating_sub(placeholder.chars().count());
    let mut out = String::new();
    for w in &words {
        let add = if out.is_empty() { w.chars().count() } else { w.chars().count() + 1 };
        if out.chars().count() + add > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        // textwrap drops everything if even the first word does not fit
        return placeholder.trim_start().to_string();
    }
    out.push_str(placeholder);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries() {
        assert!(py_word_boundary("ab cd", 0));
        assert!(py_word_boundary("ab cd", 2));
        assert!(!py_word_boundary("ab cd", 1));
        assert!(!py_word_boundary("", 0));
        // U+0301 (combining acute, Mn) is not \w in Python re
        assert!(!is_py_word('\u{301}'));
        // ½ (No) is \w in Python re
        assert!(is_py_word('½'));
    }

    #[test]
    fn findterms_like_python() {
        let t = findterms(r#"  some random  words "with   quotes  " and   spaces"#);
        let terms: Vec<String> = t.into_iter().map(|(a, b)| if a.is_empty() { b } else { a }).collect();
        assert_eq!(terms, vec!["some", "random", "words", "with   quotes  ", "and", "spaces"]);
        // an empty quote pair is matched by \S+
        let t = findterms(r#"a "" b"#);
        assert_eq!(t[1], (String::new(), "\"\"".to_string()));
        // unterminated quote
        let t = findterms(r#""abc"#);
        assert_eq!(t[0], (String::new(), "\"abc".to_string()));
    }

    #[test]
    fn shorten_like_textwrap() {
        assert_eq!(shorten("[", 80), "[");
        let long = "a ".repeat(60);
        let s = shorten(&long, 80);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 80);
    }
}
