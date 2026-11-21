//! Port of the Jinja2 lexer (Jinja2 3.1.6, `jinja2/lexer.py`: `Lexer.tokeniter`, `Lexer.wrap`,
//! `TokenStream`) for the paperless environment (`documents/templating/environment.py`):
//! default delimiters, `trim_blocks=True`, `lstrip_blocks=True`, `keep_trailing_newline=False`,
//! no line statements/comments. Error texts 1:1.

use super::JErr;
use crate::pyfmt::str_repr;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub lineno: usize,
    pub ty: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Root,
    Block,
    Variable,
    Comment,
    Raw,
}

pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Lexer as a generator: tokens are only produced when fetched (like in Python) so that the
/// error order parser ↔ lexer stays the same.
pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    lineno: usize,
    stack: Vec<St>,
    balancing: Vec<char>,
    newlines_stripped: usize,
    line_starting: bool,
    queue: VecDeque<Result<(usize, &'static str, String), JErr>>,
    done: bool,
}

const OPS2: &[(&str, &str)] = &[("//", "floordiv"), ("**", "pow"), ("==", "eq"), ("!=", "ne"), (">=", "gteq"), ("<=", "lteq")];
const OPS1: &[(char, &str)] = &[
    ('+', "add"),
    ('-', "sub"),
    ('/', "div"),
    ('*', "mul"),
    ('%', "mod"),
    ('~', "tilde"),
    ('[', "lbracket"),
    (']', "rbracket"),
    ('(', "lparen"),
    (')', "rparen"),
    ('{', "lbrace"),
    ('}', "rbrace"),
    ('>', "gt"),
    ('<', "lt"),
    ('=', "assign"),
    ('.', "dot"),
    (':', "colon"),
    ('|', "pipe"),
    (',', "comma"),
    (';', "semicolon"),
];

/// Reverse of the operator table (`reverse_operators`).
pub fn op_symbol(ty: &str) -> Option<&'static str> {
    OPS2.iter().find(|(_, t)| *t == ty).map(|(s, _)| *s).or_else(|| {
        const S1: &[&str] = &["+", "-", "/", "*", "%", "~", "[", "]", "(", ")", "{", "}", ">", "<", "=", ".", ":", "|", ",", ";"];
        OPS1.iter().position(|(_, t)| *t == ty).map(|i| S1[i])
    })
}

fn count_nl(s: &[char]) -> usize {
    s.iter().filter(|c| **c == '\n').count()
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        // newline_re.split(source)[::2], remove last empty line, join with "\n"
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut it = source.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\r' => {
                    if it.peek() == Some(&'\n') {
                        it.next();
                    }
                    lines.push(std::mem::take(&mut cur));
                }
                '\n' => lines.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        lines.push(cur);
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let src: Vec<char> = lines.join("\n").chars().collect();
        Lexer {
            src,
            pos: 0,
            lineno: 1,
            stack: vec![St::Root],
            balancing: Vec::new(),
            newlines_stripped: 0,
            line_starting: true,
            queue: VecDeque::new(),
            done: false,
        }
    }

    fn starts(&self, at: usize, lit: &str) -> bool {
        lit.chars().enumerate().all(|(k, c)| self.src.get(at + k) == Some(&c))
    }

    fn skip_ws(&self, mut at: usize) -> usize {
        while at < self.src.len() && is_py_space(self.src[at]) {
            at += 1;
        }
        at
    }

    fn s(&self, a: usize, b: usize) -> String {
        self.src[a..b].iter().collect()
    }

    /// Next token (after `wrap`), `Ok(None)` at the end.
    pub fn next_token(&mut self) -> Result<Option<Token>, JErr> {
        loop {
            if let Some(item) = self.queue.pop_front() {
                let (lineno, ty, value) = item?;
                match ty {
                    "comment_begin" | "comment" | "comment_end" | "whitespace" | "raw_begin" | "raw_end" => {
                        continue;
                    }
                    "name" => {
                        if !is_identifier(&value) {
                            return Err(JErr::syntax("Invalid character in identifier", lineno));
                        }
                    }
                    "string" => {
                        let inner: String = {
                            let v: Vec<char> = value.chars().collect();
                            v[1..v.len() - 1].iter().collect()
                        };
                        if let Err(m) = check_unicode_escape(&inner) {
                            return Err(JErr::syntax(m, lineno));
                        }
                    }
                    _ => {}
                }
                return Ok(Some(Token { lineno, ty, value }));
            }
            if self.done {
                return Ok(None);
            }
            if let Err(e) = self.step() {
                self.queue.push_back(Err(e));
                self.done = true;
            }
        }
    }

    fn push(&mut self, ty: &'static str, data: String) {
        self.queue.push_back(Ok((self.lineno, ty, data)));
    }

    fn step(&mut self) -> Result<(), JErr> {
        let state = *self.stack.last().unwrap_or(&St::Root);
        match state {
            St::Root => self.step_root(),
            St::Block | St::Variable => self.step_tag(state),
            St::Comment => self.step_comment(),
            St::Raw => self.step_raw(),
        }
    }

    /// Optionales `-`/`+` an Position `at`.
    fn sign_at(&self, at: usize) -> (char, usize) {
        match self.src.get(at) {
            Some('-') => ('-', at + 1),
            Some('+') => ('+', at + 1),
            _ => ('\0', at),
        }
    }

    /// `(?:\+%}|\-%}\s*|%}\n?)` at `at` → end.
    fn block_end_at(&self, at: usize) -> Option<usize> {
        if self.starts(at, "+%}") {
            Some(at + 3)
        } else if self.starts(at, "-%}") {
            Some(self.skip_ws(at + 3))
        } else if self.starts(at, "%}") {
            Some(if self.src.get(at + 2) == Some(&'\n') { at + 3 } else { at + 2 })
        } else {
            None
        }
    }

    fn try_root_tag(&self, s: usize) -> Option<(St, &'static str, char, usize)> {
        // raw_begin
        if self.starts(s, "{%") {
            let (sign, p) = self.sign_at(s + 2);
            let p = self.skip_ws(p);
            if self.starts(p, "raw") {
                let p = self.skip_ws(p + 3);
                if self.starts(p, "-%}") {
                    return Some((St::Raw, "raw_begin", sign, self.skip_ws(p + 3)));
                } else if self.starts(p, "%}") {
                    return Some((St::Raw, "raw_begin", sign, p + 2));
                }
            }
        }
        if self.starts(s, "{{") {
            let (sign, p) = self.sign_at(s + 2);
            return Some((St::Variable, "variable_begin", sign, p));
        }
        if self.starts(s, "{#") {
            let (sign, p) = self.sign_at(s + 2);
            return Some((St::Comment, "comment_begin", sign, p));
        }
        if self.starts(s, "{%") {
            let (sign, p) = self.sign_at(s + 2);
            return Some((St::Block, "block_begin", sign, p));
        }
        None
    }

    /// OptionalLStrip handling of the text before a tag.
    fn lstrip_text(&mut self, mut text: Vec<char>, sign: char, is_variable: bool) -> Vec<char> {
        if sign == '-' {
            let mut end = text.len();
            while end > 0 && is_py_space(text[end - 1]) {
                end -= 1;
            }
            self.newlines_stripped = count_nl(&text[end..]);
            text.truncate(end);
        } else if sign != '+' && !is_variable {
            let l_pos = text.iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
            if (l_pos > 0 || self.line_starting) && l_pos < text.len() && text[l_pos..].iter().all(|c| is_py_space(*c)) {
                text.truncate(l_pos);
            }
        }
        text
    }

    fn step_root(&mut self) -> Result<(), JErr> {
        let len = self.src.len();
        let mut found = None;
        for s in self.pos..len {
            if let Some(t) = self.try_root_tag(s) {
                found = Some((s, t));
                break;
            }
        }
        match found {
            Some((s, (st, key, sign, end))) => {
                let text: Vec<char> = self.src[self.pos..s].to_vec();
                let text = self.lstrip_text(text, sign, st == St::Variable);
                let data: String = text.iter().collect();
                if !data.is_empty() {
                    self.push("data", data);
                }
                self.lineno += count_nl(&text) + self.newlines_stripped;
                self.newlines_stripped = 0;
                let tag = self.s(s, end);
                self.push(key, tag.clone());
                self.lineno += tag.matches('\n').count();
                self.line_starting = self.src.get(end.wrapping_sub(1)) == Some(&'\n') && end > self.pos;
                self.pos = end;
                self.stack.push(st);
                Ok(())
            }
            None => {
                if self.pos < len {
                    let data = self.s(self.pos, len);
                    self.push("data", data.clone());
                    self.lineno += data.matches('\n').count();
                    self.line_starting = data.ends_with('\n');
                    self.pos = len;
                } else {
                    self.done = true;
                }
                Ok(())
            }
        }
    }

    fn finish_simple(&mut self, ty: &'static str, start: usize, end: usize) {
        let data = self.s(start, end);
        self.push(ty, data.clone());
        self.lineno += data.matches('\n').count();
        self.line_starting = data.ends_with('\n');
        self.pos = end;
    }

    fn step_tag(&mut self, st: St) -> Result<(), JErr> {
        let pos = self.pos;
        let len = self.src.len();
        // End rule (only with balanced brackets)
        if self.balancing.is_empty() {
            let end = if st == St::Block {
                self.block_end_at(pos)
            } else if self.starts(pos, "-}}") {
                Some(self.skip_ws(pos + 3))
            } else if self.starts(pos, "}}") {
                Some(pos + 2)
            } else {
                None
            };
            if let Some(end) = end {
                let ty = if st == St::Block { "block_end" } else { "variable_end" };
                self.finish_simple(ty, pos, end);
                self.stack.pop();
                return Ok(());
            }
        }
        if pos >= len {
            self.done = true;
            return Ok(());
        }
        // whitespace
        let ws = self.skip_ws(pos);
        if ws > pos {
            self.finish_simple("whitespace", pos, ws);
            return Ok(());
        }
        if let Some(end) = self.match_float(pos) {
            self.finish_simple("float", pos, end);
            return Ok(());
        }
        if let Some(end) = self.match_integer(pos) {
            self.finish_simple("integer", pos, end);
            return Ok(());
        }
        let mut p = pos;
        while p < len && is_word(self.src[p]) {
            p += 1;
        }
        if p > pos {
            self.finish_simple("name", pos, p);
            return Ok(());
        }
        if let Some(end) = self.match_string(pos) {
            self.finish_simple("string", pos, end);
            return Ok(());
        }
        // Operators
        let mut op: Option<(&'static str, usize)> = None;
        for (s, t) in OPS2 {
            if self.starts(pos, s) {
                op = Some((t, pos + 2));
                break;
            }
        }
        if op.is_none()
            && let Some((_, t)) = OPS1.iter().find(|(c, _)| self.src[pos] == *c)
        {
            op = Some((t, pos + 1));
        }
        if let Some((ty, end)) = op {
            let c = self.src[pos];
            match c {
                '{' => self.balancing.push('}'),
                '(' => self.balancing.push(')'),
                '[' => self.balancing.push(']'),
                '}' | ')' | ']' if end == pos + 1 => match self.balancing.pop() {
                    None => return Err(JErr::syntax(format!("unexpected '{c}'"), self.lineno)),
                    Some(exp) if exp != c => {
                        return Err(JErr::syntax(format!("unexpected '{c}', expected '{exp}'"), self.lineno));
                    }
                    Some(_) => {}
                },
                _ => {}
            }
            self.finish_simple(ty, pos, end);
            // Operator tokens carry the operator type directly
            return Ok(());
        }
        Err(JErr::syntax(format!("unexpected char {} at {}", str_repr(&self.src[pos].to_string()), pos), self.lineno))
    }

    /// `(\d+_)*\d+` from `at` → end
    fn digit_group(&self, at: usize) -> Option<usize> {
        let len = self.src.len();
        let mut p = at;
        if p >= len || !self.src[p].is_ascii_digit() {
            return None;
        }
        loop {
            while p < len && self.src[p].is_ascii_digit() {
                p += 1;
            }
            if p + 1 < len && self.src[p] == '_' && self.src[p + 1].is_ascii_digit() {
                p += 1;
                continue;
            }
            return Some(p);
        }
    }

    fn exponent(&self, at: usize) -> Option<usize> {
        if !matches!(self.src.get(at), Some('e') | Some('E')) {
            return None;
        }
        let mut p = at + 1;
        if matches!(self.src.get(p), Some('+') | Some('-')) {
            p += 1;
        }
        self.digit_group(p)
    }

    fn match_float(&self, pos: usize) -> Option<usize> {
        if pos > 0 && self.src[pos - 1] == '.' {
            return None;
        }
        let p = self.digit_group(pos)?;
        // (\.(\d+_)*\d+)? e[+\-]?(\d+_)*\d+
        if self.src.get(p) == Some(&'.')
            && let Some(q) = self.digit_group(p + 1)
            && let Some(e) = self.exponent(q)
        {
            return Some(e);
        }
        if let Some(e) = self.exponent(p) {
            return Some(e);
        }
        if self.src.get(p) == Some(&'.')
            && let Some(q) = self.digit_group(p + 1)
        {
            return Some(q);
        }
        None
    }

    fn match_integer(&self, pos: usize) -> Option<usize> {
        let len = self.src.len();
        let prefixed = |p: char, ok: &dyn Fn(char) -> bool| -> Option<usize> {
            if self.src.get(pos) == Some(&'0') && self.src.get(pos + 1).map(|c| c.to_ascii_lowercase()) == Some(p) {
                let mut q = pos + 2;
                let mut n = 0;
                loop {
                    if q < len && ok(self.src[q]) {
                        q += 1;
                        n += 1;
                    } else if q + 1 < len && self.src[q] == '_' && ok(self.src[q + 1]) {
                        q += 2;
                        n += 1;
                    } else {
                        break;
                    }
                }
                if n > 0 {
                    return Some(q);
                }
            }
            None
        };
        if let Some(e) = prefixed('b', &|c| c == '0' || c == '1') {
            return Some(e);
        }
        if let Some(e) = prefixed('o', &|c| ('0'..='7').contains(&c)) {
            return Some(e);
        }
        if let Some(e) = prefixed('x', &|c| c.is_ascii_hexdigit()) {
            return Some(e);
        }
        let first = *self.src.get(pos)?;
        let rest = |q0: usize, ok: &dyn Fn(char) -> bool| -> usize {
            let mut q = q0;
            loop {
                if q < len && ok(self.src[q]) {
                    q += 1;
                } else if q + 1 < len && self.src[q] == '_' && ok(self.src[q + 1]) {
                    q += 2;
                } else {
                    return q;
                }
            }
        };
        if ('1'..='9').contains(&first) {
            return Some(rest(pos + 1, &|c| c.is_ascii_digit()));
        }
        if first == '0' {
            return Some(rest(pos + 1, &|c| c == '0'));
        }
        None
    }

    fn match_string(&self, pos: usize) -> Option<usize> {
        let q = *self.src.get(pos)?;
        if q != '\'' && q != '"' {
            return None;
        }
        let len = self.src.len();
        let mut p = pos + 1;
        while p < len {
            let c = self.src[p];
            if c == q {
                return Some(p + 1);
            }
            if c == '\\' {
                if p + 1 >= len {
                    return None;
                }
                p += 2;
                continue;
            }
            p += 1;
        }
        None
    }

    fn step_comment(&mut self) -> Result<(), JErr> {
        let len = self.src.len();
        for s in self.pos..len {
            let end = if self.starts(s, "+#}") {
                Some(s + 3)
            } else if self.starts(s, "-#}") {
                Some(self.skip_ws(s + 3))
            } else if self.starts(s, "#}") {
                Some(if self.src.get(s + 2) == Some(&'\n') { s + 3 } else { s + 2 })
            } else {
                None
            };
            if let Some(end) = end {
                let text = self.s(self.pos, s);
                if !text.is_empty() {
                    self.push("comment", text.clone());
                }
                self.lineno += text.matches('\n').count();
                let endtxt = self.s(s, end);
                self.push("comment_end", endtxt.clone());
                self.lineno += endtxt.matches('\n').count();
                self.line_starting = endtxt.ends_with('\n');
                self.pos = end;
                self.stack.pop();
                return Ok(());
            }
        }
        if self.pos < len {
            return Err(JErr::syntax("Missing end of comment tag", self.lineno));
        }
        self.done = true;
        Ok(())
    }

    fn step_raw(&mut self) -> Result<(), JErr> {
        let len = self.src.len();
        for s in self.pos..len {
            if !self.starts(s, "{%") {
                continue;
            }
            let (sign, p) = self.sign_at(s + 2);
            let p = self.skip_ws(p);
            if !self.starts(p, "endraw") {
                continue;
            }
            let p = self.skip_ws(p + 6);
            let Some(end) = self.block_end_at(p) else {
                continue;
            };
            let text: Vec<char> = self.src[self.pos..s].to_vec();
            let text = self.lstrip_text(text, sign, false);
            let data: String = text.iter().collect();
            if !data.is_empty() {
                self.push("data", data);
            }
            self.lineno += count_nl(&text) + self.newlines_stripped;
            self.newlines_stripped = 0;
            let endtxt = self.s(s, end);
            self.push("raw_end", endtxt.clone());
            self.lineno += endtxt.matches('\n').count() + self.newlines_stripped;
            self.line_starting = endtxt.ends_with('\n');
            self.pos = end;
            self.stack.pop();
            return Ok(());
        }
        if self.pos < len {
            return Err(JErr::syntax("Missing end of raw directive", self.lineno));
        }
        self.done = true;
        Ok(())
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '·'
}

/// Python `str.isidentifier()` (approximated: first character letter/`_`).
fn is_identifier(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    it.all(|c| c.is_alphanumeric() || c == '_' || c == '·')
}

/// `value.encode("ascii", "backslashreplace").decode("unicode-escape")` – error check only.
fn check_unicode_escape(s: &str) -> Result<(), String> {
    // backslashreplace: non-ASCII → \xNN / \uNNNN / \UNNNNNNNN
    let mut enc = String::new();
    for c in s.chars() {
        let u = c as u32;
        if u < 0x80 {
            enc.push(c);
        } else if u < 0x100 {
            enc.push_str(&format!("\\x{u:02x}"));
        } else if u < 0x10000 {
            enc.push_str(&format!("\\u{u:04x}"));
        } else {
            enc.push_str(&format!("\\U{u:08x}"));
        }
    }
    let b: Vec<char> = enc.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] != '\\' {
            i += 1;
            continue;
        }
        let Some(&e) = b.get(i + 1) else {
            return Err("\\ at end of string".into());
        };
        let hex = |n: usize, msg: &str| -> Result<usize, String> {
            let digits: Vec<char> = b.iter().skip(i + 2).take(n).copied().collect();
            if digits.len() < n || !digits.iter().all(|c| c.is_ascii_hexdigit()) {
                return Err(msg.to_string());
            }
            Ok(n)
        };
        match e {
            'x' => i += 2 + hex(2, "truncated \\xXX escape")?,
            'u' => i += 2 + hex(4, "truncated \\uXXXX escape")?,
            'U' => {
                let n = hex(8, "truncated \\UXXXXXXXX escape")?;
                let v: String = b[i + 2..i + 2 + n].iter().collect();
                if u32::from_str_radix(&v, 16).map_or(true, |v| v > 0x10FFFF) {
                    return Err("illegal Unicode character".into());
                }
                i += 2 + n;
            }
            'N' => {
                if b.get(i + 2) != Some(&'{') {
                    return Err("malformed \\N character escape".into());
                }
                match b[i + 3..].iter().position(|c| *c == '}') {
                    Some(0) | None => return Err("malformed \\N character escape".into()),
                    Some(p) => i += 4 + p,
                }
            }
            _ => i += 2,
        }
    }
    Ok(())
}

/// `describe_token_type`
pub fn describe_type(ty: &str) -> String {
    if let Some(s) = op_symbol(ty) {
        return s.to_string();
    }
    match ty {
        "comment_begin" => "begin of comment",
        "comment_end" => "end of comment",
        "comment" | "linecomment" => "comment",
        "block_begin" => "begin of statement block",
        "block_end" => "end of statement block",
        "variable_begin" => "begin of print statement",
        "variable_end" => "end of print statement",
        "linestatement_begin" => "begin of line statement",
        "linestatement_end" => "end of line statement",
        "data" => "template data / text",
        "eof" => "end of template",
        other => other,
    }
    .to_string()
}

pub fn describe_token(t: &Token) -> String {
    if t.ty == "name" { t.value.clone() } else { describe_type(t.ty) }
}

pub fn describe_expr(expr: &str) -> String {
    if let Some((ty, val)) = expr.split_once(':') {
        if ty == "name" {
            return val.to_string();
        }
        return describe_type(ty);
    }
    describe_type(expr)
}

impl Token {
    pub fn test(&self, expr: &str) -> bool {
        if self.ty == expr {
            return true;
        }
        if let Some((ty, val)) = expr.split_once(':') {
            return ty == self.ty && val == self.value;
        }
        false
    }
    pub fn test_any(&self, exprs: &[&str]) -> bool {
        exprs.iter().any(|e| self.test(e))
    }
}

/// `TokenStream`
pub struct TokenStream {
    lexer: Lexer,
    pushed: VecDeque<Token>,
    pub current: Token,
    closed: bool,
}

impl TokenStream {
    pub fn new(source: &str) -> Result<Self, JErr> {
        let mut ts = TokenStream {
            lexer: Lexer::new(source),
            pushed: VecDeque::new(),
            current: Token { lineno: 1, ty: "initial", value: String::new() },
            closed: false,
        };
        ts.next()?;
        Ok(ts)
    }

    /// `__bool__`
    pub fn has_more(&self) -> bool {
        !self.pushed.is_empty() || self.current.ty != "eof"
    }

    /// `TokenStream.__next__` (name as upstream).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Token, JErr> {
        let rv = self.current.clone();
        if let Some(t) = self.pushed.pop_front() {
            self.current = t;
        } else if self.current.ty != "eof" {
            match if self.closed { None } else { self.lexer.next_token()? } {
                Some(t) => self.current = t,
                None => self.close(),
            }
        }
        Ok(rv)
    }

    fn close(&mut self) {
        self.current = Token { lineno: self.current.lineno, ty: "eof", value: String::new() };
        self.closed = true;
    }

    pub fn push(&mut self, t: Token) {
        self.pushed.push_back(t);
    }

    pub fn look(&mut self) -> Result<Token, JErr> {
        let old = self.next()?;
        let result = self.current.clone();
        self.push(result.clone());
        self.current = old;
        Ok(result)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), JErr> {
        for _ in 0..n {
            self.next()?;
        }
        Ok(())
    }

    pub fn next_if(&mut self, expr: &str) -> Result<Option<Token>, JErr> {
        if self.current.test(expr) { Ok(Some(self.next()?)) } else { Ok(None) }
    }

    pub fn skip_if(&mut self, expr: &str) -> Result<bool, JErr> {
        Ok(self.next_if(expr)?.is_some())
    }

    pub fn expect(&mut self, expr: &str) -> Result<Token, JErr> {
        if !self.current.test(expr) {
            let e = describe_expr(expr);
            if self.current.ty == "eof" {
                return Err(JErr::syntax(format!("unexpected end of template, expected {}.", str_repr(&e)), self.current.lineno));
            }
            return Err(JErr::syntax(
                format!("expected token {}, got {}", str_repr(&e), str_repr(&describe_token(&self.current))),
                self.current.lineno,
            ));
        }
        self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<(usize, &'static str, String)> {
        let mut l = Lexer::new(s);
        let mut out = Vec::new();
        while let Some(t) = l.next_token().unwrap() {
            out.push((t.lineno, t.ty, t.value));
        }
        out
    }

    #[test]
    fn basic() {
        let t = toks("a{{ x|upper }}b\n  {% if y %}c{% endif %}");
        let types: Vec<&str> = t.iter().map(|x| x.1).collect();
        assert_eq!(
            types,
            vec![
                "data",
                "variable_begin",
                "name",
                "pipe",
                "name",
                "variable_end",
                "data",
                "block_begin",
                "name",
                "name",
                "block_end",
                "data",
                "block_begin",
                "name",
                "block_end"
            ]
        );
        // lstrip_blocks removes the indentation before {% if %}
        assert_eq!(t[6].2, "b\n");
        assert_eq!(t[7].0, 2);
    }

    #[test]
    fn numbers() {
        let t = toks("{{ 1.5e3 0x1F 1_000 3.14 }}");
        let types: Vec<&str> = t.iter().map(|x| x.1).collect();
        assert_eq!(types, vec!["variable_begin", "float", "integer", "integer", "float", "variable_end"]);
    }

    #[test]
    fn errors() {
        let mut l = Lexer::new("{{ x ) }}");
        let mut err = None;
        loop {
            match l.next_token() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert_eq!(err.unwrap().msg, "unexpected ')'");
    }
}
