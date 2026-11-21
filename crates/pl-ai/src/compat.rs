//! Compatibility mode: exact reproduction of upstream `paperless_ai/matching.py@v3.4.2`.
//!
//! * [`py_normalize`] = `_normalize(s)`: `s.lower()`, `re.sub(r"[^\w\s]", "", s)`, `s.strip()`
//!   with Python's Unicode semantics (`str.lower`, `\w` = `str.isalnum()` or `_`,
//!   `\s`/`strip` = `str.isspace()`).
//! * [`match_names_to_pool`] = `_match_names_to_queryset(names, queryset)`: per name at most
//!   one object, first exact (normalized), then `difflib.get_close_matches(n=1, cutoff=0.8)`;
//!   a matched object leaves the pool.
//! * [`extract_unmatched_names`] = `extract_unmatched_names(names, matched_objects)`.
//!
//! The pool order is that of the Django query `model.objects.filter(pk__in=...)` –
//! `MatchingModel.Meta.ordering = ("name",)`, i.e. `ORDER BY name` in the DB's collation
//! (PostgreSQL: collation of the database). The caller supplies the pool in this order.

use crate::difflib;

/// upstream `MATCH_THRESHOLD = 0.8`.
pub const MATCH_THRESHOLD: f64 = 0.8;

/// Python's `str.isspace()` for one character (Unicode White_Space plus the
/// information separators U+001C..U+001F, which Python counts as whitespace).
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.isalnum()` for one character (approximately exact): letters (L*) and
/// numbers (N*). Rust's `is_alphanumeric` additionally includes `Other_Alphabetic`
/// (combining characters M* and circled letters So), which Python does **not** count as
/// alphanumeric – they are excluded here.
pub fn py_isalnum(c: char) -> bool {
    if !c.is_alphanumeric() {
        return false;
    }
    if unicode_normalization::char::is_combining_mark(c) {
        return false;
    }
    // So characters with Other_Alphabetic: ⒶⓐⓏ…, 🄰…, 🅐…, 🅰…
    !matches!(c as u32, 0x24B6..=0x24E9 | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189)
}

/// Python's `\w` (Unicode mode of `re`).
pub fn py_is_word(c: char) -> bool {
    c == '_' || py_isalnum(c)
}

/// upstream `matching._normalize`.
pub fn py_normalize(s: &str) -> String {
    let lowered = s.to_lowercase();
    let filtered: String = lowered.chars().filter(|c| py_is_word(*c) || py_isspace(*c)).collect();
    filtered.trim_matches(py_isspace).to_string()
}

/// An object in the pool (id + name).
pub trait Named {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
}

impl Named for (i64, String) {
    fn id(&self) -> i64 {
        self.0
    }
    fn name(&self) -> &str {
        &self.1
    }
}

/// upstream `_match_names_to_queryset(names, queryset)`. `None`/empty names are
/// skipped (`if not name: continue`). Returns the matched objects in match order.
pub fn match_names_to_pool<T: Named + Clone>(names: &[Option<&str>], pool: &[T]) -> Vec<T> {
    let mut objects: Vec<T> = pool.to_vec();
    let mut object_names: Vec<String> = objects.iter().map(|o| py_normalize(o.name())).collect();
    let mut results = Vec::new();
    for name in names {
        let Some(name) = name else { continue };
        if name.is_empty() {
            continue;
        }
        let target = py_normalize(name);
        let index = if let Some(i) = object_names.iter().position(|n| *n == target) {
            i
        } else {
            let poss: Vec<&str> = object_names.iter().map(String::as_str).collect();
            let m = difflib::get_close_matches(&target, &poss, 1, MATCH_THRESHOLD);
            let Some(best) = m.first() else { continue };
            let Some(i) = object_names.iter().position(|n| n == best) else { continue };
            i
        };
        object_names.remove(index);
        results.push(objects.remove(index));
    }
    results
}

/// upstream `extract_unmatched_names(names, matched_objects)`.
pub fn extract_unmatched_names<T: Named>(names: &[&str], matched: &[T]) -> Vec<String> {
    let matched_names: std::collections::HashSet<String> = matched.iter().map(|o| py_normalize(o.name())).collect();
    names.iter().filter(|n| !matched_names.contains(&py_normalize(n))).map(|n| n.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(names: &[&str]) -> Vec<(i64, String)> {
        names.iter().enumerate().map(|(i, n)| (i as i64 + 1, n.to_string())).collect()
    }

    #[test]
    fn upstream_test_matching_fuzzy() {
        // paperless_ai/tests/test_matching.py::test_match_tags_with_fuzzy_matching
        let p = pool(&["Test Tag 1", "Test Tag 2"]);
        let r = match_names_to_pool(&[Some("Test Taag 1"), Some("Teest Tag 2")], &p);
        assert_eq!(r.iter().map(|o| o.0).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn upstream_test_empty_names() {
        let p = pool(&["Test Tag 1"]);
        assert!(match_names_to_pool(&[None, Some(""), Some("   ")], &p).is_empty());
    }

    #[test]
    fn upstream_punctuated_unmatched() {
        let p = pool(&["J Smith"]);
        assert!(extract_unmatched_names(&["J. Smith"], &p).is_empty());
    }
}
