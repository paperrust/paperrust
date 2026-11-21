//! Rust port of whoosh-compat 0.3.0 (the Whoosh query language -> AST ->
//! programmatic tantivy query library paperless-ngx v3.4.2 uses for its
//! query-mode full-text search).
//!
//! Module map (Python source in brackets):
//! * [`errors`]  - Cause, DiagnosticKind, Diagnostic, QueryError  [errors.py]
//! * [`fields`]  - FieldKind, FieldRef, FieldSpec, FieldRegistry  [fields.py]
//! * [`ast`]     - Node, normalize(), analyze()                   [ast.py]
//! * [`parser`]  - QueryParser + plugins + date grammar
//!   [parser/default.py, plugins.py, syntax.py, taggers.py, text.py,
//!   common.py, priorities.py, times.py, dateparse.py]
//! * [`emit`]    - the tantivy emitter                            [emitters/tantivy_.py]
//!
//! Skipped because paperless can never reach them:
//! * emitters/base.py (the abstract Emitter protocol) and any other backend;
//! * `free_text_tokens()`, `ast.Visitor`, `print_debug`/debug tagging,
//!   plugin add/remove APIs (the plugin set is fixed: the default set +
//!   FieldAliasPlugin + CommaValuesPlugin + MultifieldPlugin +
//!   DateParserPlugin, exactly what `whoosh_compat.parse` builds);
//! * FieldRegistry construction-time validation (paperless' fixed field
//!   table passes it upstream) and FieldSpec pickling;
//! * custom tagger expressions/subclasses (the skip memos are therefore
//!   always on, as upstream for the default expressions);
//! * the `_json_paths_supported` probe: on tantivy-py 0.26 it is always
//!   False, so JSON-subpath leaves always use the parse_query fallback.
//!
//! The whoosh grammar has no `field:>x`/`<x` comparison and no `term~`
//! fuzzy syntax (no GtLtPlugin/FuzzyTermPlugin in whoosh-compat's default
//! set); such text is ordinary term text, exactly as upstream.

// Nested `if`s are kept as in the Python source (1:1 port readability).
#![allow(clippy::collapsible_if)]

pub mod ast;
pub mod emit;
pub mod errors;
pub mod fields;
pub mod parser;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use self::ast::Node;
use self::errors::Diagnostic;
use self::fields::FieldRegistry;

/// whoosh_compat/__init__.py::ParseResult
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub ast: Node,
    pub diagnostics: Vec<Diagnostic>,
}

/// whoosh_compat/__init__.py::parse
///
/// `Err` is the ValueError upstream raises for host misconfiguration
/// (empty/unknown default field, unknown field_boosts key).
pub fn parse(
    query: &str,
    registry: &FieldRegistry,
    default_fields: &[&str],
    field_boosts: &[(&str, f64)],
    tz: Tz,
    basedate: DateTime<Utc>,
) -> Result<ParseResult, String> {
    if default_fields.is_empty() {
        return Err("default_fields must not be empty".to_string());
    }
    let mut canonical = Vec::with_capacity(default_fields.len());
    for name in default_fields {
        let r = registry.make_ref(name).ok_or_else(|| format!("default_fields names unknown field {}", crate::error::py_repr(name)))?;
        canonical.push(r.to_string());
    }
    let mut boosts = Vec::with_capacity(field_boosts.len());
    for (k, b) in field_boosts {
        let r = registry.make_ref(k).ok_or_else(|| format!("field_boosts key {} does not name a known field", crate::error::py_repr(k)))?;
        boosts.push((r.to_string(), *b));
    }
    let mut p = parser::QueryParser::new(registry, canonical, boosts, tz, basedate);
    let node = p.parse(query);
    let node = ast::normalize(&node);
    Ok(ParseResult { ast: node, diagnostics: std::mem::take(&mut p.diagnostics) })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::ast::{Node, TermText};
    use super::errors::DiagnosticKind;
    use super::fields::FieldRef;
    use super::*;

    fn p(q: &str) -> ParseResult {
        let reg = crate::query_user::get_field_registry(None);
        let now = Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        parse(q, &reg, &["title", "content"], &[("title", 2.0)], chrono_tz::UTC, now).unwrap()
    }

    fn term(f: &str, t: &str) -> Node {
        Node::term(Some(FieldRef::plain(f)), t, 0)
    }

    fn word(t: &str) -> Node {
        Node::Or(vec![Node::Boosted(Box::new(term("title", t)), 2.0), term("content", t)])
    }

    #[test]
    fn multifield_expansion_and_boosts() {
        let r = p("foo");
        assert!(r.diagnostics.is_empty());
        assert!(ast::same(&r.ast, &word("foo")));
        let r = p("title:foo bar");
        assert!(ast::same(&r.ast, &Node::And(vec![term("title", "foo"), word("bar")])));
        let r = p("type:x");
        assert!(ast::same(&r.ast, &term("document_type", "x")));
        let r = p("a OR b");
        let Node::Or(c) = &r.ast else { panic!("{:?}", r.ast) };
        assert_eq!(c.len(), 4);
        let r = p("NOT a");
        assert!(ast::same(&r.ast, &Node::Not(Box::new(word("a")))));
        let r = p("unknown:x");
        assert!(ast::same(&r.ast, &word("unknown:x")));
        let r = p("notes:hello");
        assert!(ast::same(&r.ast, &Node::term(Some(FieldRef::sub("notes", "note")), "hello", 0)));
        let r = p("asn:5");
        assert!(matches!(&r.ast, Node::Term { text: TermText::Int(5), .. }));
        let r = p("(a b)^3");
        assert!(matches!(&r.ast, Node::Boosted(_, b) if *b == 3.0));
    }

    #[test]
    fn patterns_and_ranges() {
        assert!(ast::same(&p("title:inv*").ast, &Node::Prefix { field: Some(FieldRef::plain("title")), text: "inv".into() }));
        assert!(ast::same(
            &p("title:202[0-3]*").ast,
            &Node::Wildcard { field: Some(FieldRef::plain("title")), pattern: "202[0-3]*".into() }
        ));
        assert!(ast::same(&p("asn:*").ast, &Node::Every(Some(FieldRef::plain("asn")))));
        let r = p("asn:{1 TO 5]");
        assert!(ast::same(
            &r.ast,
            &Node::NumericRange { field: FieldRef::plain("asn"), lo: Some(1), hi: Some(5), incl_lo: false, incl_hi: true }
        ));
        let r = p("created:[2005 to 2009]");
        let lo = Utc.with_ymd_and_hms(2005, 1, 1, 0, 0, 0).unwrap();
        let hi = Utc.with_ymd_and_hms(2010, 1, 1, 0, 0, 0).unwrap();
        assert!(ast::same(
            &r.ast,
            &Node::DateRange { field: FieldRef::plain("created"), lo: Some(lo), hi: Some(hi), incl_lo: true, incl_hi: false }
        ));
        let r = p("title:[a TO b]");
        assert!(matches!(r.ast, Node::TermRange { .. }));
    }

    #[test]
    fn diagnostics() {
        let r = p("created:202023");
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].kind, DiagnosticKind::BadDate);
        assert_eq!(r.diagnostics[0].raw_value.as_deref(), Some("202023"));
        let r = p("added:december 2019");
        assert_eq!(r.diagnostics[0].suggestion.as_deref(), Some("\"december 2019\""));
        let r = p("added:\"august 3pm\"");
        assert_eq!(r.diagnostics[0].kind, DiagnosticKind::BadDate);
        let r = p("asn:abc OR page_count:-1");
        assert_eq!(r.diagnostics.len(), 2);
        assert!(r.diagnostics.iter().all(|d| d.kind == DiagnosticKind::BadNumber));
        let chain = vec!["a"; 205].join(" ANDNOT ");
        assert_eq!(p(&chain).diagnostics[0].kind, DiagnosticKind::TooDeep);
    }
}
