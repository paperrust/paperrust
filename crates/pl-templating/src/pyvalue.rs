//! Python values in the template context: `date`, `datetime`, `PlaceholderString`, `MatchingModelContext`
//! as well as `str()`/`repr()`/`==`/`+`/`~`/`*` with Python semantics and size limits.
//!
//! upstream: src/documents/templating/filepath.py@v3.4.2 (PlaceholderString, MatchingModelContext),
//! CPython 3.12 (`str`, `repr`, `datetime`), Jinja2 3.1.6 runtime (`~` = `str(a) + str(b)`).

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use chrono::{Datelike, FixedOffset, NaiveDate, NaiveDateTime, Timelike};
use minijinja::value::{DynObject, Object, ObjectRepr, Value, ValueKind};
use minijinja::{Error, ErrorKind};

use crate::pystr::{py_float_repr, py_str_repr};
use crate::strftime::{TzInfo, strftime};

/// Maximum length (bytes) of a string that an operation in the template may produce.
/// Protection against memory exhaustion (Python would raise `MemoryError`, Rust aborts).
pub const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of elements of a generated list.
pub const MAX_SEQ_ITEMS: usize = 1_000_000;

pub(crate) const SECURITY_PREFIX: &str = "SecurityError: ";

pub(crate) fn security_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, format!("{SECURITY_PREFIX}{}", msg.into()))
}

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, msg.into())
}

pub(crate) fn too_large() -> Error {
    security_error("result exceeds the sandbox size limit")
}

pub(crate) fn check_len(n: usize) -> Result<(), Error> {
    if n > MAX_STRING_BYTES { Err(too_large()) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// Objects
// ---------------------------------------------------------------------------

/// `PlaceholderString("-none-")`: is output as "-none-", is falsy and equal to "none"/"-none-".
#[derive(Debug)]
pub struct Placeholder;

impl Object for Placeholder {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn is_true(self: &Arc<Self>) -> bool {
        false
    }
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("-none-")
    }
    fn custom_cmp(self: &Arc<Self>, _other: &DynObject) -> Option<Ordering> {
        Some(Ordering::Equal)
    }
}

/// The placeholder for empty values.
pub fn placeholder() -> Value {
    Value::from_object(Placeholder)
}

pub fn is_placeholder(v: &Value) -> bool {
    v.downcast_object_ref::<Placeholder>().is_some()
}

/// `MatchingModelContext`: output = name, attributes `id`, `name`, `path`.
#[derive(Debug)]
pub struct MatchingModel {
    pub id: Option<i64>,
    pub name: String,
    pub path: Option<String>,
}

impl Object for MatchingModel {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "id" => Some(self.id.map(Value::from).unwrap_or(Value::from(()))),
            "name" => Some(Value::from(self.name.clone())),
            "path" => Some(self.path.clone().map(Value::from).unwrap_or(Value::from(()))),
            _ => None,
        }
    }
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Python `datetime.date`.
#[derive(Debug, Clone, Copy)]
pub struct PyDate(pub NaiveDate);

impl Object for PyDate {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let d = self.0;
        Some(Value::from(match key.as_str()? {
            "year" => d.year() as i64,
            "month" => d.month() as i64,
            "day" => d.day() as i64,
            _ => return None,
        }))
    }
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&date_iso(self.0))
    }
    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<Ordering> {
        other.downcast_ref::<PyDate>().map(|o| self.0.cmp(&o.0))
    }
}

/// Python `datetime.datetime` (naive or with fixed offset).
#[derive(Debug, Clone, Copy)]
pub struct PyDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<FixedOffset>,
}

impl PyDateTime {
    fn utc_key(&self) -> NaiveDateTime {
        match self.offset {
            Some(o) => self.naive - chrono::Duration::seconds(o.local_minus_utc() as i64),
            None => self.naive,
        }
    }
    pub fn tzinfo(&self) -> TzInfo {
        match self.offset {
            Some(o) => TzInfo::Fixed(o),
            None => TzInfo::Naive,
        }
    }
    pub fn isoformat(&self, sep: &str) -> String {
        let n = self.naive;
        let mut s = format!("{}{}{:02}:{:02}:{:02}", date_iso(n.date()), sep, n.hour(), n.minute(), n.second());
        let us = n.nanosecond() / 1000;
        if us != 0 {
            s.push_str(&format!(".{us:06}"));
        }
        if let Some(o) = self.offset {
            let secs = o.local_minus_utc();
            let sign = if secs < 0 { '-' } else { '+' };
            let a = secs.abs();
            s.push_str(&format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60));
            if a % 60 != 0 {
                s.push_str(&format!(":{:02}", a % 60));
            }
        }
        s
    }
}

impl Object for PyDateTime {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let d = self.naive;
        Some(Value::from(match key.as_str()? {
            "year" => d.year() as i64,
            "month" => d.month() as i64,
            "day" => d.day() as i64,
            "hour" => d.hour() as i64,
            "minute" => d.minute() as i64,
            "second" => d.second() as i64,
            "microsecond" => (d.nanosecond() / 1000) as i64,
            _ => return None,
        }))
    }
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.isoformat(" "))
    }
    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<Ordering> {
        other.downcast_ref::<PyDateTime>().map(|o| self.utc_key().cmp(&o.utc_key()))
    }
}

/// `date.isoformat()` (four-digit year).
pub fn date_iso(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

pub fn date_value(d: NaiveDate) -> Value {
    Value::from_object(PyDate(d))
}

pub fn datetime_value(naive: NaiveDateTime, offset: Option<FixedOffset>) -> Value {
    Value::from_object(PyDateTime { naive, offset })
}

// ---------------------------------------------------------------------------
// str() / repr()
// ---------------------------------------------------------------------------

fn undefined_error() -> Error {
    Error::from(ErrorKind::UndefinedError)
}

/// Python `type(value)` for error messages.
pub fn py_type(v: &Value) -> &'static str {
    if is_placeholder(v) {
        return "<class 'documents.templating.filepath.PlaceholderString'>";
    }
    if v.downcast_object_ref::<PyDate>().is_some() {
        return "<class 'datetime.date'>";
    }
    if v.downcast_object_ref::<PyDateTime>().is_some() {
        return "<class 'datetime.datetime'>";
    }
    if v.downcast_object_ref::<MatchingModel>().is_some() {
        return "<class 'documents.templating.filepath.MatchingModelContext'>";
    }
    match v.kind() {
        ValueKind::Undefined => "<class 'jinja2.runtime.StrictUndefined'>",
        ValueKind::None => "<class 'NoneType'>",
        ValueKind::Bool => "<class 'bool'>",
        ValueKind::Number if v.is_integer() => "<class 'int'>",
        ValueKind::Number => "<class 'float'>",
        ValueKind::String => "<class 'str'>",
        ValueKind::Bytes => "<class 'bytes'>",
        ValueKind::Seq | ValueKind::Iterable => "<class 'list'>",
        ValueKind::Map => "<class 'dict'>",
        _ => "<class 'object'>",
    }
}

/// Python `str(value)`.
pub fn py_str(v: &Value) -> Result<String, Error> {
    if let Some(s) = v.as_str() {
        return Ok(s.to_string());
    }
    if v.as_object().is_some() && matches!(v.kind(), ValueKind::Plain) {
        return Ok(v.to_string());
    }
    match v.kind() {
        ValueKind::Undefined => Err(undefined_error()),
        ValueKind::None => Ok("None".into()),
        ValueKind::Bool => Ok(if v.is_true() { "True".into() } else { "False".into() }),
        ValueKind::Number => {
            if v.is_integer() {
                Ok(v.to_string())
            } else {
                Ok(py_float_repr(f64::try_from(v.clone()).unwrap_or(f64::NAN)))
            }
        }
        _ => py_repr(v),
    }
}

/// Python `repr(value)`.
pub fn py_repr(v: &Value) -> Result<String, Error> {
    if is_placeholder(v) {
        return Ok("'-none-'".into());
    }
    if let Some(d) = v.downcast_object_ref::<PyDate>() {
        return Ok(format!("datetime.date({}, {}, {})", d.0.year(), d.0.month(), d.0.day()));
    }
    if let Some(d) = v.downcast_object_ref::<PyDateTime>() {
        let n = d.naive;
        let mut s = format!("datetime.datetime({}, {}, {}, {}, {}", n.year(), n.month(), n.day(), n.hour(), n.minute());
        let us = n.nanosecond() / 1000;
        if n.second() != 0 || us != 0 {
            s.push_str(&format!(", {}", n.second()));
        }
        if us != 0 {
            s.push_str(&format!(", {us}"));
        }
        if let Some(o) = d.offset {
            if o.local_minus_utc() == 0 {
                s.push_str(", tzinfo=datetime.timezone.utc");
            } else {
                s.push_str(&format!(", tzinfo=datetime.timezone(datetime.timedelta(seconds={}))", o.local_minus_utc()));
            }
        }
        s.push(')');
        return Ok(s);
    }
    if v.downcast_object_ref::<MatchingModel>().is_some() {
        return Ok("<documents.templating.filepath.MatchingModelContext object>".into());
    }
    match v.kind() {
        ValueKind::String => Ok(py_str_repr(v.as_str().unwrap_or(""))),
        ValueKind::Seq | ValueKind::Iterable => {
            let mut parts = Vec::new();
            let mut total = 0usize;
            for item in v.try_iter()? {
                let r = py_repr(&item)?;
                total += r.len() + 2;
                check_len(total)?;
                parts.push(r);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        ValueKind::Map => {
            let mut parts = Vec::new();
            let mut total = 0usize;
            for k in v.try_iter()? {
                let item = v.get_item(&k)?;
                let r = format!("{}: {}", py_repr(&k)?, py_repr(&item)?);
                total += r.len() + 2;
                check_len(total)?;
                parts.push(r);
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        _ => py_str(v),
    }
}

/// String of a str-like value (str or PlaceholderString).
pub fn as_pystr(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if is_placeholder(v) {
        return Some("-none-".into());
    }
    None
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum Num {
    Int(i128),
    Float(f64),
}

/// Python number (bool counts as int).
pub fn as_num(v: &Value) -> Option<Num> {
    match v.kind() {
        ValueKind::Bool => Some(Num::Int(i128::from(v.is_true()))),
        ValueKind::Number => {
            if v.is_integer() {
                i128::try_from(v.clone()).ok().map(Num::Int)
            } else {
                f64::try_from(v.clone()).ok().map(Num::Float)
            }
        }
        _ => None,
    }
}

fn num_value(n: Num) -> Value {
    match n {
        Num::Int(i) => {
            if let Ok(x) = i64::try_from(i) {
                Value::from(x)
            } else {
                Value::from(i)
            }
        }
        Num::Float(f) => Value::from(f),
    }
}

fn num_f(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

// ---------------------------------------------------------------------------
// Operators (called via the source rewriting in env.rs)
// ---------------------------------------------------------------------------

fn check_defined(a: &Value, b: &Value) -> Result<(), Error> {
    if a.is_undefined() || b.is_undefined() { Err(undefined_error()) } else { Ok(()) }
}

/// Python `a == b` (incl. `PlaceholderString.__eq__`).
pub fn py_eq(a: &Value, b: &Value) -> Result<bool, Error> {
    check_defined(a, b)?;
    Ok(py_eq_inner(a, b))
}

pub(crate) fn py_eq_inner(a: &Value, b: &Value) -> bool {
    let pa = is_placeholder(a);
    let pb = is_placeholder(b);
    if pa || pb {
        if pa && pb {
            return true;
        }
        let other = if pa { b } else { a };
        return matches!(other.as_str(), Some("none") | Some("-none-"));
    }
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
        return match (x, y) {
            (Num::Int(x), Num::Int(y)) => x == y,
            _ => num_f(x) == num_f(y),
        };
    }
    match (a.kind(), b.kind()) {
        (ValueKind::Seq, ValueKind::Seq) => {
            let (Ok(ia), Ok(ib)) = (a.try_iter(), b.try_iter()) else {
                return false;
            };
            let va: Vec<Value> = ia.collect();
            let vb: Vec<Value> = ib.collect();
            va.len() == vb.len() && va.iter().zip(vb.iter()).all(|(x, y)| py_eq_inner(x, y))
        }
        (ValueKind::Map, ValueKind::Map) => {
            let (Ok(ka), Some(la), Some(lb)) = (a.try_iter(), a.len(), b.len()) else {
                return a == b;
            };
            la == lb
                && ka.into_iter().all(|k| match (a.get_item(&k), b.get_item_opt(&k)) {
                    (Ok(x), Some(y)) => py_eq_inner(&x, &y),
                    _ => false,
                })
        }
        _ => a == b,
    }
}

/// Python `a + b`.
pub fn py_add(a: &Value, b: &Value) -> Result<Value, Error> {
    check_defined(a, b)?;
    if let (Some(x), Some(y)) = (as_pystr(a), as_pystr(b)) {
        check_len(x.len() + y.len())?;
        return Ok(Value::from(x + &y));
    }
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
        return match (x, y) {
            (Num::Int(x), Num::Int(y)) => x.checked_add(y).map(|r| num_value(Num::Int(r))).ok_or_else(|| invalid("integer overflow")),
            _ => Ok(num_value(Num::Float(num_f(x) + num_f(y)))),
        };
    }
    if matches!(a.kind(), ValueKind::Seq) && matches!(b.kind(), ValueKind::Seq) {
        let n = a.len().unwrap_or(0) + b.len().unwrap_or(0);
        if n > MAX_SEQ_ITEMS {
            return Err(too_large());
        }
        let mut items: Vec<Value> = a.try_iter()?.collect();
        items.extend(b.try_iter()?);
        return Ok(Value::from(items));
    }
    Err(invalid(format!("unsupported operand type(s) for +: '{}' and '{}'", short_type(a), short_type(b))))
}

/// Jinja2 `a ~ b` = `str(a) + str(b)`.
pub fn py_concat(a: &Value, b: &Value) -> Result<Value, Error> {
    check_defined(a, b)?;
    let x = py_str(a)?;
    let y = py_str(b)?;
    check_len(x.len() + y.len())?;
    Ok(Value::from(x + &y))
}

/// Python `a * b`.
pub fn py_mul(a: &Value, b: &Value) -> Result<Value, Error> {
    check_defined(a, b)?;
    let rep = |s: &Value, n: &Value| -> Option<Result<Value, Error>> {
        let Some(Num::Int(k)) = as_num(n) else {
            return None;
        };
        if let Some(text) = as_pystr(s) {
            let k = k.max(0) as u128;
            if (text.len() as u128) * k > MAX_STRING_BYTES as u128 {
                return Some(Err(too_large()));
            }
            return Some(Ok(Value::from(text.repeat(k as usize))));
        }
        if matches!(s.kind(), ValueKind::Seq) {
            let k = k.max(0) as u128;
            let len = s.len().unwrap_or(0) as u128;
            if len * k > MAX_SEQ_ITEMS as u128 {
                return Some(Err(too_large()));
            }
            let items: Vec<Value> = match s.try_iter() {
                Ok(it) => it.collect(),
                Err(e) => return Some(Err(e)),
            };
            let mut out = Vec::with_capacity((len * k) as usize);
            for _ in 0..k {
                out.extend(items.iter().cloned());
            }
            return Some(Ok(Value::from(out)));
        }
        None
    };
    if let Some(r) = rep(a, b).or_else(|| rep(b, a)) {
        return r;
    }
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
        return match (x, y) {
            (Num::Int(x), Num::Int(y)) => x.checked_mul(y).map(|r| num_value(Num::Int(r))).ok_or_else(|| invalid("integer overflow")),
            _ => Ok(num_value(Num::Float(num_f(x) * num_f(y)))),
        };
    }
    Err(invalid(format!("unsupported operand type(s) for *: '{}' and '{}'", short_type(a), short_type(b))))
}

/// Python `a ** b` (int ** negative -> float).
pub fn py_pow(a: &Value, b: &Value) -> Result<Value, Error> {
    check_defined(a, b)?;
    match (as_num(a), as_num(b)) {
        (Some(Num::Int(x)), Some(Num::Int(y))) if y >= 0 => {
            let y = u32::try_from(y).map_err(|_| too_large())?;
            x.checked_pow(y).map(|r| num_value(Num::Int(r))).ok_or_else(|| invalid("integer overflow"))
        }
        (Some(x), Some(y)) => {
            let (x, y) = (num_f(x), num_f(y));
            if x == 0.0 && y < 0.0 {
                return Err(invalid("0.0 cannot be raised to a negative power"));
            }
            Ok(num_value(Num::Float(x.powf(y))))
        }
        _ => Err(invalid(format!("unsupported operand type(s) for ** or pow(): '{}' and '{}'", short_type(a), short_type(b)))),
    }
}

fn short_type(v: &Value) -> &'static str {
    let t = py_type(v);
    // "<class 'int'>" -> "int"
    let t = t.trim_start_matches("<class '").trim_end_matches("'>");
    t.rsplit('.').next().unwrap_or(t)
}

/// Python `a % b`: `str % args` (printf formatting; sequence = tuple, mapping = keys) or
/// number modulo with the sign of the divisor (W9).
pub fn py_mod(a: &Value, b: &Value) -> Result<Value, Error> {
    check_defined(a, b)?;
    if let Some(fmt) = as_pystr(a) {
        let mut args: Vec<Value> = Vec::new();
        match b.kind() {
            ValueKind::Seq => args.extend(b.try_iter()?),
            ValueKind::Map => {
                args.push(b.clone());
                let mut kw: Vec<(String, Value)> = Vec::new();
                for k in b.try_iter()? {
                    let v = b.get_item(&k)?;
                    kw.push((py_str(&k)?, v));
                }
                args.push(Value::from(minijinja::value::Kwargs::from_iter(kw)));
            }
            _ => args.push(b.clone()),
        }
        let pa = crate::methods::PyArgs::new(&args)?;
        let out = crate::filters::percent_format(&fmt, &pa)?;
        check_len(out.len())?;
        return Ok(Value::from(out));
    }
    match (as_num(a), as_num(b)) {
        (Some(Num::Int(x)), Some(Num::Int(y))) => {
            if y == 0 {
                return Err(invalid("integer modulo by zero"));
            }
            // Python: result has the sign of the divisor
            let r = x % y;
            Ok(num_value(Num::Int(if r != 0 && ((r < 0) != (y < 0)) { r + y } else { r })))
        }
        (Some(x), Some(y)) => {
            let (x, y) = (num_f(x), num_f(y));
            if y == 0.0 {
                return Err(invalid("float modulo"));
            }
            let mut r = x % y;
            if r != 0.0 && ((r < 0.0) != (y < 0.0)) {
                r += y;
            }
            Ok(num_value(Num::Float(r)))
        }
        _ => Err(invalid(format!("unsupported operand type(s) for %: '{}' and '{}'", short_type(a), short_type(b)))),
    }
}

/// Python `strftime` on `date`/`datetime` objects. `None` if not a date value.
pub fn strftime_value(v: &Value, fmt: &str) -> Option<String> {
    if let Some(d) = v.downcast_object_ref::<PyDate>() {
        let n = d.0.and_time(chrono::NaiveTime::MIN);
        return Some(strftime(&n, TzInfo::Naive, fmt));
    }
    if let Some(d) = v.downcast_object_ref::<PyDateTime>() {
        return Some(strftime(&d.naive, d.tzinfo(), fmt));
    }
    None
}

trait GetItemOpt {
    fn get_item_opt(&self, k: &Value) -> Option<Value>;
}

impl GetItemOpt for Value {
    fn get_item_opt(&self, k: &Value) -> Option<Value> {
        match self.get_item(k) {
            Ok(v) if !v.is_undefined() => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_eq() {
        let p = placeholder();
        assert!(py_eq(&p, &Value::from("none")).unwrap());
        assert!(py_eq(&Value::from("-none-"), &p).unwrap());
        assert!(!py_eq(&p, &Value::from("x")).unwrap());
        assert!(!p.is_true());
        assert_eq!(py_str(&p).unwrap(), "-none-");
    }

    #[test]
    fn str_repr() {
        assert_eq!(py_str(&Value::from(())).unwrap(), "None");
        assert_eq!(py_str(&Value::from(true)).unwrap(), "True");
        assert_eq!(py_str(&Value::from(1.0)).unwrap(), "1.0");
        let l = Value::from(vec![Value::from("a"), Value::from(1)]);
        assert_eq!(py_str(&l).unwrap(), "['a', 1]");
        let d = date_value(NaiveDate::from_ymd_opt(2020, 6, 25).unwrap());
        assert_eq!(py_str(&d).unwrap(), "2020-06-25");
    }

    #[test]
    fn ops() {
        assert!(py_eq(&Value::from(1), &Value::from(1.0)).unwrap());
        assert!(py_eq(&Value::from(true), &Value::from(1)).unwrap());
        assert!(!py_eq(&Value::from("1"), &Value::from(1)).unwrap());
        assert_eq!(py_concat(&Value::from(()), &Value::from("x")).unwrap().as_str(), Some("Nonex"));
        assert_eq!(py_mul(&Value::from("ab"), &Value::from(3)).unwrap().as_str(), Some("ababab"));
        assert!(py_mul(&Value::from("ab"), &Value::from(100_000_000_000i64)).is_err());
        assert_eq!(py_add(&placeholder(), &Value::from("/x")).unwrap().as_str(), Some("-none-/x"));
    }
}
