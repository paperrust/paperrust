// whoosh_compat/parser/default.py (whoosh-compat 0.3.0)
//
// QueryParser/MultifieldParser with the default plugin set paperless uses
// (Whitespace, SingleQuote, Fields, Wildcard, Phrase, Range, Group,
// Operators, Boost, Every, FieldAlias, CommaValues, Multifield,
// DateParser). The plugin list is fixed, so `filterize()` is the
// priority-sorted filter sequence spelled out in `process()`.

pub mod dateparse;
pub mod plugins;
pub mod syntax;
pub mod times;

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use regex::Regex;

use self::dateparse::DateParserPlugin;
use self::plugins::{folds_to_prefix, py_space};
use self::syntax::{GroupKind, SNode, TextKind};
use super::ast::{Node, TermText};
use super::errors::{Diagnostic, DiagnosticKind};
use super::fields::{FieldKind, FieldRef, FieldRegistry};

fn single_char_bracket_range() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[0-9A-Za-z]-[0-9A-Za-z]\]").ok()).as_ref()
}

/// Python `int(text)` restricted to what can matter for a u64: surrounding
/// whitespace, an optional sign, ASCII digits with single underscores
/// between them. (Non-ASCII Unicode digits are not accepted here.)
pub fn py_int(text: &str) -> Option<i128> {
    let t = text.trim_matches(py_space);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)?;
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    Some(if neg { -v } else { v })
}

/// QueryParser (MultifieldParser) state for one parse() call.
pub struct QueryParser<'r> {
    pub registry: &'r FieldRegistry,
    pub fieldnames: Vec<String>,
    pub fieldboosts: Vec<(String, f64)>,
    pub dates: Option<DateParserPlugin>,
    pub diagnostics: Vec<Diagnostic>,
    alias_reverse: Vec<(String, String)>,
    next_uid: u32,
}

impl<'r> QueryParser<'r> {
    pub fn new(
        registry: &'r FieldRegistry,
        fieldnames: Vec<String>,
        fieldboosts: Vec<(String, f64)>,
        tz: Tz,
        basedate: DateTime<Utc>,
    ) -> Self {
        let has_dates = registry.specs().iter().any(|s| matches!(s.kind, FieldKind::Date | FieldKind::Datetime));
        let mut alias_reverse = Vec::new();
        for s in registry.specs() {
            for a in &s.aliases {
                alias_reverse.push((a.clone(), s.name.clone()));
            }
        }
        QueryParser {
            registry,
            fieldnames,
            fieldboosts,
            dates: has_dates.then(|| DateParserPlugin::new(basedate, tz)),
            diagnostics: Vec::new(),
            alias_reverse,
            next_uid: 1,
        }
    }

    fn uid(&mut self) -> u32 {
        let u = self.next_uid;
        self.next_uid = self.next_uid.wrapping_add(1).max(1);
        u
    }

    /// `process()`: tag(), then the filters in priority order.
    pub fn process(&self, text: &str) -> SNode {
        let reg = self.registry;
        let flat = plugins::tag(text);
        // 0: GroupPlugin.do_groups, BoostPlugin.clean_boost
        let mut g = plugins::do_groups(flat);
        plugins::clean_boost(&mut g);
        // 50: WildcardPlugin.do_wildcards
        plugins::do_wildcards(&mut g);
        // 90: FieldAliasPlugin.do_aliases
        if !self.alias_reverse.is_empty() {
            plugins::do_aliases(&mut g, &self.alias_reverse);
        }
        // 100: FieldsPlugin.do_fieldnames
        g = plugins::do_fieldnames(g, reg);
        // 101/102: DateParserPlugin.do_date_phrases / do_unquoted_date_values
        if let Some(d) = &self.dates {
            d.do_date_phrases(&mut g, reg);
            d.do_unquoted_date_values(&mut g, reg);
        }
        // 105: CommaValuesPlugin.do_comma_values
        g = plugins::do_comma_values(g, reg);
        // 110: MultifieldPlugin.do_multifield, DateParserPlugin.do_dates
        plugins::do_multifield(&mut g, &self.fieldnames, &self.fieldboosts);
        if let Some(d) = &self.dates {
            d.do_dates(&mut g, reg);
        }
        // 500: WhitespacePlugin.remove_whitespace (x2, idempotent)
        g = plugins::remove_whitespace(g);
        // 510: BoostPlugin.do_boost
        g = plugins::do_boost(g);
        // 600: OperatorsPlugin.do_operators
        plugins::do_operators(&mut g);
        g
    }

    /// `parse()`
    pub fn parse(&mut self, text: &str) -> Node {
        self.diagnostics.clear();
        let g = self.process(text);
        self.query(&g).unwrap_or(Node::Nothing)
    }

    fn report(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    /// `field_ref`
    fn field_ref(&self, fieldname: Option<&str>) -> Option<FieldRef> {
        let f = fieldname?;
        Some(self.registry.make_ref(f).unwrap_or_else(|| FieldRef::plain(f)))
    }

    fn boosted(node: Node, boost: f64) -> Node {
        if boost != 1.0 { Node::Boosted(Box::new(node), boost) } else { node }
    }

    /// SyntaxNode.query(parser) for every node type.
    fn query(&mut self, n: &SNode) -> Option<Node> {
        match n {
            SNode::Group { kind, nodes, boost } => match kind {
                GroupKind::And | GroupKind::Or => {
                    let subs: Vec<Node> = nodes.iter().filter_map(|c| self.query(c)).collect();
                    if subs.is_empty() {
                        return None;
                    }
                    let q = if *kind == GroupKind::And { Node::And(subs) } else { Node::Or(subs) };
                    Some(Self::boosted(q, *boost))
                }
                GroupKind::Not => {
                    let first = nodes.first()?;
                    let q = self.query(first)?;
                    Some(Node::Not(Box::new(q)))
                }
                GroupKind::AndNot | GroupKind::AndMaybe | GroupKind::Require => {
                    let qa = nodes.first().and_then(|c| self.query(c));
                    let qb = nodes.get(1).and_then(|c| self.query(c));
                    if qa.is_none() && qb.is_none() {
                        return None;
                    }
                    let a = Box::new(qa.unwrap_or(Node::Nothing));
                    let b = Box::new(qb.unwrap_or(Node::Nothing));
                    Some(match kind {
                        GroupKind::AndNot => Node::AndNot(a, b),
                        GroupKind::AndMaybe => Node::AndMaybe(a, b),
                        _ => Node::Require(a, b),
                    })
                }
            },
            SNode::ErrorNode { message, sc, ec } => {
                let mut d = Diagnostic::new(DiagnosticKind::TooDeep, message.clone());
                d.startchar = *sc;
                d.endchar = *ec;
                self.report(d.clone());
                Some(Node::ErrorLeaf(Box::new(d)))
            }
            SNode::DateError { diag, .. } => {
                self.report((**diag).clone());
                Some(Node::ErrorLeaf(diag.clone()))
            }
            SNode::DateRange { fieldname, lo, hi, incl_lo, incl_hi, boost, .. } => Some(Self::boosted(
                Node::DateRange { field: FieldRef::plain(fieldname.clone()), lo: *lo, hi: *hi, incl_lo: *incl_lo, incl_hi: *incl_hi },
                *boost,
            )),
            SNode::Every { .. } => Some(Node::Every(None)),
            SNode::Range { start, end, startexcl, endexcl, fieldname, sc, ec } => {
                Some(self.range_query(fieldname.as_deref(), start.clone(), end.clone(), *startexcl, *endexcl, *sc, *ec))
            }
            SNode::Text { kind, text, fieldname, boost, sc, ec } => {
                let f = fieldname.as_deref();
                Some(match kind {
                    TextKind::Word | TextKind::QuotedWord => self.term_query(f, text, *boost, *sc, *ec),
                    TextKind::Wildcard => self.wildcard_query(f, text, *boost, *sc, *ec),
                    TextKind::Prefix => self.prefix_query(f, text, *boost, *sc, *ec),
                    TextKind::Phrase { slop } => self.phrase_query(f, text, *slop, *boost, *sc, *ec)?,
                })
            }
            // Markers never survive the filters (upstream: no query()).
            SNode::Whitespace { .. }
            | SNode::Fieldname { .. }
            | SNode::Operator { .. }
            | SNode::Boost { .. }
            | SNode::Open { .. }
            | SNode::Close { .. } => None,
        }
    }

    /// PhrasePlugin.PhraseNode.query
    fn phrase_query(
        &mut self,
        fieldname: Option<&str>,
        text: &str,
        slop: u32,
        boost: f64,
        sc: Option<usize>,
        ec: Option<usize>,
    ) -> Option<Node> {
        let r = self.field_ref(fieldname);
        let is_u64 = r.as_ref().and_then(|r| self.registry.resolve(r)).is_some_and(|rf| rf.spec.kind == FieldKind::U64);
        if is_u64 && text != "*" {
            if let Some(rr) = &r {
                if let Err(e) = self.parse_u64(text, rr, sc, ec) {
                    return Some(e);
                }
            }
        }
        let uid = self.uid();
        let q = Node::Phrase { field: r, text: text.to_string(), slop, words: None, analyzed: false, uid };
        Some(Self::boosted(q, boost))
    }

    /// `term_query`
    fn term_query(&mut self, fieldname: Option<&str>, text: &str, boost: f64, sc: Option<usize>, ec: Option<usize>) -> Node {
        let r = self.field_ref(fieldname);
        let kind = r.as_ref().and_then(|x| self.registry.resolve(x)).map(|rf| rf.spec.kind);
        if let (Some(k), Some(rr)) = (kind, &r) {
            if text == "*" && matches!(k, FieldKind::U64 | FieldKind::BooleanExists) {
                return Self::boosted(Node::Every(Some(rr.clone())), boost);
            }
            if k == FieldKind::U64 {
                return match self.parse_u64(text, rr, sc, ec) {
                    Err(e) => e,
                    Ok(v) => {
                        let uid = self.uid();
                        Self::boosted(Node::Term { field: r, text: TermText::Int(v), analyzed: false, uid }, boost)
                    }
                };
            }
            if k == FieldKind::BooleanExists {
                let s = text.trim_matches(py_space).to_lowercase();
                let truthy = !s.is_empty() && !matches!(s.as_str(), "f" | "false" | "no" | "0");
                let uid = self.uid();
                return Self::boosted(Node::Term { field: r, text: TermText::Bool(truthy), analyzed: false, uid }, boost);
            }
        }
        if let Some(e) = self.single_char_bracket_range_diagnostic(r.as_ref(), text, sc, ec, kind) {
            return e;
        }
        let uid = self.uid();
        Self::boosted(Node::term(r, text, uid), boost)
    }

    /// `_single_char_bracket_range_diagnostic`
    fn single_char_bracket_range_diagnostic(
        &mut self,
        r: Option<&FieldRef>,
        text: &str,
        sc: Option<usize>,
        ec: Option<usize>,
        field_kind: Option<FieldKind>,
    ) -> Option<Node> {
        if text.contains('*') || text.contains('?') {
            return None;
        }
        if !single_char_bracket_range()?.is_match(text) {
            return None;
        }
        let mut d = Diagnostic::new(
            DiagnosticKind::SingleCharBracketRange,
            format!(
                "{} looks like a bracket range but whoosh never treats '[' as a wildcard trigger on its own; combine it with a wildcard (e.g. a trailing '*') or double-quote the value to search it as literal text",
                crate::error::py_repr(text)
            ),
        );
        d.startchar = sc;
        d.endchar = ec;
        d.field = r.cloned();
        d.field_kind = field_kind;
        d.raw_value = Some(text.to_string());
        d.divergence = Some(60);
        self.report(d.clone());
        Some(Node::ErrorLeaf(Box::new(d)))
    }

    /// `_parse_u64`
    fn parse_u64(&mut self, text: &str, r: &FieldRef, sc: Option<usize>, ec: Option<usize>) -> Result<u64, Node> {
        match py_int(text).and_then(|v| u64::try_from(v).ok()) {
            Some(v) => Ok(v),
            None => {
                let mut d =
                    Diagnostic::new(DiagnosticKind::BadNumber, format!("{} is not a valid number for {r}", crate::error::py_repr(text)));
                d.startchar = sc;
                d.endchar = ec;
                d.field = Some(r.clone());
                d.field_kind = Some(FieldKind::U64);
                d.raw_value = Some(text.to_string());
                self.report(d.clone());
                Err(Node::ErrorLeaf(Box::new(d)))
            }
        }
    }

    /// `range_query`
    #[allow(clippy::too_many_arguments)]
    fn range_query(
        &mut self,
        fieldname: Option<&str>,
        start: Option<String>,
        end: Option<String>,
        startexcl: bool,
        endexcl: bool,
        sc: Option<usize>,
        ec: Option<usize>,
    ) -> Node {
        let r = self.field_ref(fieldname);
        let is_u64 = r.as_ref().and_then(|x| self.registry.resolve(x)).is_some_and(|rf| rf.spec.kind == FieldKind::U64);
        if let (true, Some(rr)) = (is_u64, &r) {
            let lo = match &start {
                None => None,
                Some(t) => match self.parse_u64(t, rr, sc, ec) {
                    Ok(v) => Some(v),
                    Err(e) => return e,
                },
            };
            let hi = match &end {
                None => None,
                Some(t) => match self.parse_u64(t, rr, sc, ec) {
                    Ok(v) => Some(v),
                    Err(e) => return e,
                },
            };
            return Node::NumericRange { field: rr.clone(), lo, hi, incl_lo: !startexcl, incl_hi: !endexcl };
        }
        Node::TermRange { field: r, lo: start, hi: end, incl_lo: !startexcl, incl_hi: !endexcl }
    }

    /// `_wildcard_kind_diagnostic`
    fn wildcard_kind_diagnostic(&mut self, r: Option<&FieldRef>, text: &str, sc: Option<usize>, ec: Option<usize>) -> Option<Node> {
        let rr = r?;
        let rf = self.registry.resolve(rr)?;
        let (kind, div, msg) = if rf.spec.kind == FieldKind::U64 {
            (DiagnosticKind::PatternOnNumeric, 29, format!("wildcard patterns are not supported on numeric field {rr}"))
        } else if rf.is_subpath() {
            (DiagnosticKind::PatternOnSubpath, 30, format!("wildcard patterns are not supported on a JSON subpath ({rr})"))
        } else if rf.spec.kind == FieldKind::BooleanExists {
            (DiagnosticKind::PatternOnBooleanExists, 29, format!("wildcard patterns are not supported on boolean-exists field {rr}"))
        } else {
            return None;
        };
        let mut d = Diagnostic::new(kind, msg);
        d.startchar = sc;
        d.endchar = ec;
        d.field = Some(rr.clone());
        d.field_kind = Some(rf.spec.kind);
        d.raw_value = Some(text.to_string());
        d.divergence = Some(div);
        self.report(d.clone());
        Some(Node::ErrorLeaf(Box::new(d)))
    }

    /// `wildcard_query`
    fn wildcard_query(&mut self, fieldname: Option<&str>, text: &str, boost: f64, sc: Option<usize>, ec: Option<usize>) -> Node {
        if !text.contains('*') && !text.contains('?') {
            return self.term_query(fieldname, text, boost, sc, ec);
        }
        let r = self.field_ref(fieldname);
        let node = if text == "*" {
            Node::Every(r)
        } else {
            if let Some(e) = self.wildcard_kind_diagnostic(r.as_ref(), text, sc, ec) {
                return e;
            }
            if folds_to_prefix(text, "?") {
                let mut t = text.to_string();
                t.pop();
                Node::Prefix { field: r, text: t }
            } else {
                Node::Wildcard { field: r, pattern: text.to_string() }
            }
        };
        Self::boosted(node, boost)
    }

    /// `prefix_query`
    fn prefix_query(&mut self, fieldname: Option<&str>, text: &str, boost: f64, sc: Option<usize>, ec: Option<usize>) -> Node {
        let r = self.field_ref(fieldname);
        if let Some(e) = self.wildcard_kind_diagnostic(r.as_ref(), &format!("{text}*"), sc, ec) {
            return e;
        }
        Self::boosted(Node::Prefix { field: r, text: text.to_string() }, boost)
    }
}
