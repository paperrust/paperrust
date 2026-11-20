//! Port of the parts of the Jinja2 compiler relevant for `jinja2.meta.find_undeclared_variables`
//! (Jinja2 3.1.6: `jinja2/idtracking.py` complete, `jinja2/compiler.py::CodeGenerator`
//! traversal only, frames, `enter_frame` and `fail` checks, `jinja2/meta.py::TrackingCodeGenerator`).
//! Not ported: constant folding (`optimizeconst`) – see Known differences.

use super::JErr;
use super::ast::{K, Node};
use crate::pyfmt::str_repr;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    Param,
    Resolve,
    Alias,
    Undefined,
}

#[derive(Debug, Clone)]
struct Sym {
    level: usize,
    parent: Option<usize>,
    refs: HashMap<String, String>,
    loads: HashMap<String, (Load, Option<String>)>,
    stores: HashSet<String>,
}

#[derive(Default)]
struct Arena {
    syms: Vec<Sym>,
}

impl Arena {
    fn new_sym(&mut self, parent: Option<usize>, level: Option<usize>) -> usize {
        let level = match (level, parent) {
            (Some(l), _) => l,
            (None, None) => 0,
            (None, Some(p)) => self.syms[p].level + 1,
        };
        self.syms.push(Sym {
            level,
            parent,
            refs: HashMap::new(),
            loads: HashMap::new(),
            stores: HashSet::new(),
        });
        self.syms.len() - 1
    }

    fn copy(&mut self, id: usize) -> usize {
        let s = self.syms[id].clone();
        self.syms.push(s);
        self.syms.len() - 1
    }

    fn define_ref(&mut self, id: usize, name: &str, load: Option<(Load, Option<String>)>) -> String {
        let ident = format!("l_{}_{}", self.syms[id].level, name);
        self.syms[id].refs.insert(name.to_string(), ident.clone());
        if let Some(l) = load {
            self.syms[id].loads.insert(ident.clone(), l);
        }
        ident
    }

    fn find_ref(&self, id: usize, name: &str) -> Option<String> {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if let Some(r) = self.syms[c].refs.get(name) {
                return Some(r.clone());
            }
            cur = self.syms[c].parent;
        }
        None
    }

    fn reference(&self, id: usize, name: &str) -> Result<String, JErr> {
        self.find_ref(id, name).ok_or_else(|| {
            JErr::internal(format!(
                "Tried to resolve a name to a reference that was unknown to the frame ({})",
                str_repr(name)
            ))
        })
    }

    fn store(&mut self, id: usize, name: &str) {
        self.syms[id].stores.insert(name.to_string());
        if !self.syms[id].refs.contains_key(name) {
            if let Some(p) = self.syms[id].parent
                && let Some(outer) = self.find_ref(p, name) {
                    self.define_ref(id, name, Some((Load::Alias, Some(outer))));
                    return;
                }
            self.define_ref(id, name, Some((Load::Undefined, None)));
        }
    }

    fn declare_parameter(&mut self, id: usize, name: &str) -> String {
        self.syms[id].stores.insert(name.to_string());
        self.define_ref(id, name, Some((Load::Param, None)))
    }

    fn load(&mut self, id: usize, name: &str) {
        if self.find_ref(id, name).is_none() {
            self.define_ref(id, name, Some((Load::Resolve, Some(name.to_string()))));
        }
    }

    fn branch_update(&mut self, id: usize, branches: &[usize]) {
        let mut stores: HashSet<String> = HashSet::new();
        for b in branches {
            stores.extend(self.syms[*b].stores.iter().cloned());
        }
        for s in self.syms[id].stores.iter() {
            stores.remove(s);
        }
        for b in branches {
            let (refs, loads, st) = {
                let bs = &self.syms[*b];
                (bs.refs.clone(), bs.loads.clone(), bs.stores.clone())
            };
            let me = &mut self.syms[id];
            me.refs.extend(refs);
            me.loads.extend(loads);
            me.stores.extend(st);
        }
        for name in stores {
            let Some(target) = self.find_ref(id, &name) else { continue };
            if let Some(p) = self.syms[id].parent
                && let Some(outer) = self.find_ref(p, &name) {
                    self.syms[id].loads.insert(target, (Load::Alias, Some(outer)));
                    continue;
                }
            self.syms[id].loads.insert(target, (Load::Resolve, Some(name)));
        }
    }

    // ---------------------------------------------------- FrameSymbolVisitor

    fn fsv(&mut self, cur: &mut usize, node: &Node, sap: bool) {
        match node.k {
            K::Name => {
                if sap || node.ctx == "param" {
                    self.declare_parameter(*cur, &node.s);
                } else if node.ctx == "store" {
                    self.store(*cur, &node.s);
                } else if node.ctx == "load" {
                    self.load(*cur, &node.s);
                }
            }
            K::NSRef => self.load(*cur, &node.s),
            K::If => {
                if let Some(t) = node.field(0) {
                    self.fsv(cur, t, sap);
                }
                let original = *cur;
                let inner = |ar: &mut Arena, nodes: &[Node]| -> usize {
                    let rv = ar.copy(original);
                    let mut c = rv;
                    for sub in nodes {
                        ar.fsv(&mut c, sub, sap);
                    }
                    rv
                };
                let b = inner(self, node.list(1));
                let e = inner(self, node.list(2));
                let el = inner(self, node.list(3));
                *cur = original;
                self.branch_update(original, &[b, e, el]);
            }
            K::Macro => self.store(*cur, &node.s),
            K::Import => {
                for c in node.children() {
                    self.fsv(cur, c, sap);
                }
                self.store(*cur, &node.s);
            }
            K::FromImport => {
                for c in node.children() {
                    self.fsv(cur, c, sap);
                }
                for (name, alias) in &node.names {
                    self.store(*cur, alias.as_deref().unwrap_or(name));
                }
            }
            K::Assign => {
                if let Some(v) = node.field(1) {
                    self.fsv(cur, v, sap);
                }
                if let Some(t) = node.field(0) {
                    self.fsv(cur, t, sap);
                }
            }
            K::For => {
                if let Some(it) = node.field(1) {
                    self.fsv(cur, it, sap);
                }
            }
            K::CallBlock => {
                if let Some(c) = node.field(0) {
                    self.fsv(cur, c, sap);
                }
            }
            K::FilterBlock => {
                if let Some(f) = node.field(1) {
                    self.fsv(cur, f, sap);
                }
            }
            K::With => {
                for v in node.list(1) {
                    self.fsv(cur, v, false);
                }
            }
            K::AssignBlock => {
                if let Some(t) = node.field(0) {
                    self.fsv(cur, t, sap);
                }
            }
            K::Scope | K::Block => {}
            _ => {
                for c in node.children() {
                    self.fsv(cur, c, sap);
                }
            }
        }
    }

    /// `Symbols.analyze_node` (RootVisitor)
    fn analyze(&mut self, id: usize, node: &Node, for_branch: &str) -> Result<(), JErr> {
        let mut cur = id;
        match node.k {
            K::Template | K::Block | K::Macro | K::FilterBlock | K::Scope | K::If | K::ScopedEvalContextModifier => {
                for c in node.children() {
                    self.fsv(&mut cur, c, false);
                }
            }
            K::AssignBlock => {
                for c in node.list(2) {
                    self.fsv(&mut cur, c, false);
                }
            }
            K::CallBlock => {
                for i in 1..=3 {
                    for c in node.list(i) {
                        self.fsv(&mut cur, c, false);
                    }
                }
            }
            K::For => match for_branch {
                "body" => {
                    if let Some(t) = node.field(0) {
                        self.fsv(&mut cur, t, true);
                    }
                    for c in node.list(2) {
                        self.fsv(&mut cur, c, false);
                    }
                }
                "else" => {
                    for c in node.list(3) {
                        self.fsv(&mut cur, c, false);
                    }
                }
                _ => {
                    if let Some(t) = node.field(0) {
                        self.fsv(&mut cur, t, true);
                    }
                    if let Some(t) = node.field(4) {
                        self.fsv(&mut cur, t, false);
                    }
                }
            },
            K::With => {
                for t in node.list(0) {
                    self.fsv(&mut cur, t, false);
                }
                for c in node.list(2) {
                    self.fsv(&mut cur, c, false);
                }
            }
            other => {
                return Err(JErr::internal(format!("Cannot find symbols for {:?}", other)));
            }
        }
        Ok(())
    }
}

/// `compiler.find_undeclared` (UndeclaredNameVisitor)
fn find_undeclared<'a>(nodes: impl IntoIterator<Item = &'a Node>, names: &[&str]) -> HashSet<String> {
    let mut names: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
    let mut und: HashSet<String> = HashSet::new();
    fn visit(n: &Node, names: &mut HashSet<String>, und: &mut HashSet<String>) -> bool {
        match n.k {
            K::Name => {
                if n.ctx == "load" && names.contains(&n.s) {
                    und.insert(n.s.clone());
                    if *und == *names {
                        return true;
                    }
                } else {
                    names.remove(&n.s);
                }
                false
            }
            K::Block => false,
            _ => n.children().into_iter().any(|c| visit(c, names, und)),
        }
    }
    for n in nodes {
        if visit(n, &mut names, &mut und) {
            break;
        }
    }
    und
}

#[derive(Clone, Copy)]
struct Frame {
    sym: usize,
    toplevel: bool,
    rootlevel: bool,
    require_output_check: bool,
    loop_frame: bool,
    block_frame: bool,
    soft_frame: bool,
}

enum Stop {
    Exit,
    Err(JErr),
}

impl From<JErr> for Stop {
    fn from(e: JErr) -> Self {
        Stop::Err(e)
    }
}

type VR = Result<(), Stop>;

const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
    "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

pub struct Env<'a> {
    pub filters: &'a [&'a str],
    pub tests: &'a [&'a str],
    pub globals: &'a [&'a str],
}

struct Gen<'a> {
    ar: Arena,
    env: &'a Env<'a>,
    undeclared: HashSet<String>,
    has_known_extends: bool,
    extends_so_far: usize,
}

impl<'a> Gen<'a> {
    fn root_frame(&mut self) -> Frame {
        let sym = self.ar.new_sym(None, None);
        Frame {
            sym,
            toplevel: false,
            rootlevel: false,
            require_output_check: false,
            loop_frame: false,
            block_frame: false,
            soft_frame: false,
        }
    }

    fn inner(&mut self, f: &Frame) -> Frame {
        let sym = self.ar.new_sym(Some(f.sym), None);
        Frame {
            sym,
            toplevel: false,
            rootlevel: false,
            require_output_check: f.require_output_check,
            loop_frame: false,
            block_frame: false,
            soft_frame: false,
        }
    }

    fn soft(&mut self, f: &Frame) -> Frame {
        let sym = self.ar.copy(f.sym);
        Frame { sym, rootlevel: false, soft_frame: true, ..*f }
    }

    fn enter_frame(&mut self, f: &Frame) {
        for (action, param) in self.ar.syms[f.sym].loads.values() {
            if *action == Load::Resolve
                && let Some(p) = param
                    && !self.env.globals.contains(&p.as_str()) {
                        self.undeclared.insert(p.clone());
                    }
        }
    }

    fn fail(&self, msg: String, lineno: usize) -> Stop {
        Stop::Err(JErr::compile_assertion(msg, lineno))
    }

    fn blockvisit(&mut self, nodes: &[Node], f: &Frame) -> Result<(), JErr> {
        for n in nodes {
            match self.visit(n, f) {
                Ok(()) => {}
                Err(Stop::Exit) => return Ok(()),
                Err(Stop::Err(e)) => return Err(e),
            }
        }
        Ok(())
    }

    fn bv(&mut self, nodes: &[Node], f: &Frame) -> VR {
        self.blockvisit(nodes, f).map_err(Stop::Err)
    }

    fn opt(&mut self, n: Option<&Node>, f: &Frame) -> VR {
        match n {
            Some(n) => self.visit(n, f),
            None => Ok(()),
        }
    }

    fn signature(&mut self, node: &Node, f: &Frame) -> VR {
        // Call/Filter/Test: [node, args, kwargs, dyn_args, dyn_kwargs]
        let kwargs = node.list(2);
        let workaround = kwargs.iter().any(|k| PY_KEYWORDS.contains(&k.s.as_str()));
        for a in node.list(1) {
            self.visit(a, f)?;
        }
        if !workaround {
            for k in kwargs {
                self.visit(k, f)?;
            }
        }
        self.opt(node.field(3), f)?;
        if workaround {
            for k in kwargs {
                self.opt(k.field(0), f)?;
            }
        }
        self.opt(node.field(4), f)
    }

    fn macro_body(&mut self, node: &Node, frame: &Frame) -> VR {
        let (args, defaults, body) = if node.k == K::Macro {
            (node.list(0), node.list(1), node.list(2))
        } else {
            (node.list(1), node.list(2), node.list(3))
        };
        let mut mf = self.inner(frame);
        self.ar.analyze(mf.sym, node, "")?;
        let mut explicit_caller = None;
        let mut skip_special: HashSet<&str> = HashSet::new();
        for (idx, arg) in args.iter().enumerate() {
            if arg.s == "caller" {
                explicit_caller = Some(idx);
            }
            if arg.s == "kwargs" || arg.s == "varargs" {
                skip_special.insert(arg.s.as_str());
            }
            self.ar.reference(mf.sym, &arg.s)?;
        }
        let und = find_undeclared(body.iter(), &["caller", "kwargs", "varargs"]);
        if und.contains("caller") {
            if let Some(idx) = explicit_caller {
                // node.defaults[explicit_caller - len(node.args)]
                if args.len() - idx > defaults.len() {
                    return Err(self.fail(
                        "When defining macros or call blocks the special \"caller\" argument must be omitted or be given a default."
                            .into(),
                        node.lineno,
                    ));
                }
            } else {
                self.ar.declare_parameter(mf.sym, "caller");
            }
        }
        if und.contains("kwargs") && !skip_special.contains("kwargs") {
            self.ar.declare_parameter(mf.sym, "kwargs");
        }
        if und.contains("varargs") && !skip_special.contains("varargs") {
            self.ar.declare_parameter(mf.sym, "varargs");
        }
        mf.require_output_check = false;
        self.ar.analyze(mf.sym, node, "")?;
        self.enter_frame(&mf);
        for (idx, _arg) in args.iter().enumerate() {
            let back = args.len() - idx;
            if back <= defaults.len() {
                let d = &defaults[defaults.len() - back];
                self.visit(d, &mf)?;
            }
        }
        self.bv(body, &mf)
    }

    fn filter_test(&mut self, node: &Node, f: &Frame, is_filter: bool) -> VR {
        let known = if is_filter { self.env.filters } else { self.env.tests };
        if !known.contains(&node.s.as_str()) && !f.soft_frame {
            let tn = if is_filter { "filter" } else { "test" };
            return Err(self.fail(format!("No {tn} named {}.", str_repr(&node.s)), node.lineno));
        }
        self.opt(node.field(0), f)?;
        self.signature(node, f)
    }

    fn visit(&mut self, node: &Node, f: &Frame) -> VR {
        match node.k {
            K::Template => Err(Stop::Err(JErr::internal("no root frame allowed"))),
            K::Output => {
                if f.require_output_check && self.has_known_extends {
                    return Ok(());
                }
                for c in node.list(0) {
                    if c.k != K::TemplateData {
                        self.visit(c, f)?;
                    }
                }
                Ok(())
            }
            K::Extends => {
                if !f.toplevel {
                    return Err(self.fail("cannot use extend from a non top-level scope".into(), node.lineno));
                }
                if self.extends_so_far > 0 && self.has_known_extends {
                    return Err(Stop::Exit);
                }
                self.opt(node.field(0), f)?;
                if f.rootlevel {
                    self.has_known_extends = true;
                }
                self.extends_so_far += 1;
                Ok(())
            }
            K::Block | K::Const | K::TemplateData | K::Break | K::Continue => Ok(()),
            K::Include => self.opt(node.field(0), f),
            K::Import => {
                self.ar.reference(f.sym, &node.s)?;
                self.opt(node.field(0), f)
            }
            K::FromImport => {
                self.opt(node.field(0), f)?;
                for (name, alias) in &node.names {
                    self.ar.reference(f.sym, alias.as_deref().unwrap_or(name))?;
                }
                Ok(())
            }
            K::For => self.visit_for(node, f),
            K::If => {
                let iff = self.soft(f);
                self.opt(node.field(0), &iff)?;
                self.bv(node.list(1), &iff)?;
                for elif in node.list(2) {
                    self.opt(elif.field(0), &iff)?;
                    self.bv(elif.list(1), &iff)?;
                }
                if !node.list(3).is_empty() {
                    self.bv(node.list(3), &iff)?;
                }
                Ok(())
            }
            K::Macro => {
                self.macro_body(node, f)?;
                self.ar.reference(f.sym, &node.s)?;
                Ok(())
            }
            K::CallBlock => {
                self.macro_body(node, f)?;
                self.opt(node.field(0), f)
            }
            K::FilterBlock => {
                let ff = self.inner(f);
                self.ar.analyze(ff.sym, node, "")?;
                self.enter_frame(&ff);
                self.bv(node.list(0), &ff)?;
                self.opt(node.field(1), &ff)
            }
            K::With => {
                let wf = self.inner(f);
                self.ar.analyze(wf.sym, node, "")?;
                self.enter_frame(&wf);
                for (t, e) in node.list(0).iter().zip(node.list(1).iter()) {
                    self.visit(t, &wf)?;
                    self.visit(e, f)?;
                }
                self.bv(node.list(2), &wf)
            }
            K::Assign => {
                let mut ns = Vec::new();
                node.find_all(K::NSRef, &mut ns);
                let mut seen = HashSet::new();
                for r in ns {
                    if seen.insert(r.s.clone()) {
                        self.ar.reference(f.sym, &r.s)?;
                    }
                }
                self.opt(node.field(0), f)?;
                self.opt(node.field(1), f)
            }
            K::AssignBlock => {
                let mut bf = self.inner(f);
                bf.require_output_check = false;
                self.ar.analyze(bf.sym, node, "")?;
                self.enter_frame(&bf);
                self.bv(node.list(2), &bf)?;
                self.opt(node.field(0), f)?;
                self.opt(node.field(1), &bf)
            }
            K::Scope => {
                let sf = self.inner(f);
                self.ar.analyze(sf.sym, node, "")?;
                self.enter_frame(&sf);
                self.bv(node.list(0), &sf)
            }
            K::ScopedEvalContextModifier => {
                for kw in node.list(0) {
                    self.opt(kw.field(0), f)?;
                }
                for c in node.list(1) {
                    self.visit(c, f)?;
                }
                Ok(())
            }
            K::Name | K::NSRef => {
                self.ar.reference(f.sym, &node.s)?;
                Ok(())
            }
            K::CondExpr => {
                let sf = self.soft(f);
                self.opt(node.field(1), &sf)?;
                self.opt(node.field(0), &sf)?;
                self.opt(node.field(2), &sf)
            }
            K::Call => {
                self.opt(node.field(0), f)?;
                self.signature(node, f)
            }
            K::Filter => self.filter_test(node, f, true),
            K::Test => self.filter_test(node, f, false),
            _ => {
                for c in node.children() {
                    self.visit(c, f)?;
                }
                Ok(())
            }
        }
    }

    fn visit_for(&mut self, node: &Node, frame: &Frame) -> VR {
        let mut loop_frame = self.inner(frame);
        loop_frame.loop_frame = true;
        let test_frame = self.inner(frame);
        let else_frame = self.inner(frame);
        let recursive = node.flag1;
        let body = node.list(2);
        let else_ = node.list(3);
        let mut blocks = Vec::new();
        node.find_all(K::Block, &mut blocks);
        let extended = recursive
            || find_undeclared(body.iter(), &["loop"]).contains("loop")
            || blocks.iter().any(|b| b.flag1);
        if extended {
            self.ar.declare_parameter(loop_frame.sym, "loop");
        }
        self.ar.analyze(loop_frame.sym, node, "body")?;
        if !else_.is_empty() {
            self.ar.analyze(else_frame.sym, node, "else")?;
        }
        if let Some(test) = node.field(4) {
            self.ar.analyze(test_frame.sym, node, "test")?;
            self.enter_frame(&test_frame);
            self.opt(node.field(0), &loop_frame)?;
            self.visit(test, &test_frame)?;
            self.opt(node.field(0), &loop_frame)?;
        }
        let mut names = Vec::new();
        node.find_all(K::Name, &mut names);
        for nm in names {
            if nm.ctx == "store" && nm.s == "loop" {
                return Err(self.fail("Can't assign to special loop variable in for-loop target".into(), nm.lineno));
            }
        }
        self.opt(node.field(0), &loop_frame)?;
        if !recursive {
            self.opt(node.field(1), frame)?;
        }
        self.enter_frame(&loop_frame);
        self.bv(body, &loop_frame)?;
        if !else_.is_empty() {
            self.enter_frame(&else_frame);
            self.bv(else_, &else_frame)?;
        }
        if recursive {
            self.opt(node.field(1), frame)?;
        }
        Ok(())
    }
}

/// `find_undeclared_variables(ast)` incl. the `TemplateAssertionError`s possible there.
pub fn find_undeclared_variables(tpl: &Node, env: &Env<'_>) -> Result<HashSet<String>, JErr> {
    let mut g = Gen { ar: Arena::default(), env, undeclared: HashSet::new(), has_known_extends: false, extends_so_far: 0 };
    let body = tpl.list(0);
    // Collect blocks, duplicate names
    let mut blocks = Vec::new();
    tpl.find_all(K::Block, &mut blocks);
    let mut seen: Vec<&Node> = Vec::new();
    for b in &blocks {
        if seen.iter().any(|s| s.s == b.s) {
            return Err(JErr::compile_assertion(format!("block {} defined twice", str_repr(&b.s)), b.lineno));
        }
        seen.push(b);
    }
    let mut ext = Vec::new();
    tpl.find_all(K::Extends, &mut ext);
    let have_extends = !ext.is_empty();

    let mut frame = g.root_frame();
    if find_undeclared(body.iter(), &["self"]).contains("self") {
        g.ar.declare_parameter(frame.sym, "self");
    }
    g.ar.analyze(frame.sym, tpl, "")?;
    frame.toplevel = true;
    frame.rootlevel = true;
    frame.require_output_check = have_extends && !g.has_known_extends;
    g.enter_frame(&frame);
    g.blockvisit(body, &frame)?;

    for b in seen {
        let mut bf = g.root_frame();
        bf.block_frame = true;
        let und = find_undeclared(b.list(0).iter(), &["self", "super"]);
        if und.contains("self") {
            g.ar.declare_parameter(bf.sym, "self");
        }
        if und.contains("super") {
            g.ar.declare_parameter(bf.sym, "super");
        }
        g.ar.analyze(bf.sym, b, "")?;
        g.enter_frame(&bf);
        g.blockvisit(b.list(0), &bf)?;
    }
    Ok(g.undeclared)
}
