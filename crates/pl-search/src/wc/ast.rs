// whoosh_compat/ast.py (whoosh-compat 0.3.0)
//
// Backend-neutral query AST, normalize() and analyze() (incl. the
// rewrite_leaf hook). Differences in representation, not behaviour:
// * startchar/endchar are not carried on AST nodes: they are
//   compare=False informational metadata upstream, and nothing in paperless
//   reads them (diagnostics carry their own span).
// * Python object identity (`is`, `id()`) of Term/Phrase leaves is modelled
//   by `uid`: every leaf the parser builds gets a unique non-zero uid; a leaf
//   built by a host hook has uid 0. uid is excluded from equality, like id().
// * The iterative work stacks of normalize()/analyze() are plain recursion
//   here: parse() caps nesting (TOO_DEEP, 200 per group), and the trees are
//   never aliased (no shared sub-objects), so the per-(object, context) memo
//   is not needed.
// * free_text_tokens() and the Visitor base class are not ported (paperless
//   never calls the former; the emitter matches on the enum directly).

use chrono::{DateTime, Utc};

use super::errors::Diagnostic;
use super::fields::{FieldKind, FieldRef, FieldRegistry, FieldSpec, Multitoken};

/// `Term.text`: `str | int | bool`.
#[derive(Debug, Clone, PartialEq)]
pub enum TermText {
    Str(String),
    Int(u64),
    Bool(bool),
}

impl TermText {
    /// Python `str(text)`.
    pub fn py_str(&self) -> String {
        match self {
            TermText::Str(s) => s.clone(),
            TermText::Int(i) => i.to_string(),
            TermText::Bool(true) => "True".to_string(),
            TermText::Bool(false) => "False".to_string(),
        }
    }
}

/// whoosh_compat/ast.py node classes.
#[derive(Debug, Clone)]
pub enum Node {
    /// ast.Term
    Term { field: Option<FieldRef>, text: TermText, analyzed: bool, uid: u32 },
    /// ast.And
    And(Vec<Node>),
    /// ast.Or
    Or(Vec<Node>),
    /// ast.Not
    Not(Box<Node>),
    /// ast.AndNot(positive, negative)
    AndNot(Box<Node>, Box<Node>),
    /// ast.AndMaybe(required, optional)
    AndMaybe(Box<Node>, Box<Node>),
    /// ast.Require(scored, filter_only)
    Require(Box<Node>, Box<Node>),
    /// ast.Phrase
    Phrase { field: Option<FieldRef>, text: String, slop: u32, words: Option<Vec<String>>, analyzed: bool, uid: u32 },
    /// ast.Prefix
    Prefix { field: Option<FieldRef>, text: String },
    /// ast.Wildcard
    Wildcard { field: Option<FieldRef>, pattern: String },
    /// ast.TermRange
    TermRange { field: Option<FieldRef>, lo: Option<String>, hi: Option<String>, incl_lo: bool, incl_hi: bool },
    /// ast.NumericRange
    NumericRange { field: FieldRef, lo: Option<u64>, hi: Option<u64>, incl_lo: bool, incl_hi: bool },
    /// ast.DateRange
    DateRange { field: FieldRef, lo: Option<DateTime<Utc>>, hi: Option<DateTime<Utc>>, incl_lo: bool, incl_hi: bool },
    /// ast.Every
    Every(Option<FieldRef>),
    /// ast.Nothing
    Nothing,
    /// ast.Boosted
    Boosted(Box<Node>, f64),
    /// ast.ErrorLeaf
    ErrorLeaf(Box<Diagnostic>),
    /// ast.Fuzzy (emit-only)
    Fuzzy { field: FieldRef, text: String, distance: u8, prefix: bool },
}

impl Node {
    pub fn term(field: Option<FieldRef>, text: &str, uid: u32) -> Node {
        Node::Term { field, text: TermText::Str(text.to_string()), analyzed: false, uid }
    }

    pub fn is_nothing(&self) -> bool {
        matches!(self, Node::Nothing)
    }

    fn is_unfielded_every(&self) -> bool {
        matches!(self, Node::Every(None))
    }

    /// The identity of a Term/Phrase leaf (0 = none).
    pub fn uid(&self) -> u32 {
        match self {
            Node::Term { uid, .. } | Node::Phrase { uid, .. } => *uid,
            _ => 0,
        }
    }

    /// `node.field` for leaves that carry one.
    pub fn field(&self) -> Option<&FieldRef> {
        match self {
            Node::Term { field, .. }
            | Node::Phrase { field, .. }
            | Node::Prefix { field, .. }
            | Node::Wildcard { field, .. }
            | Node::TermRange { field, .. } => field.as_ref(),
            Node::Every(f) => f.as_ref(),
            Node::NumericRange { field, .. } | Node::DateRange { field, .. } | Node::Fuzzy { field, .. } => Some(field),
            _ => None,
        }
    }
}

fn opt_ref_eq(a: &Option<FieldRef>, b: &Option<FieldRef>) -> bool {
    a == b
}

/// The `_Interner` equivalence: node equality over compare=True fields plus
/// `Phrase.words`/`analyzed` and `Term.analyzed`; floats by bits (repr), never
/// identity (uid).
pub fn same(a: &Node, b: &Node) -> bool {
    use Node::*;
    match (a, b) {
        (Term { field: f1, text: t1, analyzed: a1, .. }, Term { field: f2, text: t2, analyzed: a2, .. }) => {
            opt_ref_eq(f1, f2) && t1 == t2 && a1 == a2
        }
        (And(x), And(y)) | (Or(x), Or(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same(p, q)),
        (Not(x), Not(y)) => same(x, y),
        (AndNot(p1, n1), AndNot(p2, n2)) | (AndMaybe(p1, n1), AndMaybe(p2, n2)) | (Require(p1, n1), Require(p2, n2)) => {
            same(p1, p2) && same(n1, n2)
        }
        (
            Phrase { field: f1, text: t1, slop: s1, words: w1, analyzed: a1, .. },
            Phrase { field: f2, text: t2, slop: s2, words: w2, analyzed: a2, .. },
        ) => f1 == f2 && t1 == t2 && s1 == s2 && w1 == w2 && a1 == a2,
        (Prefix { field: f1, text: t1 }, Prefix { field: f2, text: t2 }) => f1 == f2 && t1 == t2,
        (Wildcard { field: f1, pattern: t1 }, Wildcard { field: f2, pattern: t2 }) => f1 == f2 && t1 == t2,
        (
            TermRange { field: f1, lo: l1, hi: h1, incl_lo: il1, incl_hi: ih1 },
            TermRange { field: f2, lo: l2, hi: h2, incl_lo: il2, incl_hi: ih2 },
        ) => f1 == f2 && l1 == l2 && h1 == h2 && il1 == il2 && ih1 == ih2,
        (
            NumericRange { field: f1, lo: l1, hi: h1, incl_lo: il1, incl_hi: ih1 },
            NumericRange { field: f2, lo: l2, hi: h2, incl_lo: il2, incl_hi: ih2 },
        ) => f1 == f2 && l1 == l2 && h1 == h2 && il1 == il2 && ih1 == ih2,
        (
            DateRange { field: f1, lo: l1, hi: h1, incl_lo: il1, incl_hi: ih1 },
            DateRange { field: f2, lo: l2, hi: h2, incl_lo: il2, incl_hi: ih2 },
        ) => f1 == f2 && l1 == l2 && h1 == h2 && il1 == il2 && ih1 == ih2,
        (Every(f1), Every(f2)) => f1 == f2,
        (Nothing, Nothing) => true,
        (Boosted(c1, b1), Boosted(c2, b2)) => b1.to_bits() == b2.to_bits() && same(c1, c2),
        (ErrorLeaf(d1), ErrorLeaf(d2)) => d1 == d2,
        (Fuzzy { field: f1, text: t1, distance: d1, prefix: p1 }, Fuzzy { field: f2, text: t2, distance: d2, prefix: p2 }) => {
            f1 == f2 && t1 == t2 && d1 == d2 && p1 == p2
        }
        _ => false,
    }
}

/// ast.py::_dedupe (first-seen order).
fn dedupe(nodes: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if !out.iter().any(|o| same(o, &n)) {
            out.push(n);
        }
    }
    out
}

/// ast.py::_can_still_empty_during_analysis
fn can_still_empty(node: &Node) -> bool {
    match node {
        Node::Term { analyzed, field, .. } | Node::Phrase { analyzed, field, .. } => !*analyzed && field.is_some(),
        Node::And(c) | Node::Or(c) => c.iter().any(can_still_empty),
        Node::Not(c) | Node::Boosted(c, _) => can_still_empty(c),
        Node::AndNot(a, b) | Node::AndMaybe(a, b) | Node::Require(a, b) => can_still_empty(a) || can_still_empty(b),
        _ => false,
    }
}

/// ast.py::_normalize_one, And rule.
fn normalize_and(children: Vec<Node>, post: bool) -> Node {
    if children.is_empty() {
        return Node::Nothing;
    }
    if children.iter().any(Node::is_nothing) {
        return Node::Nothing;
    }
    let mut flat: Vec<Node> = Vec::new();
    for c in children {
        match c {
            Node::And(cc) => flat.extend(cc),
            other => flat.push(other),
        }
    }
    let had_every = flat.iter().any(Node::is_unfielded_every);
    if had_every && (post || !flat.iter().filter(|c| !c.is_unfielded_every()).any(can_still_empty)) {
        flat.retain(|c| !c.is_unfielded_every());
    }
    let mut flat = dedupe(flat);
    if flat.is_empty() {
        return if had_every { Node::Every(None) } else { Node::Nothing };
    }
    if flat.len() == 1 {
        return flat.remove(0);
    }
    Node::And(flat)
}

/// ast.py::_normalize_one, Or rule.
fn normalize_or(children: Vec<Node>) -> Node {
    if children.is_empty() {
        return Node::Nothing;
    }
    let mut flat: Vec<Node> = Vec::new();
    for c in children {
        match c {
            Node::Or(cc) => flat.extend(cc),
            other => flat.push(other),
        }
    }
    if flat.iter().any(Node::is_unfielded_every) {
        return Node::Every(None);
    }
    flat.retain(|c| !c.is_nothing());
    let mut flat = dedupe(flat);
    if flat.is_empty() {
        return Node::Nothing;
    }
    if flat.len() == 1 {
        return flat.remove(0);
    }
    Node::Or(flat)
}

fn normalize_not(child: Node) -> Node {
    if child.is_nothing() {
        return Node::Every(None);
    }
    Node::Not(Box::new(child))
}

fn normalize_andnot(pos: Node, neg: Node) -> Node {
    if pos.is_nothing() {
        return Node::Nothing;
    }
    if neg.is_nothing() {
        return pos;
    }
    Node::AndNot(Box::new(pos), Box::new(neg))
}

fn normalize_andmaybe(req: Node, opt: Node) -> Node {
    if req.is_nothing() {
        return Node::Nothing;
    }
    if opt.is_nothing() {
        return req;
    }
    Node::AndMaybe(Box::new(req), Box::new(opt))
}

fn normalize_require(scored: Node, filt: Node) -> Node {
    if scored.is_nothing() || filt.is_nothing() {
        return Node::Nothing;
    }
    Node::Require(Box::new(scored), Box::new(filt))
}

fn normalize_boosted(child: Node, boost: f64) -> Node {
    if child.is_nothing() {
        return Node::Nothing;
    }
    let (child, boost) = match child {
        Node::Boosted(inner, b) => (*inner, b * boost),
        other => (other, boost),
    };
    if boost == 1.0 {
        return child;
    }
    Node::Boosted(Box::new(child), boost)
}

/// ast.py::_normalize_impl
fn normalize_impl(node: &Node, post: bool) -> Node {
    match node {
        Node::And(c) => normalize_and(c.iter().map(|x| normalize_impl(x, post)).collect(), post),
        Node::Or(c) => normalize_or(c.iter().map(|x| normalize_impl(x, post)).collect()),
        Node::Not(c) => normalize_not(normalize_impl(c, post)),
        Node::AndNot(a, b) => normalize_andnot(normalize_impl(a, post), normalize_impl(b, post)),
        Node::AndMaybe(a, b) => normalize_andmaybe(normalize_impl(a, post), normalize_impl(b, post)),
        Node::Require(a, b) => normalize_require(normalize_impl(a, post), normalize_impl(b, post)),
        Node::Boosted(c, b) => normalize_boosted(normalize_impl(c, post), *b),
        other => other.clone(),
    }
}

/// ast.py::normalize
pub fn normalize(node: &Node) -> Node {
    normalize_impl(node, false)
}

/// ast.py::_leaf_tokens
fn leaf_tokens<'r>(field: Option<&FieldRef>, text: &str, registry: &'r FieldRegistry) -> Option<(Vec<String>, &'r FieldSpec)> {
    let field = field?;
    let resolved = registry.resolve(field)?;
    let spec = resolved.spec;
    if field.json_path.is_none() && !matches!(spec.kind, FieldKind::Text | FieldKind::Keyword) {
        return None;
    }
    let tokens = match &spec.analyzer {
        None => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text.to_string()]
            }
        }
        Some(a) => a(text),
    };
    Some((tokens, spec))
}

/// ast.py::_analyze_term
fn analyze_term(node: &Node, registry: &FieldRegistry, ctx: Multitoken) -> Node {
    let Node::Term { field, text, analyzed, .. } = node else {
        return node.clone();
    };
    if *analyzed {
        return node.clone();
    }
    let Some((tokens, spec)) = leaf_tokens(field.as_ref(), &text.py_str(), registry) else {
        return node.clone();
    };
    let mk = |t: &str| Node::Term { field: field.clone(), text: TermText::Str(t.to_string()), analyzed: true, uid: 0 };
    if tokens.is_empty() {
        return Node::Nothing;
    }
    if tokens.len() == 1 {
        return mk(&tokens[0]);
    }
    let mut mode = spec.multitoken;
    if mode == Multitoken::Default {
        mode = ctx;
    }
    match mode {
        Multitoken::First => mk(&tokens[0]),
        Multitoken::Phrase => {
            Node::Phrase { field: field.clone(), text: tokens.join(" "), words: Some(tokens), analyzed: true, slop: 1, uid: 0 }
        }
        Multitoken::Or => Node::Or(tokens.iter().map(|t| mk(t)).collect()),
        _ => Node::And(tokens.iter().map(|t| mk(t)).collect()),
    }
}

/// ast.py::_analyze_phrase
fn analyze_phrase(node: &Node, registry: &FieldRegistry) -> Node {
    let Node::Phrase { field, text, slop, analyzed, .. } = node else {
        return node.clone();
    };
    if *analyzed {
        return node.clone();
    }
    let Some((tokens, _)) = leaf_tokens(field.as_ref(), text, registry) else {
        return node.clone();
    };
    if tokens.is_empty() {
        return Node::Nothing;
    }
    Node::Phrase { field: field.clone(), text: tokens.join(" "), words: Some(tokens), analyzed: true, slop: *slop, uid: 0 }
}

/// `analyze()`'s rewrite_leaf hook: `None` = "returned the leaf itself".
pub type RewriteLeaf<'a> = &'a dyn Fn(&Node) -> Option<Node>;

/// ast.py::_analyze_binary_drop; `None` = fall back to the ordinary rule.
fn binary_drop(orig_l: &Node, orig_r: &Node, new_l: &Node, new_r: &Node) -> Option<Node> {
    let ld = new_l.is_nothing() && !orig_l.is_nothing();
    let rd = new_r.is_nothing() && !orig_r.is_nothing();
    if !(ld || rd) {
        return None;
    }
    if ld && rd {
        return Some(Node::Nothing);
    }
    Some(if ld { new_r.clone() } else { new_l.clone() })
}

/// ast.py::_analyze_walk + _analyze_combine
fn walk(node: &Node, registry: &FieldRegistry, ctx: Multitoken, hook: Option<RewriteLeaf<'_>>, pin: Option<(u32, &Node)>) -> Node {
    if let Some((uid, analysis)) = pin {
        if uid != 0 && node.uid() == uid {
            return analysis.clone();
        }
    }
    match node {
        Node::And(children) => {
            let new: Vec<Node> = children.iter().map(|c| walk(c, registry, Multitoken::And, hook, pin)).collect();
            let kept: Vec<Node> = children.iter().zip(new).filter(|(o, n)| !(n.is_nothing() && !o.is_nothing())).map(|(_, n)| n).collect();
            normalize_and(kept, true)
        }
        Node::Or(children) => normalize_or(children.iter().map(|c| walk(c, registry, Multitoken::Or, hook, pin)).collect()),
        Node::Not(c) => normalize_not(walk(c, registry, ctx, hook, pin)),
        Node::AndNot(a, b) | Node::AndMaybe(a, b) | Node::Require(a, b) => {
            let na = walk(a, registry, ctx, hook, pin);
            let nb = walk(b, registry, ctx, hook, pin);
            if let Some(o) = binary_drop(a, b, &na, &nb) {
                return o;
            }
            match node {
                Node::AndNot(..) => normalize_andnot(na, nb),
                Node::AndMaybe(..) => normalize_andmaybe(na, nb),
                _ => normalize_require(na, nb),
            }
        }
        Node::Boosted(c, b) => normalize_boosted(walk(c, registry, ctx, hook, pin), *b),
        Node::Term { .. } => {
            let mut analyzed = analyze_term(node, registry, ctx);
            analyzed = match analyzed {
                Node::And(c) => normalize_and(c, true),
                Node::Or(c) => normalize_or(c),
                other => other,
            };
            match hook {
                Some(h) => apply_rewrite(node, analyzed, registry, ctx, h),
                None => analyzed,
            }
        }
        Node::Phrase { .. } => {
            let analyzed = analyze_phrase(node, registry);
            match hook {
                Some(h) => apply_rewrite(node, analyzed, registry, ctx, h),
                None => analyzed,
            }
        }
        other => other.clone(),
    }
}

/// ast.py::_apply_rewrite
fn apply_rewrite(leaf: &Node, analyzed: Node, registry: &FieldRegistry, ctx: Multitoken, hook: RewriteLeaf<'_>) -> Node {
    match hook(leaf) {
        None => analyzed,
        Some(replacement) => {
            let normalized = normalize_impl(&replacement, false);
            walk(&normalized, registry, ctx, None, Some((leaf.uid(), &analyzed)))
        }
    }
}

/// ast.py::analyze
pub fn analyze(node: &Node, registry: &FieldRegistry, default_mode: Multitoken, rewrite_leaf: Option<RewriteLeaf<'_>>) -> Node {
    let node = normalize_impl(node, false);
    walk(&node, registry, default_mode, rewrite_leaf, None)
}
