// upstream: src/documents/search/_errors.py@v3.4.2

/// Errors of the search engine layer.
///
/// `Query` corresponds to upstream `SearchQueryError` (and subclasses): a
/// user-fixable problem. The views map it to HTTP 400 `{"query": [messages...]}`
/// (`search_query_error_messages`: one entry per offending value).
/// Everything else is a server-side condition (HTTP 500 upstream).
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("{}", .0.join("; "))]
    Query(Vec<String>),
    #[error("search index error: {0}")]
    Index(String),
    #[error("internal search error: {0}")]
    Internal(String),
}

impl SearchError {
    /// Single user-facing message (`SearchQueryError(msg)`).
    pub fn query(msg: impl Into<String>) -> Self {
        SearchError::Query(vec![msg.into()])
    }

    /// `InvalidDateQuery(field, value)`; Python `{value!r}`/`{field!r}` repr.
    pub fn invalid_date(field: Option<&str>, value: Option<&str>) -> Self {
        SearchError::query(format!("Invalid date value {} for field {}.", py_repr_opt(value), py_repr_opt(field)))
    }

    /// `InvalidNumberQuery(field, value)`.
    pub fn invalid_number(field: Option<&str>, value: Option<&str>) -> Self {
        SearchError::query(format!("Invalid numeric value {} for field {}.", py_repr_opt(value), py_repr_opt(field)))
    }

    /// `QueryTooLongError(length, limit)`.
    pub fn too_long(length: usize, limit: usize) -> Self {
        SearchError::query(format!(
            "The search query is too long ({length} characters). \
             The maximum allowed length is {limit} characters."
        ))
    }

    /// `MultipleSearchQueryErrors`: flatten user errors into one.
    pub fn multiple(errors: Vec<SearchError>) -> Self {
        let mut msgs = Vec::new();
        for e in errors {
            match e {
                SearchError::Query(m) => msgs.extend(m),
                other => return other,
            }
        }
        SearchError::Query(msgs)
    }

    /// `search_query_error_messages`.
    pub fn messages(&self) -> Option<&[String]> {
        match self {
            SearchError::Query(m) => Some(m),
            _ => None,
        }
    }
}

impl From<tantivy::TantivyError> for SearchError {
    fn from(e: tantivy::TantivyError) -> Self {
        SearchError::Index(e.to_string())
    }
}

impl From<std::io::Error> for SearchError {
    fn from(e: std::io::Error) -> Self {
        SearchError::Index(e.to_string())
    }
}

/// Python `repr()` of `str | None`.
pub fn py_repr_opt(v: Option<&str>) -> String {
    match v {
        None => "None".to_string(),
        Some(s) => py_repr(s),
    }
}

/// Python `repr()` of a `str`: single quotes unless the string contains a
/// single quote and no double quote; escapes `\\`, the chosen quote, `\n`,
/// `\r`, `\t` and non-printable ASCII control characters.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if is_py_nonprintable(c) => {
                let cp = c as u32;
                if cp <= 0xff {
                    out.push_str(&format!("\\x{cp:02x}"));
                } else if cp <= 0xffff {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    out.push_str(&format!("\\U{cp:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Approximation of Python's `str.isprintable()` == false for non-ASCII:
/// C1 controls, line/paragraph separators, BOM/format characters and
/// non-breaking/odd spaces other than U+0020 (Python treats Zs except space
/// as non-printable).
fn is_py_nonprintable(c: char) -> bool {
    let cp = c as u32;
    (0x80..=0xa0).contains(&cp)
        || cp == 0xad
        || matches!(cp, 0x1680 | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064 | 0x3000 | 0xfeff)
        || (0xd800..=0xdfff).contains(&cp)
        || (0xe000..=0xf8ff).contains(&cp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_like_python() {
        assert_eq!(py_repr("abc"), "'abc'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'\"b"), "'a\\'\"b'");
        assert_eq!(py_repr("ä\n"), "'ä\\n'");
        assert_eq!(py_repr_opt(None), "None");
    }

    #[test]
    fn messages() {
        let e = SearchError::invalid_date(Some("created"), Some("foo"));
        assert_eq!(e.messages().unwrap(), ["Invalid date value 'foo' for field 'created'.".to_string()]);
        let e = SearchError::too_long(5000, 4096);
        assert_eq!(e.to_string(), "The search query is too long (5000 characters). The maximum allowed length is 4096 characters.");
    }
}
