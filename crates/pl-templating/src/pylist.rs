//! Mutable Python lists for templates (`{% set x = [] %}{{ x.append(1) }}`).
//!
//! upstream: jinja2/sandbox.py@3.1.6 – `SandboxedEnvironment` (not `ImmutableSandboxedEnvironment`)
//! allows the list methods `append`, `extend`, `insert`, `pop`, `remove`, `clear`, `reverse`,
//! `sort`; src/documents/templating/environment.py@v3.4.2 blocks only `save`/`delete`/`update`.
//!
//! minijinja values are immutable. Therefore [`wrap_list_literals`] rewrites list literals into
//! `__pl_list([...])` – but only if the template calls one of these methods at all, so that
//! all other templates stay unchanged. `__pl_list` returns a [`PyList`] object that behaves
//! like a sequence everywhere (iteration, length, index, filters, `str()`).

use std::sync::{Arc, Mutex, MutexGuard};

use minijinja::machinery::{Token, WhitespaceConfig, tokenize};
use minijinja::syntax::SyntaxConfig;
use minijinja::value::{Enumerator, Object, ObjectRepr, Value, ValueKind};
use minijinja::{Error, ErrorKind, State};

use crate::methods::PyArgs;
use crate::pyvalue::{MAX_SEQ_ITEMS, invalid, py_eq_inner, py_repr};

/// Methods that modify a list (trigger for the rewriting).
const MUTATORS: &[&str] = &["append", "extend", "insert", "pop", "remove", "clear", "reverse", "sort"];

#[derive(Debug, Default)]
pub struct PyList(Mutex<Vec<Value>>);

impl PyList {
    fn items(&self) -> MutexGuard<'_, Vec<Value>> {
        // Poisoning is uncritical here (no panic path holds the lock across user code).
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn snapshot(&self) -> Vec<Value> {
        self.items().clone()
    }
}

/// Does `v` (recursively) contain the list `me`? Prevents cycles (Python prints `[...]`; error here,
/// because a cycle would make `str()` endless).
fn contains_self(me: &Arc<PyList>, v: &Value, depth: usize) -> bool {
    if depth > 64 {
        return true;
    }
    if let Some(other) = v.downcast_object::<PyList>() {
        if Arc::ptr_eq(me, &other) {
            return true;
        }
        return other.snapshot().iter().any(|x| contains_self(me, x, depth + 1));
    }
    match v.kind() {
        ValueKind::Seq => v.try_iter().map(|mut it| it.any(|x| contains_self(me, &x, depth + 1))).unwrap_or(false),
        ValueKind::Map => v
            .try_iter()
            .map(|mut it| it.any(|k| v.get_item(&k).map(|x| contains_self(me, &x, depth + 1)).unwrap_or(false)))
            .unwrap_or(false),
        _ => false,
    }
}

fn check_insert(me: &Arc<PyList>, v: &Value, new_len: usize) -> Result<(), Error> {
    if new_len > MAX_SEQ_ITEMS {
        return Err(crate::pyvalue::too_large());
    }
    if contains_self(me, v, 0) {
        return Err(invalid("a list cannot contain itself"));
    }
    Ok(())
}

fn index_arg(v: &Value, what: &str) -> Result<i64, Error> {
    match crate::pyvalue::as_num(v) {
        Some(crate::pyvalue::Num::Int(n)) => i64::try_from(n).map_err(|_| invalid("cannot fit 'int' into an index-sized integer")),
        _ => Err(invalid(format!("'{}' object cannot be interpreted as an integer ({what})", crate::pyvalue::py_type(v)))),
    }
}

impl Object for PyList {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let items = self.items();
        let idx = key.as_i64()?;
        let len = items.len() as i64;
        let i = if idx < 0 { idx + len } else { idx };
        if i < 0 {
            return None;
        }
        items.get(i as usize).cloned()
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Values(self.snapshot())
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        Some(self.items().len())
    }

    fn is_true(self: &Arc<Self>) -> bool {
        !self.items().is_empty()
    }

    fn call_method(self: &Arc<Self>, _state: &State<'_, '_>, method: &str, args: &[Value]) -> Result<Value, Error> {
        let a = PyArgs::new(args)?;
        let none = Value::from(());
        match method {
            "append" => {
                let v = a.get(0, "object").ok_or_else(|| invalid("list.append() takes exactly one argument (0 given)"))?;
                let len = self.items().len() + 1;
                check_insert(self, &v, len)?;
                self.items().push(v);
                Ok(none)
            }
            "extend" => {
                let v = a.get(0, "iterable").ok_or_else(|| invalid("list.extend() takes exactly one argument (0 given)"))?;
                let new: Vec<Value> = v.try_iter()?.collect();
                let len = self.items().len() + new.len();
                for x in &new {
                    check_insert(self, x, len)?;
                }
                self.items().extend(new);
                Ok(none)
            }
            "insert" => {
                let i = index_arg(&a.get(0, "index").ok_or_else(|| invalid("insert expected 2 arguments, got 0"))?, "insert")?;
                let v = a.get(1, "object").ok_or_else(|| invalid("insert expected 2 arguments, got 1"))?;
                let len = self.items().len();
                check_insert(self, &v, len + 1)?;
                let l = len as i64;
                let pos = if i < 0 { (i + l).max(0) } else { i.min(l) } as usize;
                self.items().insert(pos, v);
                Ok(none)
            }
            "pop" => {
                let mut items = self.items();
                if items.is_empty() {
                    return Err(invalid("pop from empty list"));
                }
                let l = items.len() as i64;
                let i = match a.get(0, "index") {
                    Some(v) => index_arg(&v, "pop")?,
                    None => -1,
                };
                let pos = if i < 0 { i + l } else { i };
                if pos < 0 || pos >= l {
                    return Err(invalid("pop index out of range"));
                }
                Ok(items.remove(pos as usize))
            }
            "remove" => {
                let x = a.get(0, "value").ok_or_else(|| invalid("list.remove() takes exactly one argument (0 given)"))?;
                let mut items = self.items();
                match items.iter().position(|i| py_eq_inner(i, &x)) {
                    Some(p) => {
                        items.remove(p);
                        Ok(none)
                    }
                    None => Err(invalid("list.remove(x): x not in list")),
                }
            }
            "clear" => {
                self.items().clear();
                Ok(none)
            }
            "reverse" => {
                self.items().reverse();
                Ok(none)
            }
            "sort" => {
                let reverse = a.get(1, "reverse").is_some_and(|v| v.is_true());
                if a.get(0, "key").is_some() {
                    return Err(invalid("list.sort(key=...) is not supported"));
                }
                let mut items = self.snapshot();
                // Python compares only values of the same kind; minijinja ordering is the same for numbers/strings.
                let mixed = items.windows(2).any(|w| {
                    let num = |v: &Value| crate::pyvalue::as_num(v).is_some();
                    num(&w[0]) != num(&w[1]) || (w[0].kind() == ValueKind::String) != (w[1].kind() == ValueKind::String)
                });
                if mixed {
                    return Err(invalid(format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        crate::pyvalue::py_type(&items[0]),
                        crate::pyvalue::py_type(&items[1])
                    )));
                }
                items.sort();
                if reverse {
                    items.reverse();
                }
                *self.items() = items;
                Ok(none)
            }
            "copy" => Ok(Value::from_object(PyList(Mutex::new(self.snapshot())))),
            // index/count/... are handled by the general callback in methods.rs
            _ => Err(Error::from(ErrorKind::UnknownMethod)),
        }
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = Value::from(self.snapshot());
        match py_repr(&v) {
            Ok(s) => f.write_str(&s),
            Err(_) => Err(std::fmt::Error),
        }
    }
}

/// `__pl_list(seq)` – rewrite target for list literals.
pub fn fn_pl_list(v: Value) -> Result<Value, Error> {
    let items: Vec<Value> = v.try_iter()?.collect();
    Ok(Value::from_object(PyList(Mutex::new(items))))
}

/// Rewrites list literals into `__pl_list([...])` if the template calls a modifying
/// list method (`.append(` etc.). Otherwise unchanged. `None` on a splitting error.
pub(crate) fn wrap_list_literals(src: &str, ws: WhitespaceConfig) -> Option<String> {
    let toks: Vec<(Token<'_>, minijinja::machinery::Span)> = tokenize(src, false, SyntaxConfig, ws).collect::<Result<_, _>>().ok()?;
    let calls_mutator = toks.windows(3).any(|w| {
        matches!(w[0].0, Token::Dot) && matches!(w[1].0, Token::Ident(n) if MUTATORS.contains(&n)) && matches!(w[2].0, Token::ParenOpen)
    });
    if !calls_mutator {
        return Some(src.to_string());
    }
    // Determine pairs (open, close) of the literals.
    let mut inserts: Vec<(usize, &'static str)> = Vec::new();
    let mut stack: Vec<(bool, usize)> = Vec::new(); // (is literal, byte offset of '[')
    for (i, (tok, span)) in toks.iter().enumerate() {
        match tok {
            Token::BracketOpen => {
                // Subscript after name, literal, `)`, `]`, `}`; otherwise list literal.
                let literal = match i.checked_sub(1).map(|p| &toks[p].0) {
                    Some(Token::Ident(n)) => {
                        matches!(*n, "in" | "and" | "or" | "not" | "if" | "else" | "is" | "return")
                    }
                    Some(
                        Token::Str(_)
                        | Token::String(_)
                        | Token::Int(_)
                        | Token::Int128(_)
                        | Token::Float(_)
                        | Token::ParenClose
                        | Token::BracketClose
                        | Token::BraceClose,
                    ) => false,
                    _ => true,
                };
                stack.push((literal, span.start_offset as usize));
            }
            Token::BracketClose => {
                let (literal, start) = stack.pop()?;
                if literal {
                    inserts.push((start, "__pl_list("));
                    inserts.push((span.end_offset as usize, ")"));
                }
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return None;
    }
    // Insert from the back; at the same offset first the opening, then ")" (yields ")__pl_list(").
    inserts.sort_by_key(|(pos, text)| (std::cmp::Reverse(*pos), *text == ")"));
    let mut out = src.to_string();
    for (pos, text) in inserts {
        out.insert_str(pos, text);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use minijinja::value::Value;

    use crate::error::TemplateError;

    fn r(src: &str) -> Result<String, TemplateError> {
        let ctx = Value::from_iter([
            ("tags", Value::from(vec!["b", "a"])),
            ("doc", Value::from_iter([("title", Value::from("T")), ("pk", Value::from(7))])),
        ]);
        crate::env::render(src, ctx)
    }

    #[test]
    fn append_and_friends() {
        assert_eq!(
            r("{% set x = [] %}{% for t in tags %}{% if x.append(t|upper) %}{% endif %}{% endfor %}{{ x|join('-') }}").unwrap(),
            "B-A"
        );
        assert_eq!(r("{% set x = [] %}{{ x.append(1) }}|{{ x }}").unwrap(), "None|[1]");
        assert_eq!(r("{% set x = [3, 1] %}{{ x.extend([2]) or '' }}{{ x.sort() or '' }}{{ x }}").unwrap(), "[1, 2, 3]");
        assert_eq!(r("{% set x = [1, 2, 3] %}{{ x.pop() }}{{ x.pop(0) }}{{ x }}{{ x|length }}").unwrap(), "31[2]1");
        assert_eq!(r("{% set x = ['a'] %}{{ x.insert(0, 'z') or '' }}{{ x.remove('a') or '' }}{{ x[0] }}{{ x[-1] }}").unwrap(), "zz");
        assert_eq!(r("{% set x = [1] %}{{ x.append(2) or '' }}{{ x.index(2) }}{{ 2 in x }}{{ x == [1, 2] }}").unwrap(), "1TrueTrue");
        // Literals as subscript stay subscript
        assert_eq!(r("{% set x = [[1, 2]] %}{{ x.append(tags[0]) or '' }}{{ x[0][1] }}{{ x }}").unwrap(), "2[[1, 2], 'b']");
    }

    #[test]
    fn errors() {
        assert!(r("{% set x = [] %}{{ x.pop() }}").is_err());
        assert!(r("{% set x = [] %}{{ x.append(x) }}").is_err(), "zyklische Liste");
        assert!(matches!(r("{% set x = [] %}{{ x.update() }}"), Err(TemplateError::Security(_))));
    }

    #[test]
    fn untouched_without_mutator() {
        let ws = minijinja::machinery::WhitespaceConfig::default();
        assert_eq!(super::wrap_list_literals("{{ [1, 2]|join }}", ws).unwrap(), "{{ [1, 2]|join }}");
        assert_eq!(
            super::wrap_list_literals("{% set x = [[1]] %}{{ x.append(a[0]) }}", ws).unwrap(),
            "{% set x = __pl_list([__pl_list([1])]) %}{{ x.append(a[0]) }}"
        );
    }

    #[test]
    fn format_star_and_attr() {
        assert_eq!(r("{{ '%*d|%-*s|%.*f' % (5, 42, 3, 'a', 2, 3.14159) }}").unwrap(), "   42|a  |3.14");
        assert_eq!(r("{{ '{0[title]}-{0.pk:03d}'.format(doc) }}").unwrap(), "T-007");
        assert_eq!(r("{{ '{t[1]}'.format(t=tags) }}").unwrap(), "a");
        assert_eq!(r("{{ '{:>{w}}'.format('x', w=3) }}").unwrap(), "  x");
        assert!(matches!(r("{{ '{0.__class__}'.format(doc) }}"), Err(TemplateError::Security(_))));
        assert!(r("{{ '{0.missing}'.format(doc) }}").is_err());
        assert!(r("{{ '%*d' % ('x', 1) }}").is_err());
    }
}
