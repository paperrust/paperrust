//! DRF `SearchFilter` (`?search=`) of the document list.
//!
//! upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.search_fields =
//! ("title", "correspondent__name", "effective_content")), rest_framework/filters.py
//! (SearchFilter, search_smart_split), django/utils/text.py (smart_split,
//! unescape_string_literal)

use sea_query::{Alias, ExprTrait, Query};

use crate::error::FilterError;
use crate::pyval::{py_is_space, py_strip};
use crate::sql::{CORRESPONDENT, Cond, TextLookup, col, doc_col, effective_content, text_lookup};
use crate::textfold::{UpperFold, folded_lookup};

/// Django's `smart_split` (regex from django/utils/text.py re-implemented).
pub fn smart_split(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let is_plain = |c: char| !py_is_space(c) && c != '\'' && c != '"';
    // Tries to read a closed quote block starting at `j`; returns the end position.
    let quoted = |j: usize| -> Option<usize> {
        let q = *chars.get(j)?;
        if q != '"' && q != '\'' {
            return None;
        }
        let mut k = j + 1;
        while k < n {
            let c = chars[k];
            if c == q {
                return Some(k + 1);
            }
            if c == '\\' {
                // `\\.`: `.` without DOTALL does not match a line break
                if k + 1 < n && chars[k + 1] != '\n' {
                    k += 2;
                    continue;
                }
                return None;
            }
            k += 1;
        }
        None
    };
    while i < n {
        if py_is_space(chars[i]) {
            i += 1;
            continue;
        }
        // Alternative 1: [^\s'"]* ( quoted [^\s'"]* )+
        let mut j = i;
        while j < n && is_plain(chars[j]) {
            j += 1;
        }
        let mut end = None;
        let mut k = j;
        while let Some(e) = quoted(k) {
            let mut m = e;
            while m < n && is_plain(chars[m]) {
                m += 1;
            }
            end = Some(m);
            k = m;
        }
        let stop = match end {
            Some(e) => e,
            None => {
                // Alternative 2: \S+
                let mut m = i;
                while m < n && !py_is_space(chars[m]) {
                    m += 1;
                }
                m
            }
        };
        out.push(chars[i..stop].iter().collect());
        i = stop;
    }
    out
}

/// `django.utils.text.unescape_string_literal`
fn unescape_string_literal(s: &str) -> String {
    let quote = s.chars().next().unwrap_or('"');
    let inner: String = {
        let cs: Vec<char> = s.chars().collect();
        if cs.len() >= 2 { cs[1..cs.len() - 1].iter().collect() } else { String::new() }
    };
    inner.replace(&format!("\\{quote}"), &quote.to_string()).replace("\\\\", "\\")
}

/// DRF `search_smart_split`
pub fn search_smart_split(search_terms: &str) -> Vec<String> {
    let mut split_terms = Vec::new();
    for term in smart_split(search_terms) {
        let term = term.trim_matches(',');
        let first = term.chars().next();
        let last = term.chars().last();
        if matches!(first, Some('"' | '\'')) && first == last {
            split_terms.push(unescape_string_literal(term));
        } else {
            for sub in term.split(',') {
                if !sub.is_empty() {
                    split_terms.push(py_strip(sub).to_string());
                }
            }
        }
    }
    split_terms
}

/// Condition for `?search=<value>` (None/empty → no filter).
pub fn search_cond(value: Option<&str>) -> Result<Cond, FilterError> {
    search_cond_folded(value, None)
}

/// Like [`search_cond`], title/content in the fast form with a calibrated DB
/// ([`crate::textfold`]; `fold = None` = Django form).
pub fn search_cond_folded(value: Option<&str>, fold: Option<&UpperFold>) -> Result<Cond, FilterError> {
    let value = value.unwrap_or("");
    // CharField(trim_whitespace=False, allow_blank=True).run_validation(value)
    if value.contains('\0') {
        return Err(FilterError::list("Null characters are not allowed."));
    }
    let terms = search_smart_split(value);
    if terms.is_empty() {
        return Ok(Cond::True);
    }
    let mut acc = Cond::True;
    for term in &terms {
        let corr = Query::select()
            .column((Alias::new("sc"), Alias::new("id")))
            .from_as(Alias::new(CORRESPONDENT), Alias::new("sc"))
            .and_where(text_lookup(col("sc", "name"), TextLookup::IContains, term))
            .to_owned();
        let term_cond = folded_lookup(fold, doc_col("title"), TextLookup::IContains, term)
            .or(doc_col("correspondent_id").in_subquery(corr))
            .or(folded_lookup(fold, effective_content(), TextLookup::IContains, term));
        acc = acc.and(Cond::Expr(term_cond));
    }
    Ok(acc)
}

/// `?search=` as AST (L1): per term `title ICONTAINS t OR correspondent.name ICONTAINS t OR
/// effective_content ICONTAINS t`, AND across the terms. Same errors as [`search_cond`].
pub fn search_pred(value: Option<&str>) -> Result<pl_domain::filter::Pred, FilterError> {
    use pl_domain::filter::{NamedRel, Pred, TextCol, TextLookup as L};
    let value = value.unwrap_or("");
    if value.contains('\0') {
        return Err(FilterError::list("Null characters are not allowed."));
    }
    let mut acc = Pred::True;
    for term in search_smart_split(value) {
        let t = Pred::Text { col: TextCol::Title, lookup: L::IContains, value: term.clone() }
            .or(Pred::RelName { rel: NamedRel::Correspondent, lookup: L::IContains, value: term.clone() })
            .or(Pred::Text { col: TextCol::Content, lookup: L::IContains, value: term });
        acc = acc.and(t);
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_split_like_django() {
        assert_eq!(smart_split("This is \"a person\" test."), vec!["This", "is", "\"a person\"", "test."]);
        assert_eq!(smart_split("Another 'person\\'s' test."), vec!["Another", "'person\\'s'", "test."]);
        assert_eq!(smart_split("A \"\\\"funky\\\" style\" test."), vec!["A", "\"\\\"funky\\\" style\"", "test."]);
        assert_eq!(smart_split("a\"b c\"d e"), vec!["a\"b c\"d", "e"]);
        assert_eq!(smart_split("\"unclosed quote"), vec!["\"unclosed", "quote"]);
    }

    #[test]
    fn drf_search_split() {
        assert_eq!(search_smart_split("foo,bar baz"), vec!["foo", "bar", "baz"]);
        assert_eq!(search_smart_split("\"hello world\" x"), vec!["hello world", "x"]);
        assert_eq!(search_smart_split(",\"a b\","), vec!["a b"]);
        assert_eq!(search_smart_split("  "), Vec::<String>::new());
    }

    #[test]
    fn null_char_rejected() {
        let e = search_cond(Some("a\0b")).unwrap_err();
        match e {
            FilterError::Validation(v) => assert_eq!(v, serde_json::json!(["Null characters are not allowed."])),
            other => panic!("{other:?}"),
        }
    }
}
