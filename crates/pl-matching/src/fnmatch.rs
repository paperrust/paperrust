// upstream: CPython 3.12 Lib/fnmatch.py (translate, fnmatch, fnmatchcase), used by
//           src/documents/matching.py@v3.4.2 (workflow filename/path filters).
//
// `fnmatch()` calls `os.path.normcase()`, which is a no-op on POSIX (paperless runs on
// Linux/Docker); we implement the POSIX behaviour (case-sensitive, '/' not special).

use crate::matching::python_re_escape;
use crate::regex_compat::translate_python_pattern;

enum Piece {
    Star,
    Lit(String),
}

/// `fnmatch.translate(pat)` – returns the Python regex text, e.g. `(?s:.*\.pdf)\Z`.
pub fn translate(pat: &str) -> String {
    let p: Vec<char> = pat.chars().collect();
    let n = p.len();
    let mut res: Vec<Piece> = Vec::new();
    let mut i = 0;
    while i < n {
        let c = p[i];
        i += 1;
        if c == '*' {
            if !matches!(res.last(), Some(Piece::Star)) {
                res.push(Piece::Star);
            }
        } else if c == '?' {
            res.push(Piece::Lit(".".into()));
        } else if c == '[' {
            let mut j = i;
            if j < n && p[j] == '!' {
                j += 1;
            }
            if j < n && p[j] == ']' {
                j += 1;
            }
            while j < n && p[j] != ']' {
                j += 1;
            }
            if j >= n {
                res.push(Piece::Lit("\\[".into()));
            } else {
                let stuff_raw: String = p[i..j].iter().collect();
                let mut stuff: String;
                if !stuff_raw.contains('-') {
                    stuff = stuff_raw.replace('\\', r"\\");
                } else {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut k = if p[i] == '!' { i + 2 } else { i + 1 };
                    loop {
                        // k = pat.find('-', k, j)
                        let found = (k..j).find(|&x| x < n && p[x] == '-');
                        let Some(kk) = found else { break };
                        chunks.push(p[i..kk].iter().collect());
                        i = kk + 1;
                        k = kk + 3;
                    }
                    let chunk: String = p[i..j].iter().collect();
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    } else if let Some(last) = chunks.last_mut() {
                        last.push('-');
                    }
                    // Remove empty ranges -- invalid in RE.
                    let mut k = chunks.len().saturating_sub(1);
                    while k > 0 {
                        let prev_last = chunks[k - 1].chars().last();
                        let cur_first = chunks[k].chars().next();
                        if let (Some(a), Some(b)) = (prev_last, cur_first) {
                            if a > b {
                                let mut merged: String = {
                                    let mut s: Vec<char> = chunks[k - 1].chars().collect();
                                    s.pop();
                                    s.into_iter().collect()
                                };
                                merged.extend(chunks[k].chars().skip(1));
                                chunks[k - 1] = merged;
                                chunks.remove(k);
                            }
                        }
                        k -= 1;
                    }
                    stuff = chunks
                        .iter()
                        .map(|s| s.replace('\\', r"\\").replace('-', r"\-"))
                        .collect::<Vec<_>>()
                        .join("-");
                }
                // Escape set operations (&&, ~~ and ||).
                let mut esc = String::new();
                for ch in stuff.chars() {
                    if matches!(ch, '&' | '~' | '|') {
                        esc.push('\\');
                    }
                    esc.push(ch);
                }
                stuff = esc;
                i = j + 1;
                if stuff.is_empty() {
                    res.push(Piece::Lit("(?!)".into()));
                } else if stuff == "!" {
                    res.push(Piece::Lit(".".into()));
                } else {
                    if let Some(rest) = stuff.strip_prefix('!') {
                        stuff = format!("^{rest}");
                    } else if stuff.starts_with('^') || stuff.starts_with('[') {
                        stuff = format!("\\{stuff}");
                    }
                    res.push(Piece::Lit(format!("[{stuff}]")));
                }
            }
        } else {
            res.push(Piece::Lit(python_re_escape(&c.to_string())));
        }
    }

    let mut out = String::new();
    let mut idx = 0;
    while idx < res.len() {
        if let Piece::Lit(s) = &res[idx] {
            out.push_str(s);
            idx += 1;
        } else {
            break;
        }
    }
    while idx < res.len() {
        // res[idx] is STAR
        idx += 1;
        if idx == res.len() {
            out.push_str(".*");
            break;
        }
        let mut fixed = String::new();
        while idx < res.len() {
            match &res[idx] {
                Piece::Lit(s) => {
                    fixed.push_str(s);
                    idx += 1;
                }
                Piece::Star => break,
            }
        }
        if idx == res.len() {
            out.push_str(".*");
            out.push_str(&fixed);
        } else {
            out.push_str(&format!("(?>.*?{fixed})"));
        }
    }
    format!("(?s:{out})\\Z")
}

/// `fnmatch.fnmatchcase(name, pat)` (== `fnmatch.fnmatch` on POSIX).
pub fn fnmatch(name: &str, pat: &str) -> bool {
    let py = translate(pat);
    let rust = translate_python_pattern(&format!(r"\A{py}"));
    match fancy_regex::Regex::new(&rust) {
        Ok(re) => re.is_match(name).unwrap_or(false),
        Err(e) => {
            tracing::error!(target: "paperless.matching", "fnmatch pattern {pat:?} not translatable: {e}");
            false
        }
    }
}

/// Python-`re` search with IGNORECASE of the prefilter regex
/// (`fnmatch_translate(pat).lstrip("^").rstrip("$")` used as `original_filename__iregex`).
/// Semantics of Django's SQLite/Python REGEXP: `re.search`, i.e. unanchored at the start.
pub fn iregex_search(regex_text: &str, value: &str) -> bool {
    let rust = translate_python_pattern(regex_text);
    match fancy_regex::RegexBuilder::new(&rust).case_insensitive(true).build() {
        Ok(re) => re.is_match(value).unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_like_cpython() {
        // expected strings produced with CPython 3.12 fnmatch.translate
        assert_eq!(translate("*.pdf"), r"(?s:.*\.pdf)\Z");
        assert_eq!(translate("*simple*"), r"(?s:(?>.*?simple).*)\Z");
        assert_eq!(translate("a?[!b]c"), r"(?s:a.[^b]c)\Z");
        assert_eq!(translate("[a-c]*"), r"(?s:[a-c].*)\Z");
        assert_eq!(translate("[z-a]"), r"(?s:(?!))\Z");
        assert_eq!(translate("[!]"), r"(?s:\[!\])\Z");
        assert_eq!(translate("foo bar"), r"(?s:foo\ bar)\Z");
        assert_eq!(translate("[&]"), r"(?s:[\&])\Z");
        assert_eq!(translate("**/x"), r"(?s:.*/x)\Z");
    }

    #[test]
    fn matching() {
        assert!(fnmatch("simple.pdf", "*simple*"));
        assert!(!fnmatch("simple.pdf", "*foobar*"));
        assert!(fnmatch("/tmp/scratch/simple.pdf", "*/scratch/*"));
        assert!(fnmatch("a.pdf", "?.pdf"));
        assert!(!fnmatch("ab.pdf", "?.pdf"));
        assert!(fnmatch("x[y", "x[y"));
        assert!(fnmatch("a\nb", "a*b"));
        assert!(!fnmatch("A.PDF", "*.pdf"));
        assert!(fnmatch("b", "[a-c]"));
        assert!(!fnmatch("x", "[z-a]"));
    }
}
