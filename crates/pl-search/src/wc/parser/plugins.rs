// whoosh_compat/parser/plugins.py (whoosh-compat 0.3.0)
//
// Taggers and filters of the default plugin set. The tagger regular
// expressions use look-behind/look-ahead, which Rust's `regex` lacks, so the
// simple ones are hand-written matchers with identical acceptance and the
// rest run anchored on the text slice with the look-ahead folded into a
// consumed tail (the match end is taken before the tail). Each matcher names
// the Python expression it replaces.
//
// `FieldnameTagger`/`SingleQuotePlugin` keep upstream's linear-time skip
// memo (same results as plain matching).

use std::sync::OnceLock;

use regex::Regex;

use super::syntax::{GroupKind, OpKind, SNode, TextKind, replace_self, to_word};
use crate::wc::fields::FieldRegistry;

/// plugins.py::_MAX_GROUP_NESTING_DEPTH
pub const MAX_GROUP_NESTING_DEPTH: usize = 200;

/// Python `\s` (str.isspace): Unicode White_Space plus U+001C..U+001F.
pub fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `\w` (str.isalnum() or "_"), approximated by Rust's
/// `is_alphanumeric` (Alphabetic + Numeric).
pub fn py_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

fn prev_char(text: &str, pos: usize) -> Option<char> {
    text[..pos].chars().next_back()
}

fn next_char(text: &str, pos: usize) -> Option<char> {
    text[pos..].chars().next()
}

/// plugins.py::_too_deep_node
pub fn too_deep_node(node: Option<&SNode>) -> SNode {
    SNode::ErrorNode {
        message: format!("query nesting exceeds the maximum supported depth ({MAX_GROUP_NESTING_DEPTH})"),
        sc: node.and_then(SNode::startchar),
        ec: node.and_then(SNode::endchar),
    }
}

/// plugins.py::folds_to_prefix
pub fn folds_to_prefix(text: &str, qmarks: &str) -> bool {
    let n = text.chars().count();
    n > 1 && !text.contains('[') && !text.chars().any(|c| qmarks.contains(c)) && text.chars().position(|c| c == '*') == Some(n - 1)
}

/// WildcardPlugin.qmarks
pub const QMARKS: &str = "?\u{55e}\u{61f}\u{1367}";

// -- taggers --------------------------------------------------------------

/// The taggers in priority order (stable sort of `QueryParser.taggers()`
/// for the paperless plugin set): AndNot, AndMaybe (-5); Every (-1);
/// SingleQuote, Fields, Wildcard, Phrase, Open, Close, Not, And, Or,
/// Require, Boost (0); Range (1); Whitespace (100).
#[derive(Debug, Clone, Copy)]
enum Tagger {
    AndNot,
    AndMaybe,
    Every,
    SingleQuote,
    Fields,
    Wildcard,
    Phrase,
    Open,
    Close,
    Not,
    And,
    Or,
    Require,
    Boost,
    Range,
    Whitespace,
}

const TAGGERS: [Tagger; 16] = [
    Tagger::AndNot,
    Tagger::AndMaybe,
    Tagger::Every,
    Tagger::SingleQuote,
    Tagger::Fields,
    Tagger::Wildcard,
    Tagger::Phrase,
    Tagger::Open,
    Tagger::Close,
    Tagger::Not,
    Tagger::And,
    Tagger::Or,
    Tagger::Require,
    Tagger::Boost,
    Tagger::Range,
    Tagger::Whitespace,
];

fn range_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        // RangePlugin.expr; the start group's `(?=\b[Tt][Oo]\b)` look-ahead is
        // immediately followed by the same `\b[Tt][Oo]\b`, so it is dropped.
        Regex::new(r"\A(\{|\[)(?:('[^']*?'\s+)|([^\]}]+?))?\b[Tt][Oo]\b(?:(\s+'[^']*?')|([^\]}]+?))?(\}|\])").ok()
    })
    .as_ref()
}

fn boost_re() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    // BoostPlugin.expr: \^(?P<boost>[0-9]*(\.[0-9]+)?)($|(?=[ \t\r\n)]))
    RE.get_or_init(|| Regex::new(r"\A\^([0-9]*(?:\.[0-9]+)?)(?:\z|[ \t\r\n)])").ok()).as_ref()
}

/// Per-parse tagger state (the skip memos).
#[derive(Default)]
pub struct TagState {
    field_skip: Option<(usize, usize)>,
    quote_skip: Option<(usize, usize)>,
}

fn sp(s: usize, e: usize) -> (Option<usize>, Option<usize>) {
    (Some(s), Some(e))
}

/// `(?<=\s)WORD(?=\s)` style operator: `lead` decides the look-behind.
fn match_op(text: &str, pos: usize, word: &str, lead: fn(Option<char>) -> bool) -> Option<usize> {
    if !text[pos..].starts_with(word) || !lead(prev_char(text, pos)) {
        return None;
    }
    let end = pos + word.len();
    match next_char(text, end) {
        Some(c) if py_space(c) => Some(end),
        _ => None,
    }
}

fn lead_space(c: Option<char>) -> bool {
    c.is_some_and(py_space)
}
fn lead_start_or_space(c: Option<char>) -> bool {
    c.is_none_or(py_space)
}
fn lead_not(c: Option<char>) -> bool {
    c.is_none_or(|c| py_space(c) || c == '(' || c == ')')
}

fn tag_one(t: Tagger, st: &mut TagState, text: &str, pos: usize) -> Option<SNode> {
    let op = |op: OpKind, end: usize| {
        let (sc, ec) = sp(pos, end);
        SNode::Operator { op, sc, ec }
    };
    match t {
        Tagger::AndNot => match_op(text, pos, "ANDNOT", lead_space).map(|e| op(OpKind::AndNot, e)),
        Tagger::AndMaybe => match_op(text, pos, "ANDMAYBE", lead_space).map(|e| op(OpKind::AndMaybe, e)),
        Tagger::Not => match_op(text, pos, "NOT", lead_not).map(|e| op(OpKind::Not, e)),
        Tagger::And => match_op(text, pos, "AND", lead_space).map(|e| op(OpKind::And, e)),
        Tagger::Or => match_op(text, pos, "OR", lead_space).map(|e| op(OpKind::Or, e)),
        Tagger::Require => match_op(text, pos, "REQUIRE", lead_start_or_space).map(|e| op(OpKind::Require, e)),
        Tagger::Every => {
            // EveryPlugin.expr: [*]:[*]
            if text[pos..].starts_with("*:*") {
                let (sc, ec) = sp(pos, pos + 3);
                Some(SNode::Every { sc, ec })
            } else {
                None
            }
        }
        Tagger::SingleQuote => tag_single_quote(st, text, pos),
        Tagger::Fields => tag_fieldname(st, text, pos),
        Tagger::Wildcard => {
            // WildcardPlugin.expr: (?P<text>[*?՞؟፧])
            let c = next_char(text, pos)?;
            if c == '*' || QMARKS.contains(c) {
                let (sc, ec) = sp(pos, pos + c.len_utf8());
                Some(SNode::Text { kind: TextKind::Wildcard, text: c.to_string(), fieldname: None, boost: 1.0, sc, ec })
            } else {
                None
            }
        }
        Tagger::Phrase => tag_phrase(text, pos),
        Tagger::Open => {
            if text[pos..].starts_with('(') {
                let (sc, ec) = sp(pos, pos + 1);
                Some(SNode::Open { sc, ec })
            } else {
                None
            }
        }
        Tagger::Close => {
            if text[pos..].starts_with(')') {
                let (sc, ec) = sp(pos, pos + 1);
                Some(SNode::Close { sc, ec })
            } else {
                None
            }
        }
        Tagger::Boost => {
            // BoostPlugin.create
            let caps = boost_re()?.captures(&text[pos..])?;
            let g = caps.get(1)?;
            let end = pos + g.end();
            let original = text[pos..end].to_string();
            let (sc, ec) = sp(pos, end);
            match g.as_str().parse::<f64>() {
                Ok(b) if !g.as_str().is_empty() => Some(SNode::Boost { original, boost: b, sc, ec }),
                _ => Some(SNode::word(original, sc, ec)),
            }
        }
        Tagger::Range => tag_range(text, pos),
        Tagger::Whitespace => {
            // WhitespacePlugin: \s+
            let mut end = pos;
            for c in text[pos..].chars() {
                if !py_space(c) {
                    break;
                }
                end += c.len_utf8();
            }
            if end > pos {
                let (sc, ec) = sp(pos, end);
                Some(SNode::Whitespace { sc, ec })
            } else {
                None
            }
        }
    }
}

/// SingleQuotePlugin: (^|(?<=\W))'(?P<text>.*?)'(?=\s|\]|[)}]|$)
fn tag_single_quote(st: &mut TagState, text: &str, pos: usize) -> Option<SNode> {
    if let Some((s, e)) = st.quote_skip {
        if s <= pos && pos < e {
            return None;
        }
    }
    if !text[pos..].starts_with('\'') {
        return None;
    }
    if prev_char(text, pos).is_some_and(py_word) {
        return None;
    }
    let body_start = pos + 1;
    let mut i = body_start;
    for c in text[body_start..].chars() {
        if c == '\n' {
            break;
        }
        if c == '\'' {
            let after = i + 1;
            let ok = match next_char(text, after) {
                None => true,
                Some(n) => py_space(n) || n == ']' || n == ')' || n == '}',
            };
            if ok {
                let (sc, ec) = sp(pos, after);
                return Some(SNode::Text {
                    kind: TextKind::QuotedWord,
                    text: text[body_start..i].to_string(),
                    fieldname: None,
                    boost: 1.0,
                    sc,
                    ec,
                });
            }
        }
        i += c.len_utf8();
    }
    let nl = text[pos..].find('\n').map_or(text.len(), |k| pos + k);
    st.quote_skip = Some((pos, nl));
    None
}

/// FieldsPlugin.FieldnameTagger: (?P<text>[\w.]+|[*]):
fn tag_fieldname(st: &mut TagState, text: &str, pos: usize) -> Option<SNode> {
    if let Some((s, e)) = st.field_skip {
        if s <= pos && pos < e {
            return None;
        }
    }
    let mut end = pos;
    for c in text[pos..].chars() {
        if !(py_word(c) || c == '.') {
            break;
        }
        end += c.len_utf8();
    }
    if end > pos {
        if text[end..].starts_with(':') {
            let name = text[pos..end].to_string();
            let (sc, ec) = sp(pos, end + 1);
            return Some(SNode::Fieldname { fieldname: Some(name.clone()), original: format!("{name}:"), sc, ec });
        }
        st.field_skip = Some((pos, end));
        return None;
    }
    if text[pos..].starts_with("*:") {
        let (sc, ec) = sp(pos, pos + 2);
        return Some(SNode::Fieldname { fieldname: Some("*".to_string()), original: "*:".to_string(), sc, ec });
    }
    None
}

/// PhrasePlugin: "(?P<text>.*?)"(~(?P<slop>[1-9][0-9]*))?
fn tag_phrase(text: &str, pos: usize) -> Option<SNode> {
    if !text[pos..].starts_with('"') {
        return None;
    }
    let body_start = pos + 1;
    let mut close = None;
    let mut i = body_start;
    for c in text[body_start..].chars() {
        if c == '\n' {
            return None;
        }
        if c == '"' {
            close = Some(i);
            break;
        }
        i += c.len_utf8();
    }
    let close = close?;
    let body = text[body_start..close].to_string();
    let mut end = close + 1;
    let mut slop = 1u32;
    let rest = &text[end..];
    if let Some(r) = rest.strip_prefix('~') {
        if r.starts_with(|c: char| ('1'..='9').contains(&c)) {
            let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
            slop = digits.parse::<u64>().map_or(u32::MAX, |v| u32::try_from(v).unwrap_or(u32::MAX));
            end += 1 + digits.len();
        }
    }
    let (sc, ec) = sp(pos, end);
    Some(SNode::Text { kind: TextKind::Phrase { slop }, text: body, fieldname: None, boost: 1.0, sc, ec })
}

/// RangePlugin.RangeTagger
fn tag_range(text: &str, pos: usize) -> Option<SNode> {
    if !(text[pos..].starts_with('[') || text[pos..].starts_with('{')) {
        return None;
    }
    let caps = range_re()?.captures(&text[pos..])?;
    let whole = caps.get(0)?;
    let open = caps.get(1).map(|m| m.as_str());
    let close = caps.get(6).map(|m| m.as_str());
    let mut start = caps.get(2).or_else(|| caps.get(3)).map(|m| m.as_str().to_string());
    let mut end = caps.get(4).or_else(|| caps.get(5)).map(|m| m.as_str().to_string());
    if let Some(s) = start.as_mut().filter(|s| !s.is_empty()) {
        let t = s.trim_end_matches(py_space).to_string();
        *s = if t.starts_with('\'') && t.ends_with('\'') { strip_quotes(&t) } else { t };
    }
    if let Some(e) = end.as_mut().filter(|s| !s.is_empty()) {
        let t = e.trim_start_matches(py_space).to_string();
        *e = if t.starts_with('\'') && t.ends_with('\'') { strip_quotes(&t) } else { t };
    }
    let (sc, ec) = sp(pos, pos + whole.end());
    Some(SNode::Range { start, end, startexcl: open == Some("{"), endexcl: close == Some("}"), fieldname: None, sc, ec })
}

/// `s[1:-1]`
fn strip_quotes(s: &str) -> String {
    let mut chars = s.chars();
    chars.next();
    chars.next_back();
    chars.as_str().to_string()
}

/// default.py::QueryParser.tag
pub fn tag(text: &str) -> Vec<SNode> {
    let mut st = TagState::default();
    let mut stack: Vec<SNode> = Vec::new();
    let mut prev = 0usize;
    let mut pos = 0usize;
    let inter = |s: usize, e: usize| SNode::word(text[s..e].to_string(), Some(s), Some(e));
    while pos < text.len() {
        let mut matched = None;
        for t in TAGGERS {
            if let Some(node) = tag_one(t, &mut st, text, pos) {
                matched = Some(node);
                break;
            }
        }
        match matched {
            Some(node) => {
                let end = node.endchar().unwrap_or(pos).max(pos);
                if prev < pos {
                    stack.push(inter(prev, pos));
                }
                stack.push(node);
                if end <= pos {
                    // QueryParserError upstream ("did not move cursor");
                    // unreachable for these taggers, advance defensively.
                    pos += next_char(text, pos).map_or(1, char::len_utf8);
                } else {
                    pos = end;
                }
                prev = pos;
            }
            None => pos += next_char(text, pos).map_or(1, char::len_utf8),
        }
    }
    if prev < text.len() {
        stack.push(inter(prev, text.len()));
    }
    stack
}

// -- filters --------------------------------------------------------------

fn kids(g: &mut SNode) -> Option<&mut Vec<SNode>> {
    match g {
        SNode::Group { nodes, .. } => Some(nodes),
        _ => None,
    }
}

/// GroupPlugin.do_groups (parser.group is AndGroup)
pub fn do_groups(flat: Vec<SNode>) -> SNode {
    let mut stack: Vec<SNode> = vec![SNode::group(GroupKind::And, Vec::new())];
    let mut overflow_depth = 0usize;
    let mut overflow_start: Option<SNode> = None;
    for node in flat {
        match node {
            SNode::Open { .. } => {
                if overflow_depth > 0 || stack.len() >= MAX_GROUP_NESTING_DEPTH {
                    if overflow_depth == 0 {
                        overflow_start = Some(node);
                    }
                    overflow_depth += 1;
                    continue;
                }
                stack.push(SNode::group(GroupKind::And, Vec::new()));
            }
            SNode::Close { .. } => {
                if overflow_depth > 0 {
                    overflow_depth -= 1;
                    if overflow_depth == 0 {
                        let e = too_deep_node(overflow_start.as_ref());
                        if let Some(n) = stack.last_mut().and_then(kids) {
                            n.push(e);
                        }
                        overflow_start = None;
                    }
                    continue;
                }
                if stack.len() > 1 {
                    if let Some(last) = stack.pop() {
                        if let Some(n) = stack.last_mut().and_then(kids) {
                            n.push(last);
                        }
                    }
                }
            }
            other => {
                if overflow_depth > 0 {
                    continue;
                }
                if let Some(n) = stack.last_mut().and_then(kids) {
                    n.push(other);
                }
            }
        }
    }
    if overflow_depth > 0 {
        let e = too_deep_node(overflow_start.as_ref());
        if let Some(n) = stack.last_mut().and_then(kids) {
            n.push(e);
        }
    }
    let mut rest = stack.split_off(1);
    let mut top = stack.pop().unwrap_or_else(|| SNode::group(GroupKind::And, Vec::new()));
    if let Some(n) = kids(&mut top) {
        for ls in rest.iter_mut() {
            if let Some(ln) = kids(ls) {
                n.append(ln);
            }
        }
    }
    if let SNode::Group { nodes, boost, .. } = &mut top {
        if nodes.len() == 1 && nodes[0].is_group() {
            let b = *boost;
            let mut inner = nodes.remove(0);
            if let SNode::Group { boost: ib, .. } = &mut inner {
                *ib = b;
            }
            return inner;
        }
    }
    top
}

/// BoostPlugin.clean_boost (top-level group only)
pub fn clean_boost(group: &mut SNode) {
    let Some(nodes) = kids(group) else { return };
    for i in 0..nodes.len() {
        if matches!(nodes[i], SNode::Boost { .. }) && (i == 0 || !nodes[i - 1].has_boost()) {
            nodes[i] = to_word(&nodes[i]);
        }
    }
}

/// WildcardPlugin.do_wildcards
pub fn do_wildcards(group: &mut SNode) {
    let Some(nodes) = kids(group) else { return };
    let is_wild = |n: &SNode| matches!(n, SNode::Text { kind: TextKind::Wildcard, .. });
    let mut i = 0;
    while i < nodes.len() {
        if is_wild(&nodes[i]) {
            if i + 1 < nodes.len() && nodes[i + 1].is_text() {
                let next = nodes.remove(i + 1);
                if let (SNode::Text { text, .. }, Some(nt)) = (&mut nodes[i], next.text()) {
                    text.push_str(nt);
                }
            }
            if i > 0 && nodes[i - 1].is_text() {
                let prev = nodes.remove(i - 1);
                if let (SNode::Text { text, .. }, Some(pt)) = (&mut nodes[i - 1], prev.text()) {
                    *text = format!("{pt}{text}");
                }
            } else {
                i += 1;
            }
        } else {
            if nodes[i].is_group() {
                do_wildcards(&mut nodes[i]);
            }
            i += 1;
        }
    }
    for n in nodes.iter_mut() {
        if let SNode::Text { kind: TextKind::Wildcard, text, sc, ec, .. } = n {
            if folds_to_prefix(text, QMARKS) {
                let mut t = text.clone();
                t.pop();
                *n = SNode::Text { kind: TextKind::Prefix, text: t, fieldname: None, boost: 1.0, sc: *sc, ec: *ec };
            }
        }
    }
}

/// FieldAliasPlugin.do_aliases
pub fn do_aliases(group: &mut SNode, reverse: &[(String, String)]) {
    let Some(nodes) = kids(group) else { return };
    for n in nodes.iter_mut() {
        if n.is_group() {
            do_aliases(n, reverse);
        } else if n.has_fieldname() {
            if let Some(f) = n.fieldname().map(str::to_string) {
                if let Some((_, canon)) = reverse.iter().find(|(a, _)| *a == f) {
                    let canon = canon.clone();
                    n.set_fieldname(Some(&canon), true);
                }
            }
        }
    }
}

/// FieldsPlugin._fieldname_is_recognized
fn fieldname_is_recognized(reg: &FieldRegistry, nodes: &[SNode], i: usize) -> bool {
    let Some(name) = nodes[i].fieldname() else { return false };
    if reg.contains(name) {
        return true;
    }
    if !reg.is_bare_json_field(name) {
        return false;
    }
    matches!(nodes.get(i + 1), Some(SNode::Text { kind: TextKind::Wildcard, text, .. }) if text == "*")
}

/// FieldsPlugin.do_fieldnames
pub fn do_fieldnames(group: SNode, reg: &FieldRegistry) -> SNode {
    let empty = group.empty_copy();
    let SNode::Group { nodes, .. } = group else { return group };
    // First pass: demote unknown field names to text.
    let mut first: Vec<SNode> = Vec::with_capacity(nodes.len());
    let mut prev_field: Option<SNode> = None;
    for i in 0..nodes.len() {
        let is_fn = matches!(nodes[i], SNode::Fieldname { .. });
        if is_fn && !fieldname_is_recognized(reg, &nodes, i) {
            let mut node = nodes[i].clone();
            if let Some(p) = &prev_field {
                if let (SNode::Fieldname { original, sc, .. }, SNode::Fieldname { original: po, sc: psc, .. }) = (&mut node, p) {
                    *original = format!("{po}{original}");
                    *sc = *psc;
                }
            }
            prev_field = Some(node);
            continue;
        }
        let mut node = nodes[i].clone();
        if let Some(p) = prev_field.take() {
            if node.has_text() {
                if let (SNode::Text { text, sc, .. }, SNode::Fieldname { original, sc: psc, .. }) = (&mut node, &p) {
                    *text = format!("{original}{text}");
                    *sc = *psc;
                }
            } else {
                first.push(to_word(&p));
            }
        }
        first.push(node);
    }
    if let Some(p) = prev_field {
        first.push(to_word(&p));
    }
    // Second pass, backwards: apply field names to the following node.
    let mut out: Vec<SNode> = Vec::with_capacity(first.len());
    let mut i = first.len();
    while i > 0 {
        i -= 1;
        let mut node = match &first[i] {
            SNode::Fieldname { .. } => to_word(&first[i]),
            SNode::Group { .. } => do_fieldnames(first[i].clone(), reg),
            other => other.clone(),
        };
        if i > 0 && !node.is_ws() {
            if let SNode::Fieldname { fieldname, .. } = &first[i - 1] {
                let f = fieldname.clone();
                node.set_fieldname(f.as_deref(), false);
                i -= 1;
            }
        }
        out.push(node);
    }
    out.reverse();
    let mut g = empty;
    if let Some(n) = kids(&mut g) {
        *n = out;
    }
    g
}

/// CommaValuesPlugin.do_comma_values
pub fn do_comma_values(group: SNode, reg: &FieldRegistry) -> SNode {
    let mut newgroup = group.empty_copy();
    let SNode::Group { nodes, .. } = group else { return group };
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.is_group() {
            out.push(do_comma_values(node, reg));
            continue;
        }
        if let SNode::Text { kind: TextKind::Word, text, fieldname: Some(fname), boost, sc, ec } = &node {
            if text.contains(',') {
                let comma = reg.make_ref(fname).and_then(|r| reg.resolve(&r).map(|rf| rf.spec.comma_values)).unwrap_or(false);
                if comma {
                    let parts: Vec<&str> = text.split(',').filter(|p| !p.is_empty()).collect();
                    if !parts.is_empty() {
                        let words: Vec<SNode> = parts
                            .iter()
                            .map(|p| SNode::Text {
                                kind: TextKind::Word,
                                text: (*p).to_string(),
                                fieldname: Some(fname.clone()),
                                boost: *boost,
                                sc: *sc,
                                ec: *ec,
                            })
                            .collect();
                        out.push(SNode::group(GroupKind::And, words));
                        continue;
                    }
                }
            }
        }
        out.push(node);
    }
    if let Some(n) = kids(&mut newgroup) {
        *n = out;
    }
    newgroup
}

/// MultifieldPlugin.do_multifield (group = OrGroup)
pub fn do_multifield(group: &mut SNode, fields: &[String], boosts: &[(String, f64)]) {
    let Some(nodes) = kids(group) else { return };
    for n in nodes.iter_mut() {
        if n.is_group() {
            do_multifield(n, fields, boosts);
        } else if n.has_fieldname() && n.fieldname().is_none() {
            let copies: Vec<SNode> = fields
                .iter()
                .map(|f| {
                    let mut c = n.clone();
                    c.set_fieldname(Some(f), false);
                    let b = boosts.iter().find(|(k, _)| k == f).map_or(1.0, |(_, b)| *b);
                    c.set_boost(b);
                    c
                })
                .collect();
            *n = SNode::group(GroupKind::Or, copies);
        }
    }
}

/// WhitespacePlugin.remove_whitespace
pub fn remove_whitespace(group: SNode) -> SNode {
    let mut newgroup = group.empty_copy();
    let SNode::Group { nodes, .. } = group else { return group };
    let out: Vec<SNode> = nodes.into_iter().filter(|n| !n.is_ws()).map(|n| if n.is_group() { remove_whitespace(n) } else { n }).collect();
    if let Some(n) = kids(&mut newgroup) {
        *n = out;
    }
    newgroup
}

/// BoostPlugin.do_boost
pub fn do_boost(group: SNode) -> SNode {
    let mut newgroup = group.empty_copy();
    let SNode::Group { nodes, .. } = group else { return group };
    let mut out: Vec<SNode> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let node = match node {
            g @ SNode::Group { .. } => do_boost(g),
            SNode::Boost { boost, .. } if out.last().is_some_and(SNode::has_boost) => {
                if let Some(last) = out.last_mut() {
                    last.set_boost(boost);
                }
                continue;
            }
            b @ SNode::Boost { .. } => to_word(&b),
            other => other,
        };
        out.push(node);
    }
    if let Some(n) = kids(&mut newgroup) {
        *n = out;
    }
    newgroup
}

const OP_ORDER: [OpKind; 6] = [OpKind::Not, OpKind::And, OpKind::Or, OpKind::AndNot, OpKind::AndMaybe, OpKind::Require];

/// OperatorsPlugin._apply_operators (one group, not its subgroups)
fn apply_operators(group: &mut SNode) {
    let SNode::Group { nodes, .. } = group else { return };
    let nonmerging = nodes.iter().filter(|t| matches!(t, SNode::Operator { op, .. } if op.is_infix() && !op.group().merging())).count();
    if nonmerging >= MAX_GROUP_NESTING_DEPTH {
        let e = too_deep_node(nodes.first());
        *group = SNode::group(GroupKind::And, vec![e]);
        return;
    }
    for opk in OP_ORDER {
        let mut i = 0;
        while i < nodes.len() {
            if matches!(&nodes[i], SNode::Operator { op, .. } if *op == opk) {
                i = replace_self(opk, nodes, i);
            } else {
                i += 1;
            }
        }
    }
}

/// OperatorsPlugin.do_operators
pub fn do_operators(group: &mut SNode) {
    apply_operators(group);
    if let Some(nodes) = kids(group) {
        for n in nodes.iter_mut() {
            if n.is_group() {
                do_operators(n);
            }
        }
    }
}
