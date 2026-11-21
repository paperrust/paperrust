//! Port of the Jinja2 parser (Jinja2 3.1.6, `jinja2/parser.py::Parser`) incl. the extension
//! `jinja2.ext.loopcontrols` (`break`, `continue`). Error texts 1:1.

use super::JErr;
use super::ast::{F, K, Node};
use super::lexer::{TokenStream, describe_expr, describe_token};
use crate::pyfmt::str_repr;

type R<T> = Result<T, JErr>;

const STATEMENT_KEYWORDS: &[&str] =
    &["for", "if", "block", "extends", "print", "macro", "include", "from", "import", "set", "with", "autoescape"];
const COMPARE_OPS: &[&str] = &["eq", "ne", "lt", "lteq", "gt", "gteq"];

pub struct Parser {
    stream: TokenStream,
    tag_stack: Vec<String>,
    end_token_stack: Vec<Vec<&'static str>>,
}

fn n(node: Node) -> F {
    F::N(Box::new(node))
}
fn opt(node: Option<Node>) -> F {
    match node {
        Some(x) => F::N(Box::new(x)),
        None => F::Nil,
    }
}

impl Parser {
    pub fn new(source: &str) -> R<Self> {
        Ok(Parser { stream: TokenStream::new(source)?, tag_stack: Vec::new(), end_token_stack: Vec::new() })
    }

    fn fail<T>(&self, msg: impl Into<String>, lineno: Option<usize>) -> R<T> {
        Err(JErr::syntax(msg, lineno.unwrap_or(self.stream.current.lineno)))
    }

    fn fail_ut_eof<T>(&self, name: Option<&str>, stack: &[Vec<&'static str>], lineno: Option<usize>) -> R<T> {
        let mut expected: Vec<String> = Vec::new();
        for exprs in stack {
            for e in exprs {
                expected.push(describe_expr(e));
            }
        }
        let currently_looking = stack.last().map(|last| last.iter().map(|e| str_repr(&describe_expr(e))).collect::<Vec<_>>().join(" or "));
        let mut message = vec![match name {
            None => "Unexpected end of template.".to_string(),
            Some(nm) => format!("Encountered unknown tag {}.", str_repr(nm)),
        }];
        if let Some(cl) = currently_looking.filter(|s| !s.is_empty()) {
            if name.is_some_and(|nm| expected.iter().any(|e| e == nm)) {
                message.push(format!("You probably made a nesting mistake. Jinja is expecting this tag, but currently looking for {cl}."));
            } else {
                message.push(format!("Jinja was looking for the following tags: {cl}."));
            }
        }
        if let Some(top) = self.tag_stack.last() {
            message.push(format!("The innermost block that needs to be closed is {}.", str_repr(top)));
        }
        self.fail(message.join(" "), lineno)
    }

    fn fail_eof<T>(&self, end_tokens: Option<&[&'static str]>, lineno: Option<usize>) -> R<T> {
        let mut stack = self.end_token_stack.clone();
        if let Some(e) = end_tokens {
            stack.push(e.to_vec());
        }
        self.fail_ut_eof(None, &stack, lineno)
    }

    fn is_tuple_end(&self, extra: Option<&[&str]>) -> bool {
        let t = self.stream.current.ty;
        if t == "variable_end" || t == "block_end" || t == "rparen" {
            return true;
        }
        // upstream calls `test_any(extra_end_rules)` with the tuple as *one* argument
        // (instead of `*extra_end_rules`); `Token.test(tuple)` is then always false. Replicated.
        let _ = extra;
        false
    }

    fn cur_ty(&self) -> &'static str {
        self.stream.current.ty
    }

    fn parse_statement(&mut self) -> R<Vec<Node>> {
        let token = self.stream.current.clone();
        if token.ty != "name" {
            return self.fail("tag name expected", Some(token.lineno));
        }
        self.tag_stack.push(token.value.clone());
        let v = token.value.as_str();
        let r: R<Vec<Node>> = if STATEMENT_KEYWORDS.contains(&v) {
            match v {
                "for" => self.parse_for().map(|x| vec![x]),
                "if" => self.parse_if().map(|x| vec![x]),
                "block" => self.parse_block().map(|x| vec![x]),
                "extends" => self.parse_extends().map(|x| vec![x]),
                "print" => self.parse_print().map(|x| vec![x]),
                "macro" => self.parse_macro().map(|x| vec![x]),
                "include" => self.parse_include().map(|x| vec![x]),
                "from" => self.parse_from().map(|x| vec![x]),
                "import" => self.parse_import().map(|x| vec![x]),
                "set" => self.parse_set().map(|x| vec![x]),
                "with" => self.parse_with().map(|x| vec![x]),
                _ => self.parse_autoescape().map(|x| vec![x]),
            }
        } else if v == "call" {
            self.parse_call_block().map(|x| vec![x])
        } else if v == "filter" {
            self.parse_filter_block().map(|x| vec![x])
        } else if v == "break" || v == "continue" {
            // jinja2.ext.LoopControlExtension.parse
            let t = self.stream.next()?;
            let k = if t.value == "break" { K::Break } else { K::Continue };
            Ok(vec![Node::new(k, t.lineno, vec![])])
        } else {
            self.tag_stack.pop();
            let stack = self.end_token_stack.clone();
            return self.fail_ut_eof(Some(&token.value), &stack, Some(token.lineno));
        };
        self.tag_stack.pop();
        r
    }

    fn parse_statements(&mut self, end_tokens: &[&'static str], drop_needle: bool) -> R<Vec<Node>> {
        self.stream.skip_if("colon")?;
        self.stream.expect("block_end")?;
        let result = self.subparse(Some(end_tokens))?;
        if self.cur_ty() == "eof" {
            return self.fail_eof(Some(end_tokens), None);
        }
        if drop_needle {
            self.stream.next()?;
        }
        Ok(result)
    }

    fn parse_set(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let target = self.parse_assign_target(true, false, None, true)?;
        if self.stream.skip_if("assign")? {
            let expr = self.parse_tuple(false, true, None, false, false)?;
            return Ok(Node::new(K::Assign, lineno, vec![n(target), n(expr)]));
        }
        let filter_node = self.parse_filter(None, false)?;
        let body = self.parse_statements(&["name:endset"], true)?;
        Ok(Node::new(K::AssignBlock, lineno, vec![n(target), opt(filter_node), F::L(body)]))
    }

    fn parse_for(&mut self) -> R<Node> {
        let lineno = self.stream.expect("name:for")?.lineno;
        let target = self.parse_assign_target(true, false, Some(&["name:in"]), false)?;
        self.stream.expect("name:in")?;
        let iter = self.parse_tuple(false, false, Some(&["name:recursive"]), false, false)?;
        let mut test = None;
        if self.stream.skip_if("name:if")? {
            test = Some(self.parse_expression(true)?);
        }
        let recursive = self.stream.skip_if("name:recursive")?;
        let body = self.parse_statements(&["name:endfor", "name:else"], false)?;
        let else_ = if self.stream.next()?.value == "endfor" { Vec::new() } else { self.parse_statements(&["name:endfor"], true)? };
        let mut node = Node::new(K::For, lineno, vec![n(target), n(iter), F::L(body), F::L(else_), opt(test)]);
        node.flag1 = recursive;
        Ok(node)
    }

    fn parse_if(&mut self) -> R<Node> {
        let lineno = self.stream.expect("name:if")?.lineno;
        // (test, body) of the chain: [if, elif, elif, ...]
        let mut chain: Vec<(usize, Node, Vec<Node>)> = Vec::new();
        let mut cur_lineno = lineno;
        let mut else_ = Vec::new();
        loop {
            let test = self.parse_tuple(false, false, None, false, false)?;
            let body = self.parse_statements(&["name:elif", "name:else", "name:endif"], false)?;
            chain.push((cur_lineno, test, body));
            let token = self.stream.next()?;
            if token.test("name:elif") {
                cur_lineno = self.stream.current.lineno;
                continue;
            } else if token.test("name:else") {
                else_ = self.parse_statements(&["name:endif"], true)?;
            }
            break;
        }
        let mut it = chain.into_iter();
        let (l0, t0, b0) = it.next().ok_or_else(|| JErr::internal("empty if"))?;
        let elifs: Vec<Node> = it.map(|(l, t, b)| Node::new(K::If, l, vec![n(t), F::L(b), F::L(vec![]), F::L(vec![])])).collect();
        Ok(Node::new(K::If, l0, vec![n(t0), F::L(b0), F::L(elifs), F::L(else_)]))
    }

    fn parse_with(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let mut targets = Vec::new();
        let mut values = Vec::new();
        while self.cur_ty() != "block_end" {
            if !targets.is_empty() {
                self.stream.expect("comma")?;
            }
            let mut target = self.parse_assign_target(true, false, None, false)?;
            target.set_ctx("param");
            targets.push(target);
            self.stream.expect("assign")?;
            values.push(self.parse_expression(true)?);
        }
        let body = self.parse_statements(&["name:endwith"], true)?;
        Ok(Node::new(K::With, lineno, vec![F::L(targets), F::L(values), F::L(body)]))
    }

    fn parse_autoescape(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let expr = self.parse_expression(true)?;
        let kw = Node::new(K::Keyword, expr.lineno, vec![n(expr)]).with_s("autoescape");
        let body = self.parse_statements(&["name:endautoescape"], true)?;
        let m = Node::new(K::ScopedEvalContextModifier, lineno, vec![F::L(vec![kw]), F::L(body)]);
        Ok(Node::new(K::Scope, lineno, vec![F::L(vec![m])]))
    }

    fn parse_block(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let name = self.stream.expect("name")?.value;
        let scoped = self.stream.skip_if("name:scoped")?;
        let required = self.stream.skip_if("name:required")?;
        if self.cur_ty() == "sub" {
            return self.fail(
                "Block names in Jinja have to be valid Python identifiers and may not contain hyphens, use an underscore instead.",
                None,
            );
        }
        let body = self.parse_statements(&["name:endblock"], true)?;
        if required {
            for b in &body {
                let ok = b.k == K::Output
                    && b.list(0).iter().all(|o| o.k == K::TemplateData && !o.s.is_empty() && o.s.chars().all(super::lexer::is_py_space));
                if !ok {
                    return self.fail("Required blocks can only contain comments or whitespace", None);
                }
            }
        }
        self.stream.skip_if(&format!("name:{name}"))?;
        let mut node = Node::new(K::Block, lineno, vec![F::L(body)]).with_s(name);
        node.flag1 = scoped;
        node.flag2 = required;
        Ok(node)
    }

    fn parse_extends(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let t = self.parse_expression(true)?;
        Ok(Node::new(K::Extends, lineno, vec![n(t)]))
    }

    fn parse_import_context(&mut self, node: &mut Node, default: bool) -> R<()> {
        if self.stream.current.test_any(&["name:with", "name:without"]) && self.stream.look()?.test("name:context") {
            node.flag1 = self.stream.next()?.value == "with";
            self.stream.skip(1)?;
        } else {
            node.flag1 = default;
        }
        Ok(())
    }

    fn parse_include(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let t = self.parse_expression(true)?;
        let mut node = Node::new(K::Include, lineno, vec![n(t)]);
        if self.stream.current.test("name:ignore") && self.stream.look()?.test("name:missing") {
            node.flag2 = true;
            self.stream.skip(2)?;
        }
        self.parse_import_context(&mut node, true)?;
        Ok(node)
    }

    fn parse_import(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let t = self.parse_expression(true)?;
        self.stream.expect("name:as")?;
        let target = self.parse_assign_target(true, true, None, false)?.s;
        let mut node = Node::new(K::Import, lineno, vec![n(t)]).with_s(target);
        self.parse_import_context(&mut node, false)?;
        Ok(node)
    }

    fn parse_from(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let t = self.parse_expression(true)?;
        self.stream.expect("name:import")?;
        let mut node = Node::new(K::FromImport, lineno, vec![n(t)]);
        let mut has_ctx = false;
        loop {
            if !node.names.is_empty() {
                self.stream.expect("comma")?;
            }
            if self.cur_ty() == "name" {
                if self.parse_from_context(&mut node, &mut has_ctx)? {
                    break;
                }
                let target = self.parse_assign_target(true, true, None, false)?;
                if target.s.starts_with('_') {
                    return Err(JErr::parse_assertion("names starting with an underline can not be imported", target.lineno));
                }
                if self.stream.skip_if("name:as")? {
                    let alias = self.parse_assign_target(true, true, None, false)?;
                    node.names.push((target.s, Some(alias.s)));
                } else {
                    node.names.push((target.s, None));
                }
                if self.parse_from_context(&mut node, &mut has_ctx)? || self.cur_ty() != "comma" {
                    break;
                }
            } else {
                self.stream.expect("name")?;
            }
        }
        if !has_ctx {
            node.flag1 = false;
        }
        Ok(node)
    }

    fn parse_from_context(&mut self, node: &mut Node, has_ctx: &mut bool) -> R<bool> {
        let v = self.stream.current.value.as_str();
        if (v == "with" || v == "without") && self.stream.look()?.test("name:context") {
            node.flag1 = self.stream.next()?.value == "with";
            self.stream.skip(1)?;
            *has_ctx = true;
            return Ok(true);
        }
        Ok(false)
    }

    /// Returns (args, defaults).
    fn parse_signature(&mut self) -> R<(Vec<Node>, Vec<Node>)> {
        let mut args = Vec::new();
        let mut defaults = Vec::new();
        self.stream.expect("lparen")?;
        while self.cur_ty() != "rparen" {
            if !args.is_empty() {
                self.stream.expect("comma")?;
            }
            let mut arg = self.parse_assign_target(true, true, None, false)?;
            arg.set_ctx("param");
            if self.stream.skip_if("assign")? {
                defaults.push(self.parse_expression(true)?);
            } else if !defaults.is_empty() {
                return self.fail("non-default argument follows default argument", None);
            }
            args.push(arg);
        }
        self.stream.expect("rparen")?;
        Ok((args, defaults))
    }

    fn parse_call_block(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let (args, defaults) = if self.cur_ty() == "lparen" { self.parse_signature()? } else { (Vec::new(), Vec::new()) };
        let call_node = self.parse_expression(true)?;
        if call_node.k != K::Call {
            return self.fail("expected call", Some(lineno));
        }
        let body = self.parse_statements(&["name:endcall"], true)?;
        Ok(Node::new(K::CallBlock, lineno, vec![n(call_node), F::L(args), F::L(defaults), F::L(body)]))
    }

    fn parse_filter_block(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let filter = self.parse_filter(None, true)?;
        let body = self.parse_statements(&["name:endfilter"], true)?;
        Ok(Node::new(K::FilterBlock, lineno, vec![F::L(body), opt(filter)]))
    }

    fn parse_macro(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let name = self.parse_assign_target(true, true, None, false)?.s;
        let (args, defaults) = self.parse_signature()?;
        let body = self.parse_statements(&["name:endmacro"], true)?;
        Ok(Node::new(K::Macro, lineno, vec![F::L(args), F::L(defaults), F::L(body)]).with_s(name))
    }

    fn parse_print(&mut self) -> R<Node> {
        let lineno = self.stream.next()?.lineno;
        let mut nodes = Vec::new();
        while self.cur_ty() != "block_end" {
            if !nodes.is_empty() {
                self.stream.expect("comma")?;
            }
            nodes.push(self.parse_expression(true)?);
        }
        Ok(Node::new(K::Output, lineno, vec![F::L(nodes)]))
    }

    fn parse_assign_target(
        &mut self,
        with_tuple: bool,
        name_only: bool,
        extra_end_rules: Option<&[&str]>,
        with_namespace: bool,
    ) -> R<Node> {
        let target = if name_only {
            let token = self.stream.expect("name")?;
            Node::new(K::Name, token.lineno, vec![]).with_s(token.value).with_ctx("store")
        } else {
            let mut t = if with_tuple {
                self.parse_tuple(true, true, extra_end_rules, false, with_namespace)?
            } else {
                self.parse_primary(with_namespace)?
            };
            t.set_ctx("store");
            t
        };
        if !target.can_assign() {
            return self.fail(format!("can't assign to {}", str_repr(&target.k.py_name())), Some(target.lineno));
        }
        Ok(target)
    }

    fn parse_expression(&mut self, with_condexpr: bool) -> R<Node> {
        if with_condexpr { self.parse_condexpr() } else { self.parse_or() }
    }

    fn parse_condexpr(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut expr1 = self.parse_or()?;
        while self.stream.skip_if("name:if")? {
            let expr2 = self.parse_or()?;
            let expr3 = if self.stream.skip_if("name:else")? { Some(self.parse_condexpr()?) } else { None };
            expr1 = Node::new(K::CondExpr, lineno, vec![n(expr2), n(expr1), opt(expr3)]);
            lineno = self.stream.current.lineno;
        }
        Ok(expr1)
    }

    fn parse_or(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut left = self.parse_and()?;
        while self.stream.skip_if("name:or")? {
            let right = self.parse_and()?;
            left = Node::new(K::Or, lineno, vec![n(left), n(right)]);
            lineno = self.stream.current.lineno;
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut left = self.parse_not()?;
        while self.stream.skip_if("name:and")? {
            let right = self.parse_not()?;
            left = Node::new(K::And, lineno, vec![n(left), n(right)]);
            lineno = self.stream.current.lineno;
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> R<Node> {
        if self.stream.current.test("name:not") {
            let lineno = self.stream.next()?.lineno;
            let inner = self.parse_not()?;
            return Ok(Node::new(K::Not, lineno, vec![n(inner)]));
        }
        self.parse_compare()
    }

    fn parse_compare(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let expr = self.parse_math1()?;
        let mut ops = Vec::new();
        loop {
            let tt = self.cur_ty();
            if COMPARE_OPS.contains(&tt) {
                self.stream.next()?;
                let e = self.parse_math1()?;
                ops.push(Node::new(K::Operand, e.lineno, vec![n(e)]).with_s(tt));
            } else if self.stream.skip_if("name:in")? {
                let e = self.parse_math1()?;
                ops.push(Node::new(K::Operand, e.lineno, vec![n(e)]).with_s("in"));
            } else if self.stream.current.test("name:not") && self.stream.look()?.test("name:in") {
                self.stream.skip(2)?;
                let e = self.parse_math1()?;
                ops.push(Node::new(K::Operand, e.lineno, vec![n(e)]).with_s("notin"));
            } else {
                break;
            }
            lineno = self.stream.current.lineno;
        }
        if ops.is_empty() {
            return Ok(expr);
        }
        Ok(Node::new(K::Compare, lineno, vec![n(expr), F::L(ops)]))
    }

    fn parse_math1(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut left = self.parse_concat()?;
        while matches!(self.cur_ty(), "add" | "sub") {
            let k = if self.cur_ty() == "add" { K::Add } else { K::Sub };
            self.stream.next()?;
            let right = self.parse_concat()?;
            left = Node::new(k, lineno, vec![n(left), n(right)]);
            lineno = self.stream.current.lineno;
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> R<Node> {
        let lineno = self.stream.current.lineno;
        let mut args = vec![self.parse_math2()?];
        while self.cur_ty() == "tilde" {
            self.stream.next()?;
            args.push(self.parse_math2()?);
        }
        if args.len() == 1 {
            return Ok(args.remove(0));
        }
        Ok(Node::new(K::Concat, lineno, vec![F::L(args)]))
    }

    fn parse_math2(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut left = self.parse_pow()?;
        while matches!(self.cur_ty(), "mul" | "div" | "floordiv" | "mod") {
            let k = match self.cur_ty() {
                "mul" => K::Mul,
                "div" => K::Div,
                "floordiv" => K::FloorDiv,
                _ => K::Mod,
            };
            self.stream.next()?;
            let right = self.parse_pow()?;
            left = Node::new(k, lineno, vec![n(left), n(right)]);
            lineno = self.stream.current.lineno;
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut left = self.parse_unary(true)?;
        while self.cur_ty() == "pow" {
            self.stream.next()?;
            let right = self.parse_unary(true)?;
            left = Node::new(K::Pow, lineno, vec![n(left), n(right)]);
            lineno = self.stream.current.lineno;
        }
        Ok(left)
    }

    fn parse_unary(&mut self, with_filter: bool) -> R<Node> {
        let tt = self.cur_ty();
        let lineno = self.stream.current.lineno;
        let mut node = if tt == "sub" {
            self.stream.next()?;
            let inner = self.parse_unary(false)?;
            Node::new(K::Neg, lineno, vec![n(inner)])
        } else if tt == "add" {
            self.stream.next()?;
            let inner = self.parse_unary(false)?;
            Node::new(K::Pos, lineno, vec![n(inner)])
        } else {
            self.parse_primary(false)?
        };
        node = self.parse_postfix(node)?;
        if with_filter {
            node = self.parse_filter_expr(node)?;
        }
        Ok(node)
    }

    fn parse_primary(&mut self, with_namespace: bool) -> R<Node> {
        let token = self.stream.current.clone();
        match token.ty {
            "name" => {
                self.stream.next()?;
                let v = token.value.as_str();
                if matches!(v, "true" | "false" | "True" | "False" | "none" | "None") {
                    Ok(Node::new(K::Const, token.lineno, vec![]))
                } else if with_namespace && self.cur_ty() == "dot" {
                    self.stream.next()?;
                    let attr = self.stream.expect("name")?;
                    let mut node = Node::new(K::NSRef, token.lineno, vec![]).with_s(token.value.clone());
                    node.s2 = attr.value;
                    Ok(node)
                } else {
                    Ok(Node::new(K::Name, token.lineno, vec![]).with_s(token.value.clone()).with_ctx("load"))
                }
            }
            "string" => {
                self.stream.next()?;
                while self.cur_ty() == "string" {
                    self.stream.next()?;
                }
                Ok(Node::new(K::Const, token.lineno, vec![]))
            }
            "integer" | "float" => {
                self.stream.next()?;
                Ok(Node::new(K::Const, token.lineno, vec![]))
            }
            "lparen" => {
                self.stream.next()?;
                let node = self.parse_tuple(false, true, None, true, false)?;
                self.stream.expect("rparen")?;
                Ok(node)
            }
            "lbracket" => self.parse_list(),
            "lbrace" => self.parse_dict(),
            _ => self.fail(format!("unexpected {}", str_repr(&describe_token(&token))), Some(token.lineno)),
        }
    }

    fn parse_tuple(
        &mut self,
        simplified: bool,
        with_condexpr: bool,
        extra_end_rules: Option<&[&str]>,
        explicit_parentheses: bool,
        with_namespace: bool,
    ) -> R<Node> {
        let mut lineno = self.stream.current.lineno;
        let mut args: Vec<Node> = Vec::new();
        let mut is_tuple = false;
        loop {
            if !args.is_empty() {
                self.stream.expect("comma")?;
            }
            if self.is_tuple_end(extra_end_rules) {
                break;
            }
            let a = if simplified { self.parse_primary(with_namespace)? } else { self.parse_expression(with_condexpr)? };
            args.push(a);
            if self.cur_ty() == "comma" {
                is_tuple = true;
            } else {
                break;
            }
            lineno = self.stream.current.lineno;
        }
        if !is_tuple {
            if !args.is_empty() {
                return Ok(args.remove(0));
            }
            if !explicit_parentheses {
                return self.fail(format!("Expected an expression, got {}", str_repr(&describe_token(&self.stream.current))), None);
            }
        }
        Ok(Node::new(K::Tuple, lineno, vec![F::L(args)]).with_ctx("load"))
    }

    fn parse_list(&mut self) -> R<Node> {
        let token = self.stream.expect("lbracket")?;
        let mut items = Vec::new();
        while self.cur_ty() != "rbracket" {
            if !items.is_empty() {
                self.stream.expect("comma")?;
            }
            if self.cur_ty() == "rbracket" {
                break;
            }
            items.push(self.parse_expression(true)?);
        }
        self.stream.expect("rbracket")?;
        Ok(Node::new(K::List, token.lineno, vec![F::L(items)]))
    }

    fn parse_dict(&mut self) -> R<Node> {
        let token = self.stream.expect("lbrace")?;
        let mut items = Vec::new();
        while self.cur_ty() != "rbrace" {
            if !items.is_empty() {
                self.stream.expect("comma")?;
            }
            if self.cur_ty() == "rbrace" {
                break;
            }
            let key = self.parse_expression(true)?;
            self.stream.expect("colon")?;
            let value = self.parse_expression(true)?;
            let l = key.lineno;
            items.push(Node::new(K::Pair, l, vec![n(key), n(value)]));
        }
        self.stream.expect("rbrace")?;
        Ok(Node::new(K::Dict, token.lineno, vec![F::L(items)]))
    }

    fn parse_postfix(&mut self, mut node: Node) -> R<Node> {
        loop {
            let tt = self.cur_ty();
            if tt == "dot" || tt == "lbracket" {
                node = self.parse_subscript(node)?;
            } else if tt == "lparen" {
                node = self.parse_call(node)?;
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_filter_expr(&mut self, mut node: Node) -> R<Node> {
        loop {
            let tt = self.cur_ty();
            if tt == "pipe" {
                node = self.parse_filter(Some(node), false)?.ok_or_else(|| JErr::internal("filter"))?;
            } else if tt == "name" && self.stream.current.value == "is" {
                node = self.parse_test(node)?;
            } else if tt == "lparen" {
                node = self.parse_call(node)?;
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_subscript(&mut self, node: Node) -> R<Node> {
        let token = self.stream.next()?;
        if token.ty == "dot" {
            let attr_token = self.stream.current.clone();
            self.stream.next()?;
            if attr_token.ty == "name" {
                return Ok(Node::new(K::Getattr, token.lineno, vec![n(node)]).with_s(attr_token.value).with_ctx("load"));
            } else if attr_token.ty != "integer" {
                return self.fail("expected name or number", Some(attr_token.lineno));
            }
            let arg = Node::new(K::Const, attr_token.lineno, vec![]);
            return Ok(Node::new(K::Getitem, token.lineno, vec![n(node), n(arg)]).with_ctx("load"));
        }
        if token.ty == "lbracket" {
            let mut args = Vec::new();
            while self.cur_ty() != "rbracket" {
                if !args.is_empty() {
                    self.stream.expect("comma")?;
                }
                args.push(self.parse_subscribed()?);
            }
            self.stream.expect("rbracket")?;
            let arg = if args.len() == 1 { args.remove(0) } else { Node::new(K::Tuple, token.lineno, vec![F::L(args)]).with_ctx("load") };
            return Ok(Node::new(K::Getitem, token.lineno, vec![n(node), n(arg)]).with_ctx("load"));
        }
        self.fail("expected subscript expression", Some(token.lineno))
    }

    fn parse_subscribed(&mut self) -> R<Node> {
        let lineno = self.stream.current.lineno;
        let mut args: Vec<Option<Node>>;
        if self.cur_ty() == "colon" {
            self.stream.next()?;
            args = vec![None];
        } else {
            let node = self.parse_expression(true)?;
            if self.cur_ty() != "colon" {
                return Ok(node);
            }
            self.stream.next()?;
            args = vec![Some(node)];
        }
        if self.cur_ty() == "colon" {
            args.push(None);
        } else if !matches!(self.cur_ty(), "rbracket" | "comma") {
            args.push(Some(self.parse_expression(true)?));
        } else {
            args.push(None);
        }
        if self.cur_ty() == "colon" {
            self.stream.next()?;
            if !matches!(self.cur_ty(), "rbracket" | "comma") {
                args.push(Some(self.parse_expression(true)?));
            } else {
                args.push(None);
            }
        } else {
            args.push(None);
        }
        Ok(Node::new(K::Slice, lineno, args.into_iter().map(opt).collect()))
    }

    /// (args, kwargs, dyn_args, dyn_kwargs)
    #[allow(clippy::type_complexity)]
    fn parse_call_args(&mut self) -> R<(Vec<Node>, Vec<Node>, Option<Node>, Option<Node>)> {
        let token = self.stream.expect("lparen")?;
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        let mut dyn_args: Option<Node> = None;
        let mut dyn_kwargs: Option<Node> = None;
        let mut require_comma = false;
        let ensure = |ok: bool| -> R<()> {
            if ok { Ok(()) } else { Err(JErr::syntax("invalid syntax for function call expression", token.lineno)) }
        };
        while self.cur_ty() != "rparen" {
            if require_comma {
                self.stream.expect("comma")?;
                if self.cur_ty() == "rparen" {
                    break;
                }
            }
            if self.cur_ty() == "mul" {
                ensure(dyn_args.is_none() && dyn_kwargs.is_none())?;
                self.stream.next()?;
                dyn_args = Some(self.parse_expression(true)?);
            } else if self.cur_ty() == "pow" {
                ensure(dyn_kwargs.is_none())?;
                self.stream.next()?;
                dyn_kwargs = Some(self.parse_expression(true)?);
            } else if self.cur_ty() == "name" && self.stream.look()?.ty == "assign" {
                ensure(dyn_kwargs.is_none())?;
                let key = self.stream.current.value.clone();
                self.stream.skip(2)?;
                let value = self.parse_expression(true)?;
                let l = value.lineno;
                kwargs.push(Node::new(K::Keyword, l, vec![n(value)]).with_s(key));
            } else {
                ensure(dyn_args.is_none() && dyn_kwargs.is_none() && kwargs.is_empty())?;
                args.push(self.parse_expression(true)?);
            }
            require_comma = true;
        }
        self.stream.expect("rparen")?;
        Ok((args, kwargs, dyn_args, dyn_kwargs))
    }

    fn parse_call(&mut self, node: Node) -> R<Node> {
        let lineno = self.stream.current.lineno;
        let (args, kwargs, da, dk) = self.parse_call_args()?;
        Ok(Node::new(K::Call, lineno, vec![n(node), F::L(args), F::L(kwargs), opt(da), opt(dk)]))
    }

    fn parse_filter(&mut self, mut node: Option<Node>, mut start_inline: bool) -> R<Option<Node>> {
        while self.cur_ty() == "pipe" || start_inline {
            if !start_inline {
                self.stream.next()?;
            }
            let token = self.stream.expect("name")?;
            let mut name = token.value.clone();
            while self.cur_ty() == "dot" {
                self.stream.next()?;
                name.push('.');
                name.push_str(&self.stream.expect("name")?.value);
            }
            let (args, kwargs, da, dk) =
                if self.cur_ty() == "lparen" { self.parse_call_args()? } else { (Vec::new(), Vec::new(), None, None) };
            node = Some(Node::new(K::Filter, token.lineno, vec![opt(node), F::L(args), F::L(kwargs), opt(da), opt(dk)]).with_s(name));
            start_inline = false;
        }
        Ok(node)
    }

    fn parse_test(&mut self, node: Node) -> R<Node> {
        let token = self.stream.next()?;
        let negated = if self.stream.current.test("name:not") {
            self.stream.next()?;
            true
        } else {
            false
        };
        let mut name = self.stream.expect("name")?.value;
        while self.cur_ty() == "dot" {
            self.stream.next()?;
            name.push('.');
            name.push_str(&self.stream.expect("name")?.value);
        }
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        let mut da = None;
        let mut dk = None;
        if self.cur_ty() == "lparen" {
            (args, kwargs, da, dk) = self.parse_call_args()?;
        } else if matches!(self.cur_ty(), "name" | "string" | "integer" | "float" | "lparen" | "lbracket" | "lbrace")
            && !self.stream.current.test_any(&["name:else", "name:or", "name:and"])
        {
            if self.stream.current.test("name:is") {
                return self.fail("You cannot chain multiple tests with is", None);
            }
            let arg = self.parse_primary(false)?;
            let arg = self.parse_postfix(arg)?;
            args = vec![arg];
        }
        let t = Node::new(K::Test, token.lineno, vec![n(node), F::L(args), F::L(kwargs), opt(da), opt(dk)]).with_s(name);
        if negated {
            return Ok(Node::new(K::Not, token.lineno, vec![n(t)]));
        }
        Ok(t)
    }

    fn subparse(&mut self, end_tokens: Option<&[&'static str]>) -> R<Vec<Node>> {
        if let Some(e) = end_tokens {
            self.end_token_stack.push(e.to_vec());
        }
        let r = self.subparse_inner(end_tokens);
        if end_tokens.is_some() {
            self.end_token_stack.pop();
        }
        r
    }

    fn subparse_inner(&mut self, end_tokens: Option<&[&'static str]>) -> R<Vec<Node>> {
        let mut body: Vec<Node> = Vec::new();
        let mut data_buffer: Vec<Node> = Vec::new();
        fn flush(body: &mut Vec<Node>, buf: &mut Vec<Node>) {
            if !buf.is_empty() {
                let lineno = buf[0].lineno;
                body.push(Node::new(K::Output, lineno, vec![F::L(std::mem::take(buf))]));
            }
        }
        while self.stream.has_more() {
            let token = self.stream.current.clone();
            match token.ty {
                "data" => {
                    if !token.value.is_empty() {
                        data_buffer.push(Node::new(K::TemplateData, token.lineno, vec![]).with_s(token.value));
                    }
                    self.stream.next()?;
                }
                "variable_begin" => {
                    self.stream.next()?;
                    data_buffer.push(self.parse_tuple(false, true, None, false, false)?);
                    self.stream.expect("variable_end")?;
                }
                "block_begin" => {
                    flush(&mut body, &mut data_buffer);
                    self.stream.next()?;
                    if let Some(e) = end_tokens
                        && self.stream.current.test_any(e)
                    {
                        return Ok(body);
                    }
                    let rv = self.parse_statement()?;
                    body.extend(rv);
                    self.stream.expect("block_end")?;
                }
                _ => return Err(JErr::internal("internal parsing error")),
            }
        }
        flush(&mut body, &mut data_buffer);
        Ok(body)
    }

    pub fn parse(mut self) -> R<Node> {
        let body = self.subparse(None)?;
        Ok(Node::new(K::Template, 1, vec![F::L(body)]))
    }
}
