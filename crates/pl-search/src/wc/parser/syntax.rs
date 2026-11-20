// whoosh_compat/parser/syntax.py (whoosh-compat 0.3.0)
//
// The intermediate syntax tree. Python's class hierarchy is flattened into
// one enum; the class-level flags (`has_fieldname`, `has_boost`,
// `has_text`, `merging`, `is_ws()`, `is_text()`) are methods. The node
// subclasses defined in plugins.py/dateparse.py (QuotedWordNode,
// WildcardNode, PrefixNode, PhraseNode, BoostNode, OpenBracket,
// CloseBracket, EveryNode, DateRangeSyntaxNode, DateErrorNode) live here too.
// `query()` (the AST conversion) is in parser/mod.rs because it calls the
// QueryParser hooks. Character positions are byte offsets into the query
// (Python uses code-point offsets); they are only ever compared with each
// other, never reported.

use chrono::{DateTime, Utc};

use crate::wc::errors::Diagnostic;

/// Which group class (AndGroup, OrGroup, AndNotGroup, AndMaybeGroup,
/// RequireGroup, NotGroup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    And,
    Or,
    AndNot,
    AndMaybe,
    Require,
    Not,
}

impl GroupKind {
    /// `GroupNode.merging`
    pub fn merging(self) -> bool {
        matches!(self, GroupKind::And | GroupKind::Or)
    }
    /// `has_boost` (BinaryGroup sets False; Wrapper keeps GroupNode's True)
    pub fn has_boost(self) -> bool {
        !matches!(self, GroupKind::AndNot | GroupKind::AndMaybe | GroupKind::Require)
    }
}

/// TextNode subclasses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// syntax.WordNode
    Word,
    /// SingleQuotePlugin.QuotedWordNode (is_quoted = True)
    QuotedWord,
    /// WildcardPlugin.WildcardNode
    Wildcard,
    /// WildcardPlugin.PrefixNode
    Prefix,
    /// PhrasePlugin.PhraseNode
    Phrase { slop: u32 },
}

/// Operator node: optype (Prefix/Infix) + grouptype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// PrefixOperator / NotGroup
    Not,
    And,
    Or,
    AndNot,
    AndMaybe,
    Require,
}

impl OpKind {
    pub fn group(self) -> GroupKind {
        match self {
            OpKind::Not => GroupKind::Not,
            OpKind::And => GroupKind::And,
            OpKind::Or => GroupKind::Or,
            OpKind::AndNot => GroupKind::AndNot,
            OpKind::AndMaybe => GroupKind::AndMaybe,
            OpKind::Require => GroupKind::Require,
        }
    }
    pub fn is_infix(self) -> bool {
        !matches!(self, OpKind::Not)
    }
}

pub type Pos = Option<usize>;

#[derive(Debug, Clone)]
pub enum SNode {
    /// syntax.Whitespace
    Whitespace { sc: Pos, ec: Pos },
    /// syntax.FieldnameNode
    Fieldname { fieldname: Option<String>, original: String, sc: Pos, ec: Pos },
    /// GroupNode subclasses
    Group { kind: GroupKind, nodes: Vec<SNode>, boost: f64 },
    /// TextNode subclasses
    Text { kind: TextKind, text: String, fieldname: Option<String>, boost: f64, sc: Pos, ec: Pos },
    /// syntax.RangeNode
    Range {
        start: Option<String>,
        end: Option<String>,
        startexcl: bool,
        endexcl: bool,
        fieldname: Option<String>,
        sc: Pos,
        ec: Pos,
    },
    /// syntax.Operator subclasses
    Operator { op: OpKind, sc: Pos, ec: Pos },
    /// BoostPlugin.BoostNode
    Boost { original: String, boost: f64, sc: Pos, ec: Pos },
    /// GroupPlugin.OpenBracket
    Open { sc: Pos, ec: Pos },
    /// GroupPlugin.CloseBracket
    Close { sc: Pos, ec: Pos },
    /// EveryPlugin.EveryNode
    Every { sc: Pos, ec: Pos },
    /// syntax.ErrorNode (TOO_DEEP); span is that of the node it stands for
    ErrorNode { message: String, sc: Pos, ec: Pos },
    /// dateparse.DateRangeSyntaxNode
    DateRange {
        fieldname: String,
        lo: Option<DateTime<Utc>>,
        hi: Option<DateTime<Utc>>,
        incl_lo: bool,
        incl_hi: bool,
        boost: f64,
        sc: Pos,
        ec: Pos,
    },
    /// dateparse.DateErrorNode
    DateError { diag: Box<Diagnostic>, sc: Pos, ec: Pos },
}

impl SNode {
    pub fn group(kind: GroupKind, nodes: Vec<SNode>) -> SNode {
        SNode::Group { kind, nodes, boost: 1.0 }
    }

    pub fn word(text: String, sc: Pos, ec: Pos) -> SNode {
        SNode::Text { kind: TextKind::Word, text, fieldname: None, boost: 1.0, sc, ec }
    }

    pub fn startchar(&self) -> Pos {
        match self {
            SNode::Group { nodes, .. } => nodes.first().and_then(SNode::startchar),
            SNode::Whitespace { sc, .. }
            | SNode::Fieldname { sc, .. }
            | SNode::Text { sc, .. }
            | SNode::Range { sc, .. }
            | SNode::Operator { sc, .. }
            | SNode::Boost { sc, .. }
            | SNode::Open { sc, .. }
            | SNode::Close { sc, .. }
            | SNode::Every { sc, .. }
            | SNode::ErrorNode { sc, .. }
            | SNode::DateRange { sc, .. }
            | SNode::DateError { sc, .. } => *sc,
        }
    }

    pub fn endchar(&self) -> Pos {
        match self {
            SNode::Group { nodes, .. } => nodes.last().and_then(SNode::endchar),
            SNode::Whitespace { ec, .. }
            | SNode::Fieldname { ec, .. }
            | SNode::Text { ec, .. }
            | SNode::Range { ec, .. }
            | SNode::Operator { ec, .. }
            | SNode::Boost { ec, .. }
            | SNode::Open { ec, .. }
            | SNode::Close { ec, .. }
            | SNode::Every { ec, .. }
            | SNode::ErrorNode { ec, .. }
            | SNode::DateRange { ec, .. }
            | SNode::DateError { ec, .. } => *ec,
        }
    }

    /// `set_range` (GroupNode: on every child).
    pub fn set_range(&mut self, s: Pos, e: Pos) {
        match self {
            SNode::Group { nodes, .. } => {
                for n in nodes {
                    n.set_range(s, e);
                }
            }
            SNode::Whitespace { sc, ec }
            | SNode::Fieldname { sc, ec, .. }
            | SNode::Text { sc, ec, .. }
            | SNode::Range { sc, ec, .. }
            | SNode::Operator { sc, ec, .. }
            | SNode::Boost { sc, ec, .. }
            | SNode::Open { sc, ec }
            | SNode::Close { sc, ec }
            | SNode::Every { sc, ec }
            | SNode::ErrorNode { sc, ec, .. }
            | SNode::DateRange { sc, ec, .. }
            | SNode::DateError { sc, ec, .. } => {
                *sc = s;
                *ec = e;
            }
        }
    }

    pub fn is_ws(&self) -> bool {
        matches!(self, SNode::Whitespace { .. })
    }

    /// `is_text()` (TextNode)
    pub fn is_text(&self) -> bool {
        matches!(self, SNode::Text { .. })
    }

    pub fn is_group(&self) -> bool {
        matches!(self, SNode::Group { .. })
    }

    /// `has_text`
    pub fn has_text(&self) -> bool {
        matches!(self, SNode::Text { .. })
    }

    /// `has_fieldname`
    pub fn has_fieldname(&self) -> bool {
        matches!(
            self,
            SNode::Fieldname { .. } | SNode::Text { .. } | SNode::Range { .. } | SNode::DateRange { .. }
        )
    }

    /// `has_boost`
    pub fn has_boost(&self) -> bool {
        match self {
            SNode::Group { kind, .. } => kind.has_boost(),
            SNode::Text { .. } | SNode::DateRange { .. } => true,
            _ => false,
        }
    }

    /// `node.fieldname` (None for nodes without one)
    pub fn fieldname(&self) -> Option<&str> {
        match self {
            SNode::Fieldname { fieldname, .. }
            | SNode::Text { fieldname, .. }
            | SNode::Range { fieldname, .. } => fieldname.as_deref(),
            SNode::DateRange { fieldname, .. } => Some(fieldname.as_str()),
            _ => None,
        }
    }

    /// `set_fieldname(name, override)`
    pub fn set_fieldname(&mut self, name: Option<&str>, ovr: bool) {
        match self {
            SNode::Group { nodes, .. } => {
                for n in nodes {
                    n.set_fieldname(name, ovr);
                }
            }
            SNode::Fieldname { fieldname, .. }
            | SNode::Text { fieldname, .. }
            | SNode::Range { fieldname, .. } => {
                if fieldname.is_none() || ovr {
                    *fieldname = name.map(str::to_string);
                }
            }
            // fieldname is never None on a DateRangeSyntaxNode
            SNode::DateRange { fieldname, .. } if ovr => {
                if let Some(n) = name {
                    *fieldname = n.to_string();
                }
            }
            _ => {}
        }
    }

    /// `set_boost(boost)` (no-op without has_boost)
    pub fn set_boost(&mut self, b: f64) {
        if !self.has_boost() {
            return;
        }
        match self {
            SNode::Group { boost, .. } | SNode::Text { boost, .. } | SNode::DateRange { boost, .. } => {
                *boost = b;
            }
            _ => {}
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            SNode::Text { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    /// `type(node) is syntax.WordNode` (excludes QuotedWordNode)
    pub fn is_plain_word(&self) -> bool {
        matches!(self, SNode::Text { kind: TextKind::Word, .. })
    }

    /// `GroupNode.empty_copy()`
    pub fn empty_copy(&self) -> SNode {
        match self {
            SNode::Group { kind, boost, .. } => SNode::Group {
                kind: *kind,
                nodes: Vec::new(),
                boost: if kind.has_boost() { *boost } else { 1.0 },
            },
            other => other.clone(),
        }
    }
}

/// syntax.py::to_word(FieldnameNode | BoostNode)
pub fn to_word(n: &SNode) -> SNode {
    let original = match n {
        SNode::Fieldname { original, .. } | SNode::Boost { original, .. } => original.clone(),
        other => other.text().unwrap_or_default().to_string(),
    };
    SNode::word(original, n.startchar(), n.endchar())
}

/// PrefixOperator / InfixOperator `replace_self`; returns the new position.
pub fn replace_self(op: OpKind, group: &mut Vec<SNode>, position: usize) -> usize {
    if !op.is_infix() {
        // syntax.py::PrefixOperator.replace_self
        let length = group.len();
        group.remove(position);
        if position + 1 < length {
            let n = group.remove(position);
            group.insert(position, SNode::group(GroupKind::Not, vec![n]));
        }
        return position;
    }
    // syntax.py::InfixOperator.replace_self (all operators are leftassoc)
    let gtype = op.group();
    if position > 0 && position + 1 < group.len() {
        let left_is_gtype = matches!(&group[position - 1], SNode::Group { kind, .. } if *kind == gtype);
        if gtype.merging() && left_is_gtype {
            let right = group.remove(position + 1);
            group.remove(position);
            if let SNode::Group { nodes, .. } = &mut group[position - 1] {
                nodes.push(right);
            }
        } else {
            let right = group.remove(position + 1);
            group.remove(position);
            let left = group.remove(position - 1);
            group.insert(position - 1, SNode::group(gtype, vec![left, right]));
        }
    } else {
        group.remove(position);
    }
    position
}
