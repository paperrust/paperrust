// whoosh_compat/emitters/tantivy_.py (whoosh-compat 0.3.0)
//
// AST -> programmatic tantivy query, mapping each tantivy-py 0.26 call to the
// tantivy 0.26 constructor it wraps:
//   Query.term_query        -> TermQuery::new(term, WithFreqsAndPositions)
//   Query.phrase_query      -> PhraseQuery::new_with_offset_and_slop
//   Query.regex_query       -> RegexQuery::from_pattern
//   Query.fuzzy_term_query  -> FuzzyTermQuery::new_prefix / new
//   Query.range_query       -> RangeQuery::new (dates: from_timestamp_secs,
//                              i.e. truncated to whole seconds like tantivy-py)
//   Query.exists_query      -> ExistsQuery::new
//   Query.boolean_query     -> BooleanQuery::from / with_minimum_required_clauses
//   Query.disjunction_max_query, boost_query, const_score_query,
//   empty_query, all_query  -> the same-named tantivy types
//   index.parse_query       -> QueryParser::for_index(..).parse_query
// The JSON-subpath carve-out takes the parse_query branch, because the
// tantivy-py 0.26 `term_query` probe (`schema.get_field("notes.note")`)
// always fails (`_json_paths_supported` is False there).
// Python ValueErrors from tantivy-py (missing field, type mismatch) are
// mapped exactly as `_reporting_schema_drift` + emit()'s backstop do:
// field missing from the schema -> SCHEMA_FIELD_MISSING, else
// BACKEND_REJECTED.

use std::ops::Bound;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, ConstScoreQuery, DisjunctionMaxQuery, EmptyQuery, ExistsQuery,
    FuzzyTermQuery, Occur, PhraseQuery, Query, QueryParser, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::{Field, IndexRecordOption, Schema, Type};
use tantivy::{Index, Term};

use super::ast::{analyze, Node, RewriteLeaf, TermText};
use super::errors::{Diagnostic, DiagnosticKind, QueryError};
use super::fields::{ExistsStrategy, FieldKind, FieldRef, FieldRegistry, Multitoken, PatternNormalizer, ResolvedField};

const FUZZY_MAX_DISTANCE: u8 = 2;

type Q = Box<dyn Query>;
type R<T> = Result<T, QueryError>;

/// Python 3.7+ `re.escape`.
pub fn py_re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\u{b}\u{c}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `_alternatives`: deduplicated, in order.
fn alternatives(normalize: &PatternNormalizer, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in normalize(text) {
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// `_alternation`
fn alternation(alts: &[String]) -> Option<String> {
    match alts.len() {
        0 => None,
        1 => Some(py_re_escape(&alts[0])),
        _ => Some(format!("(?:{})", alts.iter().map(|a| py_re_escape(a)).collect::<Vec<_>>().join("|"))),
    }
}

/// `_normalize_class_body`
fn normalize_class_body(body: &[char], normalize: Option<&PatternNormalizer>) -> Vec<char> {
    body.iter()
        .map(|ch| {
            let Some(n) = normalize else { return *ch };
            let folded = alternatives(n, &ch.to_string());
            if folded.len() == 1 {
                let mut cs = folded[0].chars();
                if let (Some(c), None) = (cs.next(), cs.next()) {
                    return c;
                }
            }
            *ch
        })
        .collect()
}

enum ClassResult {
    NoClose,
    Empty,
    Frag(String),
}

/// `_translate_class` (CPython fnmatch.translate bracket handling).
/// `Err` = the IndexError a pathological chunk merge raises upstream.
fn translate_class(p: &[char], i: usize, normalize: Option<&PatternNormalizer>, last_close: Option<usize>) -> Result<(ClassResult, usize), ()> {
    let n = p.len();
    let mut j = i;
    if j < n && p[j] == '!' {
        j += 1;
    }
    if j < n && p[j] == ']' {
        j += 1;
    }
    let close = match last_close {
        Some(lc) if j <= lc => p[j..].iter().position(|c| *c == ']').map(|k| j + k),
        _ => None,
    };
    let Some(close) = close else { return Ok((ClassResult::NoClose, i)) };
    let j = close;
    let mut stuff: Vec<char> = p[i..j].to_vec();
    let fold_from = usize::from(stuff.first() == Some(&'!'));
    let folded = normalize_class_body(&stuff[fold_from..], normalize);
    stuff.truncate(fold_from);
    stuff.extend(folded);
    let negated = stuff.first() == Some(&'!');
    let esc = |s: &[char]| -> String {
        let mut o = String::new();
        for c in s {
            match c {
                '\\' => o.push_str("\\\\"),
                '-' => o.push_str("\\-"),
                c => o.push(*c),
            }
        }
        o
    };
    let mut out: String;
    if !stuff.contains(&'-') {
        out = stuff.iter().map(|c| if *c == '\\' { "\\\\".to_string() } else { c.to_string() }).collect();
    } else {
        let mut chunks: Vec<Vec<char>> = Vec::new();
        let mut k = if negated { 2 } else { 1 };
        let mut start = 0;
        loop {
            let found = if k <= stuff.len() { stuff[k..].iter().position(|c| *c == '-').map(|x| k + x) } else { None };
            let Some(kk) = found else { break };
            chunks.push(stuff[start..kk].to_vec());
            start = kk + 1;
            k = kk + 3;
        }
        let chunk = stuff[start..].to_vec();
        if !chunk.is_empty() {
            chunks.push(chunk);
        } else if let Some(last) = chunks.last_mut() {
            last.push('-');
        }
        let mut k = chunks.len().saturating_sub(1);
        while k > 0 {
            let a = *chunks[k - 1].last().ok_or(())?;
            let b = *chunks[k].first().ok_or(())?;
            if a > b {
                let tail: Vec<char> = chunks[k][1..].to_vec();
                chunks[k - 1].pop();
                chunks[k - 1].extend(tail);
                chunks.remove(k);
            }
            k -= 1;
        }
        out = chunks.iter().map(|c| esc(c)).collect::<Vec<_>>().join("-");
    }
    if out.is_empty() {
        return Ok((ClassResult::Empty, j + 1));
    }
    if out == "!" {
        return Ok((ClassResult::Frag(".".to_string()), j + 1));
    }
    if let Some(rest) = out.strip_prefix('!') {
        out = format!("^{rest}");
    } else if out.starts_with('^') {
        out = format!("\\{out}");
    }
    for ch in ['[', '&', '~'] {
        out = out.replace(ch, &format!("\\{ch}"));
    }
    Ok((ClassResult::Frag(format!("[{out}]")), j + 1))
}

/// `glob_to_regex`; `Ok(None)` = provably matches nothing.
pub fn glob_to_regex(pattern: &str, normalizer: Option<&PatternNormalizer>) -> Result<Option<String>, ()> {
    let p: Vec<char> = pattern.chars().collect();
    let n = p.len();
    let mut out: Vec<String> = Vec::new();
    let mut literal = String::new();
    let mut impossible = false;
    let flush = |literal: &mut String, out: &mut Vec<String>, impossible: &mut bool| {
        if !literal.is_empty() {
            let alts = match normalizer {
                Some(nz) => alternatives(nz, literal),
                None => vec![literal.clone()],
            };
            literal.clear();
            match alternation(&alts) {
                None => *impossible = true,
                Some(f) => out.push(f),
            }
        }
    };
    let last_close = p.iter().rposition(|c| *c == ']');
    let mut i = 0;
    while i < n {
        let c = p[i];
        i += 1;
        if c == '*' {
            flush(&mut literal, &mut out, &mut impossible);
            while i < n && p[i] == '*' {
                i += 1;
            }
            out.push(".*".to_string());
        } else if c == '?' {
            flush(&mut literal, &mut out, &mut impossible);
            out.push(".".to_string());
        } else if c == '[' {
            let (res, ni) = translate_class(&p, i, normalizer, last_close)?;
            i = ni;
            match res {
                ClassResult::NoClose => literal.push(c),
                ClassResult::Empty => return Ok(None),
                ClassResult::Frag(f) => {
                    flush(&mut literal, &mut out, &mut impossible);
                    out.push(f);
                }
            }
        } else {
            literal.push(c);
        }
    }
    flush(&mut literal, &mut out, &mut impossible);
    if impossible {
        return Ok(None);
    }
    Ok(Some(out.concat()))
}

fn tantivy_date_min() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1677, 9, 21).and_then(|d| d.and_hms_opt(0, 12, 44)).unwrap_or_default()
}
fn tantivy_date_max() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2262, 4, 11).and_then(|d| d.and_hms_opt(23, 47, 16)).unwrap_or_default()
}

/// `_boolean_query` with `_pad_if_all_negative`
fn boolean_query(mut clauses: Vec<(Occur, Q)>, min_should: Option<usize>) -> Q {
    if !clauses.is_empty() && clauses.iter().all(|(o, _)| *o == Occur::MustNot) {
        clauses.insert(0, (Occur::Must, Box::new(BoostQuery::new(Box::new(AllQuery), 0.0))));
    }
    match min_should {
        Some(m) => Box::new(BooleanQuery::with_minimum_required_clauses(clauses, m)),
        None => Box::new(BooleanQuery::from(clauses)),
    }
}

/// TantivyEmitter
struct Emitter<'a> {
    index: &'a Index,
    schema: Schema,
    registry: &'a FieldRegistry,
}

impl Emitter<'_> {
    fn fail(&self, kind: DiagnosticKind, message: String, resolved: Option<&ResolvedField<'_>>, raw: Option<String>, div: Option<u32>) -> QueryError {
        let mut d = Diagnostic::new(kind, message);
        if let Some(r) = resolved {
            d.field = Some(FieldRef { name: r.spec.name.clone(), json_path: r.json_path.clone() });
            d.field_kind = Some(r.spec.kind);
        }
        d.raw_value = raw;
        d.divergence = div;
        QueryError { diagnostic: Box::new(d) }
    }

    fn resolve(&self, field: Option<&FieldRef>) -> R<ResolvedField<'_>> {
        let Some(f) = field else {
            return Err(self.fail(DiagnosticKind::AstUnfieldedTerm, "cannot emit an unfielded term".into(), None, None, None));
        };
        self.registry.resolve(f).ok_or_else(|| {
            self.fail(
                DiagnosticKind::AstUnknownField,
                format!("unknown field {}", crate::error::py_repr(&f.to_string())),
                None,
                Some(f.to_string()),
                None,
            )
        })
    }

    fn field_in_schema(&self, name: &str) -> bool {
        self.schema.get_field(name).is_ok()
    }

    /// A tantivy-py ValueError through `_reporting_schema_drift`.
    fn value_error(&self, resolved: &ResolvedField<'_>, msg: String) -> QueryError {
        if !self.field_in_schema(&resolved.spec.name) {
            return self.fail(DiagnosticKind::SchemaFieldMissing, format!("cannot emit query: {msg}"), Some(resolved), None, None);
        }
        self.fail(DiagnosticKind::BackendRejected, format!("cannot emit query: {msg}"), None, None, None)
    }

    /// `get_field` + type check of tantivy-py's `make_term`.
    fn field(&self, resolved: &ResolvedField<'_>, expect: Option<Type>) -> R<Field> {
        let name = &resolved.spec.name;
        let f = self
            .schema
            .get_field(name)
            .map_err(|_| self.value_error(resolved, format!("Field `{name}` is not defined in the schema.")))?;
        if let Some(t) = expect {
            let actual = self.schema.get_field_entry(f).field_type().value_type();
            if actual != t {
                return Err(self.value_error(resolved, format!("Field type mismatch for `{name}`")));
            }
        }
        Ok(f)
    }

    fn text_term(&self, resolved: &ResolvedField<'_>, text: &str) -> R<Term> {
        Ok(Term::from_field_text(self.field(resolved, Some(Type::Str))?, text))
    }

    fn emit(&self, node: &Node) -> R<Q> {
        match node {
            Node::Nothing => Ok(Box::new(EmptyQuery)),
            Node::Every(None) => Ok(Box::new(AllQuery)),
            Node::Every(Some(f)) => {
                let resolved = self.resolve(Some(f))?;
                if resolved.spec.kind == FieldKind::BooleanExists {
                    let t = FieldRef::plain(resolved.spec.exists_target.clone().unwrap_or_default());
                    let target = self.resolve(Some(&t))?;
                    return self.exists_query(&target);
                }
                self.exists_query(&resolved)
            }
            Node::ErrorLeaf(d) => Err(QueryError { diagnostic: d.clone() }),
            Node::Term { .. } => self.visit_term(node),
            Node::Phrase { .. } => self.visit_phrase(node),
            Node::Prefix { field, text } => {
                let resolved = self.resolve(field.as_ref())?;
                self.reject_pattern_kind(&resolved)?;
                let fragment = match &resolved.spec.pattern_normalizer {
                    None => Some(py_re_escape(text)),
                    Some(n) => alternation(&alternatives(n, text)),
                };
                match fragment {
                    None => Ok(Box::new(EmptyQuery)),
                    Some(f) => self.regex_query(&resolved, &format!("{f}.*")),
                }
            }
            Node::Wildcard { field, pattern } => {
                let resolved = self.resolve(field.as_ref())?;
                self.reject_pattern_kind(&resolved)?;
                let regex = glob_to_regex(pattern, resolved.spec.pattern_normalizer.as_ref()).map_err(|()| {
                    self.fail(DiagnosticKind::AstInvalidShape, "cannot emit query: string index out of range".into(), None, None, None)
                })?;
                match regex {
                    None => Ok(Box::new(EmptyQuery)),
                    Some(r) => self.regex_query(&resolved, &r),
                }
            }
            Node::Fuzzy { field, text, distance, prefix } => {
                let resolved = self.resolve(Some(field))?;
                if resolved.is_subpath() || !matches!(resolved.spec.kind, FieldKind::Text | FieldKind::Keyword) {
                    return Err(self.fail(
                        DiagnosticKind::AstKindNotImplemented,
                        format!("fuzzy emission for field kind {} is not implemented", resolved.spec.kind.name()),
                        Some(&resolved),
                        None,
                        None,
                    ));
                }
                if *distance > FUZZY_MAX_DISTANCE {
                    return Err(self.fail(
                        DiagnosticKind::AstBadNumber,
                        format!("fuzzy distance must be an integer between 0 and {FUZZY_MAX_DISTANCE}, got {distance}"),
                        Some(&resolved),
                        Some(distance.to_string()),
                        None,
                    ));
                }
                let forms = match &resolved.spec.pattern_normalizer {
                    Some(n) => alternatives(n, text),
                    None => vec![text.clone()],
                };
                let forms: Vec<String> = forms.into_iter().filter(|f| !f.trim().is_empty()).collect();
                if forms.is_empty() {
                    return Ok(Box::new(EmptyQuery));
                }
                let mut queries: Vec<Q> = Vec::with_capacity(forms.len());
                for form in &forms {
                    let term = self.text_term(&resolved, form)?;
                    queries.push(if *prefix {
                        Box::new(FuzzyTermQuery::new_prefix(term, *distance, true))
                    } else {
                        Box::new(FuzzyTermQuery::new(term, *distance, true))
                    });
                }
                if queries.len() == 1 {
                    return Ok(queries.remove(0));
                }
                Ok(Box::new(DisjunctionMaxQuery::new(queries)))
            }
            Node::TermRange { field, .. } => {
                let resolved = self.resolve(field.as_ref())?;
                let div = if resolved.is_subpath() {
                    Some(30)
                } else if matches!(resolved.spec.kind, FieldKind::Text | FieldKind::Keyword) {
                    Some(5)
                } else {
                    None
                };
                Err(self.fail(DiagnosticKind::TextRange, "text ranges are not supported".into(), Some(&resolved), None, div))
            }
            Node::NumericRange { field, lo, hi, incl_lo, incl_hi } => {
                let resolved = self.resolve(Some(field))?;
                if lo.is_none() && hi.is_none() {
                    return self.exists_query(&resolved);
                }
                let f = self.field(&resolved, Some(Type::U64))?;
                let b = |v: Option<u64>, incl: bool| match v {
                    None => Bound::Unbounded,
                    Some(v) if incl => Bound::Included(Term::from_field_u64(f, v)),
                    Some(v) => Bound::Excluded(Term::from_field_u64(f, v)),
                };
                Ok(Box::new(RangeQuery::new(b(*lo, *incl_lo), b(*hi, *incl_hi))))
            }
            Node::DateRange { field, lo, hi, incl_lo, incl_hi } => self.visit_daterange(field, *lo, *hi, *incl_lo, *incl_hi),
            Node::And(children) => {
                let mut qs = children.iter().map(|c| self.emit(c)).collect::<R<Vec<Q>>>()?;
                match qs.len() {
                    0 => Ok(Box::new(EmptyQuery)),
                    1 => Ok(qs.remove(0)),
                    _ => Ok(boolean_query(qs.into_iter().map(|q| (Occur::Must, q)).collect(), None)),
                }
            }
            Node::Or(children) => {
                let mut qs = children.iter().map(|c| self.emit(c)).collect::<R<Vec<Q>>>()?;
                match qs.len() {
                    0 => Ok(Box::new(EmptyQuery)),
                    1 => Ok(qs.remove(0)),
                    _ => Ok(boolean_query(qs.into_iter().map(|q| (Occur::Should, q)).collect(), Some(1))),
                }
            }
            Node::Not(c) => Ok(boolean_query(vec![(Occur::MustNot, self.emit(c)?)], None)),
            Node::AndNot(p, n) => {
                let p = self.emit(p)?;
                let n = self.emit(n)?;
                Ok(boolean_query(vec![(Occur::Must, p), (Occur::MustNot, n)], None))
            }
            Node::AndMaybe(r, o) => {
                let r = self.emit(r)?;
                let o = self.emit(o)?;
                Ok(boolean_query(vec![(Occur::Must, r), (Occur::Should, o)], None))
            }
            Node::Require(s, f) => {
                let s = self.emit(s)?;
                let f = self.emit(f)?;
                Ok(boolean_query(vec![(Occur::Must, s), (Occur::Must, Box::new(ConstScoreQuery::new(f, 0.0)))], None))
            }
            Node::Boosted(c, b) => Ok(Box::new(BoostQuery::new(self.emit(c)?, *b as f32))),
        }
    }

    /// `_exists_query`
    fn exists_query(&self, resolved: &ResolvedField<'_>) -> R<Q> {
        let spec = resolved.spec;
        let strategy = self.registry.exists_strategy(spec);
        if matches!(strategy, Some(ExistsStrategy::FastField | ExistsStrategy::FastJsonField)) && !self.field_in_schema(&spec.name) {
            return Err(self.fail(
                DiagnosticKind::SchemaFieldMissing,
                format!("field {} is not defined in the index schema", crate::error::py_repr(&spec.name)),
                Some(resolved),
                None,
                None,
            ));
        }
        match strategy {
            Some(ExistsStrategy::FastField) => Ok(Box::new(ExistsQuery::new(spec.name.clone(), false))),
            Some(ExistsStrategy::FastJsonField) => {
                if resolved.is_subpath() {
                    Ok(Box::new(ExistsQuery::new(resolved.dotted_name(), false)))
                } else {
                    Ok(Box::new(ExistsQuery::new(spec.name.clone(), true)))
                }
            }
            Some(ExistsStrategy::TermScan) => {
                let f = self.field(resolved, None)?;
                RegexQuery::from_pattern(".*", f)
                    .map(|q| Box::new(q) as Q)
                    .map_err(|e| self.value_error(resolved, e.to_string()))
            }
            None => Err(self.fail(
                DiagnosticKind::ExistsRequiresFast,
                format!(
                    "field {} ({}) has no way to match 'exists' while non-fast",
                    crate::error::py_repr(&resolved.dotted_name()),
                    spec.kind.name()
                ),
                Some(resolved),
                None,
                None,
            )),
        }
    }

    /// `_emit_json_term` / `_emit_json_phrase` parse_query fallback.
    fn json_parse_query(&self, resolved: &ResolvedField<'_>, text: &str) -> R<Q> {
        let f = self.field(resolved, None)?;
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let qs = format!("{}:\"{escaped}\"", resolved.dotted_name());
        QueryParser::for_index(self.index, vec![f])
            .parse_query(&qs)
            .map_err(|e| self.value_error(resolved, e.to_string()))
    }

    fn visit_term(&self, node: &Node) -> R<Q> {
        let Node::Term { field, text, .. } = node else { return Ok(Box::new(EmptyQuery)) };
        if let Some(f) = field {
            if f.json_path.is_some() {
                if let Some(resolved) = self.registry.resolve(f) {
                    return self.json_parse_query(&resolved, &text.py_str());
                }
            }
        }
        let resolved = self.resolve(field.as_ref())?;
        let spec = resolved.spec;
        match spec.kind {
            FieldKind::Text | FieldKind::Keyword => {
                let term = self.text_term(&resolved, &text.py_str())?;
                Ok(Box::new(TermQuery::new(term, IndexRecordOption::WithFreqsAndPositions)))
            }
            FieldKind::U64 => {
                let v = match text {
                    TermText::Int(v) => Some(*v),
                    TermText::Str(s) => super::parser::py_int(s).and_then(|v| u64::try_from(v).ok()),
                    TermText::Bool(b) => Some(u64::from(*b)),
                };
                let Some(v) = v else {
                    return Err(self.fail(
                        DiagnosticKind::AstBadNumber,
                        format!("{} is not a valid number for {}", crate::error::py_repr(&text.py_str()), crate::error::py_repr(&spec.name)),
                        Some(&resolved),
                        Some(text.py_str()),
                        None,
                    ));
                };
                let f = self.field(&resolved, Some(Type::U64))?;
                Ok(Box::new(TermQuery::new(Term::from_field_u64(f, v), IndexRecordOption::WithFreqsAndPositions)))
            }
            FieldKind::BooleanExists => {
                let t = FieldRef::plain(spec.exists_target.clone().unwrap_or_default());
                let target = self.resolve(Some(&t))?;
                let exists = self.exists_query(&target)?;
                let truthy = match text {
                    TermText::Bool(b) => *b,
                    TermText::Int(i) => *i != 0,
                    TermText::Str(s) => {
                        let s = s.trim().to_lowercase();
                        !s.is_empty() && !matches!(s.as_str(), "f" | "false" | "no" | "0")
                    }
                };
                if truthy {
                    return Ok(exists);
                }
                Ok(boolean_query(vec![(Occur::MustNot, exists)], None))
            }
            FieldKind::Json => Err(self.fail(
                DiagnosticKind::AstJsonNeedsSubpath,
                format!("field {} is a JSON field; term queries must address a subpath", crate::error::py_repr(&spec.name)),
                Some(&resolved),
                None,
                None,
            )),
            _ => Err(self.fail(
                DiagnosticKind::AstKindNotImplemented,
                format!("term emission for field kind {} is not implemented", spec.kind.name()),
                Some(&resolved),
                None,
                None,
            )),
        }
    }

    fn visit_phrase(&self, node: &Node) -> R<Q> {
        let Node::Phrase { field, text, slop, words, .. } = node else { return Ok(Box::new(EmptyQuery)) };
        let words: Vec<String> = words.clone().unwrap_or_else(|| vec![text.clone()]);
        if let Some(f) = field {
            if f.json_path.is_some() {
                if let Some(resolved) = self.registry.resolve(f) {
                    return self.json_parse_query(&resolved, &words.join(" "));
                }
            }
        }
        let resolved = self.resolve(field.as_ref())?;
        let spec = resolved.spec;
        match spec.kind {
            FieldKind::Text | FieldKind::Keyword => {
                if words.len() == 1 {
                    let term = self.text_term(&resolved, &words[0])?;
                    return Ok(Box::new(TermQuery::new(term, IndexRecordOption::WithFreqsAndPositions)));
                }
                let mapped = slop.saturating_sub(1);
                let mut terms = Vec::with_capacity(words.len());
                for (i, w) in words.iter().enumerate() {
                    terms.push((i, self.text_term(&resolved, w)?));
                }
                Ok(Box::new(PhraseQuery::new_with_offset_and_slop(terms, mapped)))
            }
            FieldKind::U64 => {
                if text == "*" {
                    return self.exists_query(&resolved);
                }
                let v = super::parser::py_int(text).and_then(|v| u64::try_from(v).ok());
                let Some(v) = v else {
                    return Err(self.fail(
                        DiagnosticKind::AstBadNumber,
                        format!("{} is not a valid number for {}", crate::error::py_repr(text), crate::error::py_repr(&spec.name)),
                        Some(&resolved),
                        Some(text.clone()),
                        None,
                    ));
                };
                let f = self.field(&resolved, Some(Type::U64))?;
                Ok(Box::new(TermQuery::new(Term::from_field_u64(f, v), IndexRecordOption::WithFreqsAndPositions)))
            }
            FieldKind::BooleanExists => {
                let t = FieldRef::plain(spec.exists_target.clone().unwrap_or_default());
                let target = self.resolve(Some(&t))?;
                let exists = self.exists_query(&target)?;
                let s = text.trim().to_lowercase();
                if text == "*" || (!s.is_empty() && !matches!(s.as_str(), "f" | "false" | "no" | "0")) {
                    return Ok(exists);
                }
                Ok(boolean_query(vec![(Occur::MustNot, exists)], None))
            }
            FieldKind::Json => Err(self.fail(
                DiagnosticKind::AstJsonNeedsSubpath,
                format!("field {} is a JSON field; phrase queries must address a subpath", crate::error::py_repr(&spec.name)),
                Some(&resolved),
                None,
                None,
            )),
            _ => Err(self.fail(
                DiagnosticKind::AstKindNotImplemented,
                format!("phrase emission for field kind {} is not implemented", spec.kind.name()),
                Some(&resolved),
                None,
                None,
            )),
        }
    }

    /// `_regex_query`
    fn regex_query(&self, resolved: &ResolvedField<'_>, regex: &str) -> R<Q> {
        let f = self.field(resolved, None)?;
        RegexQuery::from_pattern(regex, f).map(|q| Box::new(q) as Q).map_err(|e| {
            self.fail(
                DiagnosticKind::PatternTooComplex,
                format!("pattern is too complex for the backend to compile: {e}"),
                Some(resolved),
                None,
                None,
            )
        })
    }

    /// `_reject_pattern_incompatible_kind`
    fn reject_pattern_kind(&self, resolved: &ResolvedField<'_>) -> R<()> {
        let spec = resolved.spec;
        if resolved.is_subpath() {
            return Err(self.fail(
                DiagnosticKind::AstPatternOnKind,
                format!("wildcard/prefix patterns are not supported on JSON subpath {}", crate::error::py_repr(&resolved.dotted_name())),
                Some(resolved),
                None,
                Some(30),
            ));
        }
        if spec.kind == FieldKind::BooleanExists {
            return Err(self.fail(DiagnosticKind::AstPatternOnKind, "wildcard/prefix patterns are not supported on boolean-exists field".into(), Some(resolved), None, Some(29)));
        }
        if spec.kind == FieldKind::Json {
            return Err(self.fail(DiagnosticKind::AstJsonNeedsSubpath, "pattern queries must address a subpath".into(), Some(resolved), None, None));
        }
        if !matches!(spec.kind, FieldKind::Text | FieldKind::Keyword) {
            return Err(self.fail(
                DiagnosticKind::AstPatternOnKind,
                format!("pattern emission for field kind {} is not implemented", spec.kind.name()),
                Some(resolved),
                None,
                None,
            ));
        }
        Ok(())
    }

    /// `visit_daterange` (clamped into tantivy's i64-nanosecond window)
    fn visit_daterange(&self, field: &FieldRef, lo: Option<DateTime<Utc>>, hi: Option<DateTime<Utc>>, incl_lo: bool, incl_hi: bool) -> R<Q> {
        let resolved = self.resolve(Some(field))?;
        let mut lo = lo.map(|d| d.naive_utc());
        let mut hi = hi.map(|d| d.naive_utc());
        let (mut incl_lo, mut incl_hi) = (incl_lo, incl_hi);
        let (min, max) = (tantivy_date_min(), tantivy_date_max());
        if lo.is_some_and(|l| l > max) || hi.is_some_and(|h| h < min) {
            return Ok(Box::new(EmptyQuery));
        }
        if lo.is_some_and(|l| l < min) {
            lo = Some(min);
            incl_lo = true;
        }
        if hi.is_some_and(|h| h > max) {
            hi = Some(max);
            incl_hi = true;
        }
        if lo.is_none() && hi.is_none() {
            return self.exists_query(&resolved);
        }
        let f = self.field(&resolved, Some(Type::Date))?;
        let b = |v: Option<NaiveDateTime>, incl: bool| match v {
            None => Bound::Unbounded,
            Some(v) => {
                let t = Term::from_field_date(f, tantivy::DateTime::from_timestamp_secs(v.and_utc().timestamp()));
                if incl {
                    Bound::Included(t)
                } else {
                    Bound::Excluded(t)
                }
            }
        };
        Ok(Box::new(RangeQuery::new(b(lo, incl_lo), b(hi, incl_hi))))
    }
}

/// emitters/tantivy_.py::emit
pub fn emit(node: &Node, index: &Index, registry: &FieldRegistry, rewrite_leaf: Option<RewriteLeaf<'_>>) -> Result<Q, QueryError> {
    let analyzed = analyze(node, registry, Multitoken::And, rewrite_leaf);
    let e = Emitter { index, schema: index.schema(), registry };
    e.emit(&analyzed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn glob_translation_matches_fnmatch() {
        let fold: PatternNormalizer = Arc::new(|t: &str| vec![t.to_lowercase()]);
        let g = |p: &str| glob_to_regex(p, Some(&fold)).unwrap();
        assert_eq!(g("BILL[I]NG*"), Some("bill[i]ng.*".to_string()));
        assert_eq!(g("202[0-3]*"), Some("202[0-3].*".to_string()));
        assert_eq!(g("a**b?"), Some("a.*b.".to_string()));
        assert_eq!(g("[!a]x"), Some("[^a]x".to_string()));
        assert_eq!(g("[z-a]"), None);
        // fnmatch: "[!]" has no closing bracket (the "]" is a member), so
        // the "[" is literal.
        assert_eq!(g("[!]"), Some("\\[!\\]".to_string()));
        assert_eq!(g("[!]]x"), Some("[^]]x".to_string()));
        assert_eq!(g("a[b"), Some("a\\[b".to_string()));
        assert_eq!(g("in.v*"), Some("in\\.v.*".to_string()));
        let none: PatternNormalizer = Arc::new(|_t: &str| vec![]);
        assert_eq!(glob_to_regex("ab*", Some(&none)).unwrap(), None);
        let two: PatternNormalizer = Arc::new(|t: &str| vec![t.to_string(), format!("{t}x")]);
        assert_eq!(glob_to_regex("a*", Some(&two)).unwrap(), Some("(?:a|ax).*".to_string()));
    }
}
