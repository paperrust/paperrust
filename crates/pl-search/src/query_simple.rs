// upstream: src/documents/search/_query.py@v3.4.2
//   normalize_search_text, _CJK_RE, _has_cjk, extract_cjk_text, _parse_cjk_text,
//   _build_cjk_query, _any_of, _build_simple_token_query, parse_simple_query,
//   parse_simple_text_highlight_query, parse_simple_text_query, parse_simple_title_query
//
// Query mode (whoosh grammar) lives in query_user.rs / wc/.

use std::sync::OnceLock;

use regex::Regex;
use tantivy::Index;
use tantivy::query::{BooleanQuery, BoostQuery, EmptyQuery, Occur, Query, QueryParser, RegexQuery};
use unicode_normalization::UnicodeNormalization;

use crate::error::SearchError;
use crate::tokenizer::simple_search_tokens;

pub const SIMPLE_SEARCH_FIELDS: &[&str] = &["simple_title", "simple_content"];
pub const TITLE_SEARCH_FIELDS: &[&str] = &["simple_title"];
pub const CJK_CONTENT_FIELDS: &[&str] = &["bigram_content"];
pub const CJK_TITLE_FIELDS: &[&str] = &["bigram_title"];

/// `normalize_search_text`: NFC.
pub fn normalize_search_text(text: &str) -> String {
    text.nfc().collect()
}

/// `_CJK_RE`: `[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}ーｰ〆ﾞﾟ]+`
/// (Constant pattern, always compiles; `None` only keeps this panic-free.)
pub fn cjk_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}ーｰ〆ﾞﾟ]+").ok())
        .as_ref()
}

/// `_has_cjk`.
pub fn has_cjk(text: &str) -> bool {
    cjk_re().is_some_and(|re| re.is_match(text))
}

/// `_CJK_RE.findall(text)`.
pub fn cjk_runs(text: &str) -> Vec<&str> {
    match cjk_re() {
        Some(re) => re.find_iter(text).map(|m| m.as_str()).collect(),
        None => Vec::new(),
    }
}

/// `extract_cjk_text`: CJK runs joined by a single space ("" when none).
pub fn extract_cjk_text(text: &str) -> String {
    cjk_runs(text).join(" ")
}

/// tantivy-py `Index.parse_query(query, default_field_names)`:
/// `QueryParser::for_index(index, fields)` (disjunction by default, no
/// regexes), unknown/non-indexed field -> error.
pub fn tantivy_parse_query(
    index: &Index,
    query: &str,
    fields: &[&str],
) -> Result<Box<dyn Query>, SearchError> {
    let schema = index.schema();
    let mut default_fields = Vec::with_capacity(fields.len());
    for name in fields {
        let field = schema
            .get_field(name)
            .map_err(|e| SearchError::Internal(e.to_string()))?;
        default_fields.push(field);
    }
    let parser = QueryParser::for_index(index, default_fields);
    parser
        .parse_query(query)
        .map_err(|e| SearchError::Internal(e.to_string()))
}

/// `_build_cjk_query`.
fn build_cjk_query(index: &Index, raw_query: &str, fields: &[&str]) -> Option<Box<dyn Query>> {
    let cjk_text = extract_cjk_text(raw_query);
    if cjk_text.is_empty() {
        return None;
    }
    match tantivy_parse_query(index, &cjk_text, fields) {
        Ok(q) => Some(q),
        Err(_) => {
            tracing::debug!("Skipping CJK search clause: could not parse CJK text: {cjk_text:?}");
            None
        }
    }
}

/// `_any_of`: none -> empty, one -> itself, many -> boolean_query.
pub fn any_of(mut clauses: Vec<(Occur, Box<dyn Query>)>) -> Box<dyn Query> {
    match clauses.len() {
        0 => Box::new(EmptyQuery),
        1 => match clauses.pop() {
            Some((_, q)) => q,
            None => Box::new(EmptyQuery),
        },
        _ => Box::new(BooleanQuery::new(clauses)),
    }
}

/// Python `regex.escape(s)` (mrab `regex`, special_only=True).
pub fn py_regex_escape(s: &str) -> String {
    const META: &str = "()[]{}?*+|^$\\.-#&~";
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if META.contains(c) || c.is_whitespace() {
            out.push('\\');
            out.push(c);
        } else if c == '\0' {
            out.push_str("\\000");
        } else {
            out.push(c);
        }
    }
    out
}

/// Python `str.isdecimal()` (approximation, see contract "Known differences").
fn is_decimal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_numeric())
}

fn simple_field_boost(field: &str) -> f32 {
    if field == "simple_title" { 2.0 } else { 1.0 }
}

/// `_build_simple_token_query`.
fn build_simple_token_query(
    index: &Index,
    fields: &[&str],
    token: &str,
    allow_infix: bool,
) -> Result<Box<dyn Query>, SearchError> {
    let escaped = py_regex_escape(token);
    let pattern = if allow_infix {
        format!(".*{escaped}.*")
    } else {
        format!(r"({escaped}.*|.*[\x20-\x2f\x3a-\x40\x5b-\x60\x7b-\x7e]{escaped}.*)")
    };
    let schema = index.schema();
    let mut field_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for name in fields {
        let field = schema
            .get_field(name)
            .map_err(|e| SearchError::Internal(e.to_string()))?;
        let mut q: Box<dyn Query> = Box::new(
            RegexQuery::from_pattern(&pattern, field)
                .map_err(|e| SearchError::Internal(e.to_string()))?,
        );
        let boost = simple_field_boost(name);
        if boost > 1.0 {
            q = Box::new(BoostQuery::new(q, boost));
        }
        field_queries.push((Occur::Should, q));
    }
    Ok(any_of(field_queries))
}

/// `parse_simple_query`.
pub fn parse_simple_query(
    index: &Index,
    raw_query: &str,
    fields: &[&str],
    cjk_fields: Option<&[&str]>,
) -> Result<Box<dyn Query>, SearchError> {
    let tokens = simple_search_tokens(raw_query);
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    if !tokens.is_empty() {
        let single = tokens.len() == 1;
        let mut token_queries = Vec::with_capacity(tokens.len());
        for token in &tokens {
            let allow_infix = single || !is_decimal(token);
            token_queries.push((
                Occur::Must,
                build_simple_token_query(index, fields, token, allow_infix)?,
            ));
        }
        clauses.push((Occur::Should, any_of(token_queries)));
    }
    if let Some(cjk_fields) = cjk_fields.filter(|f| !f.is_empty())
        && has_cjk(raw_query)
        && let Some(q) = build_cjk_query(index, raw_query, cjk_fields)
    {
        clauses.push((Occur::Should, q));
    }
    Ok(any_of(clauses))
}

/// `parse_simple_text_highlight_query`. Errors map to upstream's `ValueError`
/// (caller falls back to the user query).
pub fn parse_simple_text_highlight_query(
    index: &Index,
    raw_query: &str,
) -> Result<Box<dyn Query>, SearchError> {
    let stripped: String = raw_query
        .chars()
        .map(|c| if c == '-' || c == '+' { ' ' } else { c })
        .collect();
    let tokens = simple_search_tokens(&stripped);
    if tokens.is_empty() {
        return Ok(Box::new(EmptyQuery));
    }
    let quoted: Vec<String> = tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    tantivy_parse_query(index, &quoted.join(" "), &["content"])
}

/// `parse_simple_text_query`.
pub fn parse_simple_text_query(index: &Index, raw_query: &str) -> Result<Box<dyn Query>, SearchError> {
    parse_simple_query(index, raw_query, SIMPLE_SEARCH_FIELDS, Some(CJK_CONTENT_FIELDS))
}

/// `parse_simple_title_query`.
pub fn parse_simple_title_query(index: &Index, raw_query: &str) -> Result<Box<dyn Query>, SearchError> {
    parse_simple_query(index, raw_query, TITLE_SEARCH_FIELDS, Some(CJK_TITLE_FIELDS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_extraction() {
        assert!(has_cjk("invoice 東京"));
        assert!(!has_cjk("invoice"));
        assert_eq!(extract_cjk_text("a東京b大阪"), "東京 大阪");
        assert_eq!(extract_cjk_text("コーヒー"), "コーヒー");
        assert_eq!(extract_cjk_text("latin"), "");
    }

    #[test]
    fn escape_like_mrab_regex() {
        assert_eq!(py_regex_escape("a.b-c"), r"a\.b\-c");
        assert_eq!(py_regex_escape("x#&~"), r"x\#\&\~");
        assert_eq!(py_regex_escape("é_1"), "é_1");
    }

    #[test]
    fn nfc() {
        assert_eq!(normalize_search_text("e\u{301}"), "é");
    }
}
