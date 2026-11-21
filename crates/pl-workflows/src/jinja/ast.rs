//! Nodes of the Jinja2 AST (Jinja2 3.1.6, `jinja2/nodes.py`), generic: fields in the
//! order of `Node.fields` so that `iter_child_nodes` and the visitor order are right.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Template,
    Output,
    Extends,
    For,
    If,
    Macro,
    CallBlock,
    FilterBlock,
    With,
    Block,
    Include,
    Import,
    FromImport,
    Assign,
    AssignBlock,
    Scope,
    ScopedEvalContextModifier,
    Break,
    Continue,
    Name,
    NSRef,
    Const,
    TemplateData,
    Tuple,
    List,
    Dict,
    Pair,
    Keyword,
    CondExpr,
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    And,
    Or,
    Not,
    Neg,
    Pos,
    Concat,
    Compare,
    Operand,
    Getattr,
    Getitem,
    Slice,
    Call,
    Filter,
    Test,
}

impl K {
    /// `type(node).__name__.lower()`
    pub fn py_name(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

#[derive(Debug, Clone)]
pub enum F {
    N(Box<Node>),
    L(Vec<Node>),
    Nil,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub k: K,
    pub lineno: usize,
    /// Name.name, NSRef.name, Filter/Test.name, Block/Macro.name, Keyword.key, Getattr.attr,
    /// Operand.op, Import.target, TemplateData.data
    pub s: String,
    /// NSRef.attr
    pub s2: String,
    /// "load" | "store" | "param" (Name, Tuple, Getattr, Getitem)
    pub ctx: &'static str,
    /// Block.scoped, For.recursive, Include/Import/FromImport.with_context
    pub flag1: bool,
    /// Block.required, Include.ignore_missing
    pub flag2: bool,
    /// FromImport.names (name, alias)
    pub names: Vec<(String, Option<String>)>,
    pub f: Vec<F>,
}

impl Node {
    pub fn new(k: K, lineno: usize, f: Vec<F>) -> Self {
        Node { k, lineno, s: String::new(), s2: String::new(), ctx: "load", flag1: false, flag2: false, names: Vec::new(), f }
    }

    pub fn with_s(mut self, s: impl Into<String>) -> Self {
        self.s = s.into();
        self
    }

    pub fn with_ctx(mut self, ctx: &'static str) -> Self {
        self.ctx = ctx;
        self
    }

    /// Kindknoten in Feldreihenfolge (`iter_child_nodes`).
    pub fn children(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        for f in &self.f {
            match f {
                F::N(n) => out.push(n.as_ref()),
                F::L(l) => out.extend(l.iter()),
                F::Nil => {}
            }
        }
        out
    }

    pub fn children_mut(&mut self) -> Vec<&mut Node> {
        let mut out = Vec::new();
        for f in &mut self.f {
            match f {
                F::N(n) => out.push(n.as_mut()),
                F::L(l) => out.extend(l.iter_mut()),
                F::Nil => {}
            }
        }
        out
    }

    pub fn field(&self, i: usize) -> Option<&Node> {
        match self.f.get(i) {
            Some(F::N(n)) => Some(n),
            _ => None,
        }
    }

    pub fn list(&self, i: usize) -> &[Node] {
        match self.f.get(i) {
            Some(F::L(l)) => l,
            _ => &[],
        }
    }

    pub fn has_ctx(&self) -> bool {
        matches!(self.k, K::Name | K::Tuple | K::Getattr | K::Getitem)
    }

    /// `set_ctx`: breadth-first search over all nodes with a `ctx` field.
    pub fn set_ctx(&mut self, ctx: &'static str) {
        let mut todo: std::collections::VecDeque<&mut Node> = std::collections::VecDeque::new();
        todo.push_back(self);
        while let Some(n) = todo.pop_front() {
            if n.has_ctx() {
                n.ctx = ctx;
            }
            for c in n.children_mut() {
                todo.push_back(c);
            }
        }
    }

    pub fn can_assign(&self) -> bool {
        match self.k {
            K::Name => !matches!(self.s.as_str(), "true" | "false" | "none" | "True" | "False" | "None"),
            K::NSRef => true,
            K::Tuple => self.list(0).iter().all(Node::can_assign),
            _ => false,
        }
    }

    /// `find_all(kind)` (depth-first search, without the node itself).
    pub fn find_all<'a>(&'a self, k: K, out: &mut Vec<&'a Node>) {
        for c in self.children() {
            if c.k == k {
                out.push(c);
            }
            c.find_all(k, out);
        }
    }
}
