//! Sandbox environment (minijinja), configured like the Jinja2 environment of upstream.
//!
//! upstream: src/documents/templating/environment.py@v3.4.2 (JinjaEnvironment:
//! SandboxedEnvironment, trim_blocks=True, lstrip_blocks=True, keep_trailing_newline=False,
//! autoescape=False, loopcontrols; StrictUndefined with logging from filepath.py/workflows.py).
//!
//! Python semantics that minijinja does not have by itself are re-implemented:
//! * `==`/`!=`, `+`, `~`, `*` are rewritten into function calls before compiling
//!   (`__pl_eq`, `__pl_add`, `__pl_concat`, `__pl_mul`), so that `PlaceholderString == "none"`,
//!   Python `str()` for `~` and size limits apply.
//! * Output via Python's `str()` (`None`, `True`, `1.0`, `['a']`).
//! * Python methods on str/dict/list/date (see methods.rs).

use std::io;
use std::sync::OnceLock;

use minijinja::machinery::{tokenize, Token, WhitespaceConfig};
use minijinja::syntax::SyntaxConfig;
use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, State, UndefinedBehavior};

use crate::error::TemplateError;
use crate::filters as f;
use crate::pyvalue::{py_add, py_concat, py_eq, py_mul, py_str, MAX_STRING_BYTES, SECURITY_PREFIX};

/// Upper limit of VM instructions per render operation (protection against infinite loops/DoS).
pub const FUEL: u64 = 20_000_000;
/// Maximum size of the output in bytes.
pub const MAX_OUTPUT_BYTES: usize = MAX_STRING_BYTES;
/// Maximum size of a template in bytes (far above anything upstream fields reasonably hold).
pub const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;

/// Jinja2 globals that `find_undeclared_variables` does not report.
pub const JINJA2_GLOBALS: &[&str] = &["range", "dict", "lipsum", "cycler", "joiner", "namespace"];

fn whitespace() -> WhitespaceConfig {
    WhitespaceConfig { keep_trailing_newline: false, lstrip_blocks: true, trim_blocks: true }
}

fn fn_eq(a: Value, b: Value) -> Result<bool, Error> {
    py_eq(&a, &b)
}
fn fn_add(a: Value, b: Value) -> Result<Value, Error> {
    py_add(&a, &b)
}
fn fn_concat(a: Value, b: Value) -> Result<Value, Error> {
    py_concat(&a, &b)
}
fn fn_mul(a: Value, b: Value) -> Result<Value, Error> {
    py_mul(&a, &b)
}
fn fn_mod(a: Value, b: Value) -> Result<Value, Error> {
    crate::pyvalue::py_mod(&a, &b)
}
fn fn_pow(a: Value, b: Value) -> Result<Value, Error> {
    crate::pyvalue::py_pow(&a, &b)
}

fn formatter(out: &mut minijinja::Output, _state: &State, value: &Value) -> Result<(), Error> {
    if value.is_undefined() {
        return Err(Error::from(ErrorKind::UndefinedError));
    }
    let s = py_str(value)?;
    out.write_str(&s).map_err(|_| Error::new(ErrorKind::WriteFailure, "write failed"))
}

fn build_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_keep_trailing_newline(false);
    env.set_auto_escape_callback(|_| AutoEscape::None);
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_fuel(Some(FUEL));
    env.set_recursion_limit(200);
    env.set_formatter(formatter);
    env.set_unknown_method_callback(crate::methods::unknown_method);

    for name in f::MINIJINJA_ONLY_FILTERS {
        env.remove_filter(name);
    }
    for name in f::MINIJINJA_ONLY_TESTS {
        env.remove_test(name);
    }
    env.remove_global("debug");

    // upstream-Filter
    env.add_filter("get_cf_value", f::f_get_cf_value);
    env.add_filter("datetime", f::f_datetime);
    env.add_filter("slugify", f::f_slugify);
    env.add_filter("localize_date", f::f_localize_date);

    // Jinja2 filters with Python semantics
    env.add_filter("string", f::f_string);
    env.add_filter("lower", f::f_lower);
    env.add_filter("upper", f::f_upper);
    env.add_filter("capitalize", f::f_capitalize);
    env.add_filter("title", f::f_title);
    env.add_filter("replace", f::f_replace);
    env.add_filter("trim", f::f_trim);
    env.add_filter("center", f::f_center);
    env.add_filter("join", f::f_join);
    env.add_filter("truncate", f::f_truncate);
    env.add_filter("wordcount", f::f_wordcount);
    env.add_filter("int", f::f_int);
    env.add_filter("float", f::f_float);
    env.add_filter("round", f::f_round);
    env.add_filter("length", f::f_length);
    env.add_filter("count", f::f_length);
    env.add_filter("format", f::f_format);
    env.add_filter("filesizeformat", f::f_filesizeformat);
    env.add_filter("striptags", f::f_striptags);
    env.add_filter("forceescape", |state: &State, v: Value| minijinja::filters::escape(state, &v));
    // Jinja2 filters/globals without a minijinja counterpart (W9)
    env.add_filter("random", crate::extras::f_random);
    env.add_filter("urlize", crate::extras::f_urlize);
    env.add_filter("wordwrap", crate::extras::f_wordwrap);
    env.add_filter("xmlattr", crate::extras::f_xmlattr);
    env.add_global("lipsum", Value::from_function(crate::extras::fn_lipsum));
    env.add_global("cycler", Value::from_function(crate::extras::fn_cycler));
    env.add_global("joiner", Value::from_function(crate::extras::fn_joiner));

    // Tests with Python equality
    for name in ["eq", "equalto", "=="] {
        env.add_test(name, f::t_eq);
    }
    for name in ["ne", "!="] {
        env.add_test(name, f::t_ne);
    }
    env.add_test("callable", f::t_callable);

    // Rewrite targets
    env.add_function("__pl_eq", fn_eq);
    env.add_function("__pl_add", fn_add);
    env.add_function("__pl_concat", fn_concat);
    env.add_function("__pl_mul", fn_mul);
    env.add_function("__pl_pow", fn_pow);
    env.add_function("__pl_mod", fn_mod);
    env.add_function("__pl_list", crate::pylist::fn_pl_list);
    env
}

fn base_env() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(build_env)
}

/// Copy of the environment with the lifetime of the template (cloning only copies Arc pointers).
fn env<'a>() -> Environment<'a> {
    base_env().clone()
}

// ---------------------------------------------------------------------------
// Rewriting the operators
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Eq,
    Ne,
    Add,
    Concat,
    Mul,
    Mod,
    Pow,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    Open,
    Close,
    /// Binary operator with binding strength (4 = comparison, 5 = +/-, 6 = ~, 7 = * / // %, 8 = **)
    Bin(u8, Option<Op>),
    Plus,
    Minus,
    /// End of an expression (block boundaries, comma, colon, `=`, keywords)
    Boundary,
    Is,
    Not,
    Other,
}

struct Tk {
    k: K,
    start: usize,
    end: usize,
}

const KEYWORDS: &[&str] = &[
    "and", "or", "in", "if", "elif", "else", "for", "recursive", "set", "with", "as", "import", "from", "include",
    "extends", "block", "macro", "call", "filter", "ignore", "missing", "context", "without", "endif", "endfor",
];

fn lex(src: &str) -> Option<Vec<Tk>> {
    let mut out = Vec::new();
    for item in tokenize(src, false, SyntaxConfig, whitespace()) {
        let (tok, span) = item.ok()?;
        let k = match tok {
            Token::TemplateData(_)
            | Token::VariableStart
            | Token::VariableEnd
            | Token::BlockStart
            | Token::BlockEnd
            | Token::Comma
            | Token::Colon
            | Token::Assign => K::Boundary,
            Token::ParenOpen | Token::BracketOpen | Token::BraceOpen => K::Open,
            Token::ParenClose | Token::BracketClose | Token::BraceClose => K::Close,
            Token::Eq => K::Bin(4, Some(Op::Eq)),
            Token::Ne => K::Bin(4, Some(Op::Ne)),
            Token::Lt | Token::Lte | Token::Gt | Token::Gte => K::Bin(4, None),
            Token::Plus => K::Plus,
            Token::Minus => K::Minus,
            Token::Tilde => K::Bin(6, Some(Op::Concat)),
            Token::Mul => K::Bin(7, Some(Op::Mul)),
            Token::Mod => K::Bin(7, Some(Op::Mod)),
            Token::Div | Token::FloorDiv => K::Bin(7, None),
            Token::Pow => K::Bin(8, Some(Op::Pow)),
            Token::Ident("is") => K::Is,
            Token::Ident("not") => K::Not,
            Token::Ident(name) if KEYWORDS.contains(&name) => K::Boundary,
            _ => K::Other,
        };
        out.push(Tk { k, start: span.start_offset as usize, end: span.end_offset as usize });
    }
    Some(out)
}

fn is_unary(toks: &[Tk], i: usize) -> bool {
    i == 0
        || matches!(
            toks[i - 1].k,
            K::Open | K::Bin(..) | K::Plus | K::Minus | K::Boundary | K::Not | K::Is
        )
}

/// Does `k` at position `i` terminate the operand of an operator of strength `level`?
fn stops(toks: &[Tk], i: usize, level: u8) -> bool {
    match toks[i].k {
        K::Boundary => true,
        K::Not => !(i > 0 && toks[i - 1].k == K::Is),
        K::Bin(l, _) => l <= level,
        K::Plus | K::Minus => !is_unary(toks, i) && 5 <= level,
        _ => false,
    }
}

fn operand_left(toks: &[Tk], k: usize, level: u8) -> Option<usize> {
    let mut start = None;
    let mut depth = 0i32;
    let mut i = k;
    while i > 0 {
        i -= 1;
        match toks[i].k {
            K::Close => depth += 1,
            K::Open => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ if depth > 0 => {}
            _ if stops(toks, i, level) => break,
            _ => {}
        }
        start = Some(i);
    }
    if depth != 0 {
        return None;
    }
    start
}

fn operand_right(toks: &[Tk], k: usize, level: u8) -> Option<usize> {
    let mut end = None;
    let mut depth = 0i32;
    let mut i = k + 1;
    while i < toks.len() {
        match toks[i].k {
            K::Open => depth += 1,
            K::Close => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ if depth > 0 => {}
            K::Plus | K::Minus if is_unary(toks, i) => {}
            _ if stops(toks, i, level) => break,
            _ => {}
        }
        end = Some(i);
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    end
}

/// Rewrites `==`, `!=`, binary `+`, `~` and `*` into function calls.
/// `None` if the template cannot be split cleanly (the compiler then reports the error).
pub(crate) fn rewrite_operators(src: &str) -> Option<String> {
    let mut cur = src.to_string();
    for _pass in 0..512 {
        let toks = lex(&cur)?;
        let mut picks: Vec<(usize, usize, usize, Op)> = Vec::new(); // (start_tok, op_tok, end_tok, op)
        let mut busy: Vec<(usize, usize)> = Vec::new();
        for k in 0..toks.len() {
            let (level, op) = match toks[k].k {
                K::Bin(l, Some(op)) => (l, op),
                K::Plus if !is_unary(&toks, k) => (5, Op::Add),
                _ => continue,
            };
            let l = operand_left(&toks, k, level)?;
            let r = operand_right(&toks, k, level)?;
            let (a, b) = (toks[l].start, toks[r].end);
            if busy.iter().any(|(s, e)| a < *e && *s < b) {
                continue;
            }
            busy.push((a, b));
            picks.push((l, k, r, op));
        }
        if picks.is_empty() {
            return Some(cur);
        }
        for (l, k, r, op) in picks.into_iter().rev() {
            let a = toks[l].start;
            let b = toks[r].end;
            let left = &cur[a..toks[k].start];
            let right = &cur[toks[k].end..b];
            let call = match op {
                Op::Eq => format!("(__pl_eq(({left}),({right})))"),
                Op::Ne => format!("(not __pl_eq(({left}),({right})))"),
                Op::Add => format!("(__pl_add(({left}),({right})))"),
                Op::Concat => format!("(__pl_concat(({left}),({right})))"),
                Op::Mul => format!("(__pl_mul(({left}),({right})))"),
                Op::Mod => format!("(__pl_mod(({left}),({right})))"),
                Op::Pow => format!("(__pl_pow(({left}),({right})))"),
            };
            cur = format!("{}{}{}", &cur[..a], call, &cur[b..]);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

struct CappedWriter {
    buf: Vec<u8>,
}

impl io::Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len() + data.len() > MAX_OUTPUT_BYTES {
            return Err(io::Error::other("output size limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Map minijinja errors to the upstream exception classes.
pub(crate) fn classify(e: &Error) -> TemplateError {
    let detail = e.detail().map(str::to_string).unwrap_or_else(|| e.kind().to_string());
    match e.kind() {
        ErrorKind::SyntaxError | ErrorKind::UnknownFilter | ErrorKind::UnknownTest => TemplateError::Syntax(detail),
        ErrorKind::UndefinedError | ErrorKind::UnknownMethod | ErrorKind::UnknownFunction => {
            TemplateError::Undefined(detail)
        }
        ErrorKind::OutOfFuel | ErrorKind::WriteFailure => TemplateError::Security(detail),
        _ if detail.starts_with(SECURITY_PREFIX) => {
            TemplateError::Security(detail.trim_start_matches(SECURITY_PREFIX).to_string())
        }
        _ => TemplateError::Other(detail),
    }
}

/// Compiles the template unchanged (syntax check only, like `env.parse`).
pub(crate) fn check_syntax(source: &str) -> Result<(), TemplateError> {
    if source.len() > MAX_TEMPLATE_BYTES {
        return Err(TemplateError::Security("template too large".into()));
    }
    // Parse only: minijinja folds constants while compiling (e.g. `'a' * 10**11`) and would
    // request memory unchecked in doing so. Only the rewritten template is compiled.
    minijinja::machinery::parse(source, "<string>", SyntaxConfig, whitespace())
        .map(|_| ())
        .map_err(|e| classify(&e))
}

/// Check and rewrite (the result may be compiled).
fn prepare(source: &str) -> Result<String, TemplateError> {
    check_syntax(source)?;
    let source = crate::pylist::wrap_list_literals(source, whitespace())
        .ok_or_else(|| TemplateError::Syntax("template could not be analysed".into()))?;
    rewrite_operators(&source).ok_or_else(|| TemplateError::Syntax("template could not be analysed".into()))
}

/// Undeclared variables (top level) of the template, without Jinja2 globals.
pub(crate) fn undeclared_variables(source: &str) -> Result<Vec<String>, TemplateError> {
    let rewritten = prepare(source)?;
    let env = env();
    // Panic while compiling (finding S-9, see `render`) → template error
    let analysed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Vec<String>, TemplateError> {
        let tmpl = env.template_from_str(&rewritten).map_err(|e| classify(&e))?;
        Ok(tmpl
            .undeclared_variables(false)
            .into_iter()
            .filter(|n| !JINJA2_GLOBALS.contains(&n.as_str()) && !n.starts_with("__pl_"))
            .collect())
    }));
    let mut v = match analysed {
        Ok(r) => r?,
        Err(_) => return Err(TemplateError::Other("template could not be analysed".into())),
    };
    v.sort();
    Ok(v)
}

/// Checks filter and test names against the Jinja2/upstream list (Jinja2: TemplateAssertionError
/// when compiling). Returns the text of the first error message like Jinja2's `str(e)`.
pub(crate) fn check_filter_and_test_names(source: &str) -> Option<String> {
    let toks: Vec<(Token<'_>, minijinja::machinery::Span)> =
        tokenize(source, false, SyntaxConfig, whitespace()).collect::<Result<_, _>>().ok()?;
    let ident = |i: usize| -> Option<&str> {
        match toks.get(i) {
            Some((Token::Ident(n), _)) => Some(n),
            _ => None,
        }
    };
    for i in 0..toks.len() {
        let line = toks[i].1.start_line;
        let filter_name = match &toks[i].0 {
            Token::Pipe => ident(i + 1),
            Token::BlockStart if ident(i + 1) == Some("filter") => ident(i + 2),
            _ => None,
        };
        if let Some(name) = filter_name
            && !f::JINJA2_FILTERS.contains(&name) && !f::PAPERLESS_FILTERS.contains(&name) {
                return Some(format!("No filter named '{name}'.\n  File \"<introspection>\", line {line}"));
            }
        if ident(i) == Some("is") {
            let j = if ident(i + 1) == Some("not") { i + 2 } else { i + 1 };
            let name = match toks.get(j) {
                Some((Token::Ident(n), _)) => Some(n.to_string()),
                Some((Token::Eq, _)) => Some("==".into()),
                Some((Token::Ne, _)) => Some("!=".into()),
                Some((Token::Lt, _)) => Some("<".into()),
                Some((Token::Lte, _)) => Some("<=".into()),
                Some((Token::Gt, _)) => Some(">".into()),
                Some((Token::Gte, _)) => Some(">=".into()),
                _ => None,
            };
            if let Some(name) = name
                && !f::JINJA2_TESTS.contains(&name.as_str()) {
                    return Some(format!("No test named '{name}'.\n  File \"<introspection>\", line {line}"));
                }
        }
    }
    None
}

/// Renders `source` with `ctx` in the sandbox.
pub(crate) fn render(source: &str, ctx: Value) -> Result<String, TemplateError> {
    let rewritten = prepare(source)?;
    if let Some(msg) = check_filter_and_test_names(source) {
        return Err(TemplateError::Syntax(msg));
    }
    let env = env();
    // Security review 2026-11-18 (fuzzing, finding S-9): minijinja 2.19 can panic while compiling
    // (constant folding `Expr::as_const` → `Value::cmp`, unwrap on None, e.g. when comparing with
    // huge number literals) and while rendering. A panic must neither abort the request nor the
    // worker task, but becomes an ordinary template error.
    let mut w = CappedWriter { buf: Vec::new() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), TemplateError> {
        let tmpl = env.template_from_str(&rewritten).map_err(|e| classify(&e))?;
        tmpl.render_captured_to(ctx, &mut w).map_err(|e| {
            tracing::warn!(target: "paperless.templating", "Template rendering error: {e:#}");
            classify(&e)
        })?;
        Ok(())
    }));
    match result {
        Ok(r) => r?,
        Err(_) => {
            tracing::error!(target: "paperless.templating", "Template rendering panicked (treated as template error)");
            return Err(TemplateError::Other("template rendering failed".into()));
        }
    }
    String::from_utf8(w.buf).map_err(|_| TemplateError::Other("invalid utf-8 output".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fuzzing finding S-9: minijinja panic while compiling (comparison with a huge number literal).
    #[test]
    fn minijinja_panic_becomes_error() {
        let src = "E{{ 0-0900000000000000000000000000000000000000>000-000000000-00-09.0}}";
        let r = render(src, Value::from(()));
        assert!(r.is_err(), "{r:?}");
        assert!(undeclared_variables(src).is_err());
        // second variant from the 20.11. fuzzing (same cause in minijinja, also caught)
        let src2 = "{{666666666+6-666666666666666666666+66666/66--2<222222222222266666666666660 }}";
        let _ = render(src2, Value::from(()));
        let _ = undeclared_variables(src2);
    }

    #[test]
    fn rewrite_simple() {
        assert_eq!(
            rewrite_operators("{% if a == 'none' %}x{% endif %}").unwrap(),
            "{% if (__pl_eq((a ),( 'none'))) %}x{% endif %}"
        );
        let r = rewrite_operators("{{ a ~ b == c }}").unwrap();
        assert!(!r.contains("==") && !r.contains('~'), "{r}");
        let r = rewrite_operators("{{ -a * b + c|f(1 == 2) }}").unwrap();
        assert!(!r.contains("==") && !r.contains('*') && !r.contains('+'), "{r}");
        assert_eq!(rewrite_operators("{{ x }}{# a == b #}raw == text").unwrap(), "{{ x }}{# a == b #}raw == text");
    }
}
