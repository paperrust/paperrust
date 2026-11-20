//! Filters of the template environment.
//!
//! upstream: src/documents/templating/filters.py@v3.4.2 (localize_date, format_datetime,
//! get_cf_value), src/documents/templating/filepath.py@v3.4.2 (registration incl. Django
//! `slugify`), jinja2/filters.py@3.1.6 (built-in filters whose minijinja counterpart differs).

use minijinja::value::{Rest, Value, ValueKind};
use minijinja::{Error, State};

use crate::localize::{format_localized, load_locale, DateValue};
use crate::methods::{char_slice, py_capitalize, py_replace, PyArgs};
use crate::pystr::{django_slugify, parse_date, parse_datetime, py_isspace, py_strip, py_strip_chars, py_str_repr};
use crate::pyvalue::{
    as_num, as_pystr, check_len, invalid, is_placeholder, py_repr, py_str, py_type, strftime_value, Num, PyDate,
    PyDateTime,
};
use crate::strftime::{strftime, TzInfo};

/// Names of the built-in Jinja2 filters (3.1.6) – authoritative for the "No filter named" check.
pub const JINJA2_FILTERS: &[&str] = &[
    "abs", "attr", "batch", "capitalize", "center", "count", "d", "default", "dictsort", "e", "escape",
    "filesizeformat", "first", "float", "forceescape", "format", "groupby", "indent", "int", "items", "join",
    "last", "length", "list", "lower", "map", "max", "min", "pprint", "random", "reject", "rejectattr",
    "replace", "reverse", "round", "safe", "select", "selectattr", "slice", "sort", "string", "striptags", "sum",
    "title", "tojson", "trim", "truncate", "unique", "upper", "urlencode", "urlize", "wordcount", "wordwrap",
    "xmlattr",
];

/// Filters registered by upstream.
pub const PAPERLESS_FILTERS: &[&str] = &["get_cf_value", "datetime", "slugify", "localize_date"];

/// Jinja2-Tests (3.1.6).
pub const JINJA2_TESTS: &[&str] = &[
    "boolean", "callable", "defined", "divisibleby", "eq", "equalto", "escaped", "even", "false", "filter",
    "float", "ge", "gt", "greaterthan", "in", "integer", "iterable", "le", "lower", "lessthan", "lt", "mapping",
    "ne", "none", "number", "odd", "sameas", "sequence", "string", "test", "true", "undefined", "upper", "==",
    "!=", "<", "<=", ">", ">=",
];

/// minijinja filters/tests that do not exist in Jinja2 (are removed).
pub const MINIJINJA_ONLY_FILTERS: &[&str] = &["lines", "bool", "chain", "zip", "split", "json"];
pub const MINIJINJA_ONLY_TESTS: &[&str] = &["startingwith", "endingwith", "safe"];

// ---------------------------------------------------------------------------
// upstream-Filter
// ---------------------------------------------------------------------------

/// `format_datetime(value, format)` (Filter `datetime`).
pub fn f_datetime(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("format_datetime", &["format"])?;
    let fmt = args
        .get(0, "format")
        .ok_or_else(|| invalid("format_datetime() missing 1 required positional argument: 'format'"))?;
    let fmt = py_str(&fmt)?;
    if value.as_str().is_some() || is_placeholder(&value) {
        let s = as_pystr(&value).unwrap_or_default();
        let parsed = match parse_datetime(&s).map_err(invalid)? {
            Some(dt) => Some(strftime(&dt.naive, dt.offset.map(TzInfo::Fixed).unwrap_or(TzInfo::Naive), &fmt)),
            None => parse_date(&s)
                .map_err(invalid)?
                .map(|d| strftime(&d.and_time(chrono::NaiveTime::MIN), TzInfo::Naive, &fmt)),
        };
        return match parsed {
            Some(r) => Ok(Value::from(r)),
            None => Err(invalid(format!("Invalid date or datetime string: {}", py_str_repr(&s)))),
        };
    }
    if value.is_undefined() {
        return Err(Error::from(minijinja::ErrorKind::UndefinedError));
    }
    match strftime_value(&value, &fmt) {
        Some(r) => Ok(Value::from(r)),
        None => Err(invalid(format!("'{}' object has no attribute 'strftime'", short(&value)))),
    }
}

fn short(v: &Value) -> String {
    py_type(v).trim_start_matches("<class '").trim_end_matches("'>").rsplit('.').next().unwrap_or("").to_string()
}

/// Django `slugify`.
pub fn f_slugify(value: Value) -> Result<Value, Error> {
    Ok(Value::from(django_slugify(&py_str(&value)?)))
}

/// `localize_date(value, format, locale)`.
pub fn f_localize_date(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("localize_date", &["format", "locale"])?;
    let format = args
        .get(0, "format")
        .ok_or_else(|| invalid("localize_date() missing 2 required positional arguments: 'format' and 'locale'"))?;
    let locale = args
        .get(1, "locale")
        .ok_or_else(|| invalid("localize_date() missing 1 required positional argument: 'locale'"))?;
    if value.is_undefined() {
        return Err(Error::from(minijinja::ErrorKind::UndefinedError));
    }
    // if isinstance(value, str): value = parse_datetime(value)
    let mut parsed: Option<DateValue> = None;
    let mut is_none = false;
    if value.as_str().is_some() || is_placeholder(&value) {
        let s = as_pystr(&value).unwrap_or_default();
        match parse_datetime(&s).map_err(invalid)? {
            Some(dt) => {
                parsed = Some(DateValue::DateTime(
                    dt.naive,
                    dt.offset.unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc)),
                ))
            }
            None => is_none = true,
        }
    }
    let locale_str = py_str(&locale)?;
    let loc = load_locale(&locale_str).ok_or_else(|| invalid(format!("Invalid locale identifier: {locale_str}")))?;
    let format = py_str(&format)?;
    let dv = if let Some(p) = parsed {
        p
    } else if is_none {
        return Err(invalid("Unsupported type <class 'NoneType'> for localize_date"));
    } else if let Some(d) = value.downcast_object_ref::<PyDateTime>() {
        DateValue::DateTime(d.naive, d.offset.unwrap_or_else(|| chrono::Offset::fix(&chrono::Utc)))
    } else if let Some(d) = value.downcast_object_ref::<PyDate>() {
        DateValue::Date(d.0)
    } else {
        return Err(invalid(format!("Unsupported type {} for localize_date", py_type(&value))));
    };
    format_localized(dv, &format, &loc).map(Value::from).map_err(invalid)
}

/// `get_cf_value(custom_field_data, name, default=None)`.
pub fn f_get_cf_value(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("get_cf_value", &["name", "default"])?;
    let name = args
        .get(0, "name")
        .ok_or_else(|| invalid("get_cf_value() missing 1 required positional argument: 'name'"))?;
    let default = args.get(1, "default").unwrap_or(Value::from(()));
    if value.is_undefined() {
        return Err(Error::from(minijinja::ErrorKind::UndefinedError));
    }
    // if name in custom_field_data and custom_field_data[name]["value"] is not None
    let contains = match value.kind() {
        ValueKind::Map => !value.get_item(&name)?.is_undefined(),
        ValueKind::String => match (value.as_str(), name.as_str()) {
            (Some(h), Some(n)) => h.contains(n),
            _ => return Err(invalid("'in <string>' requires string as left operand")),
        },
        ValueKind::Seq => value.try_iter()?.any(|x| crate::pyvalue::py_eq_inner(&x, &name)),
        _ => return Err(invalid(format!("argument of type '{}' is not iterable", short(&value)))),
    };
    if contains {
        let entry = value.get_item(&name)?;
        if !matches!(value.kind(), ValueKind::Map) {
            return Err(invalid("string indices must be integers, not 'str'"));
        }
        let v = entry.get_item(&Value::from("value"))?;
        if v.is_undefined() {
            return Err(invalid("KeyError: 'value'"));
        }
        if !v.is_none() {
            return Ok(v);
        }
    }
    if !default.is_none() {
        return Ok(default);
    }
    Ok(Value::from(()))
}

// ---------------------------------------------------------------------------
// Jinja2 filters with Python semantics
// ---------------------------------------------------------------------------

pub fn f_string(value: Value) -> Result<Value, Error> {
    if value.is_safe() {
        return Ok(value);
    }
    Ok(Value::from(py_str(&value)?))
}

pub fn f_lower(value: Value) -> Result<Value, Error> {
    Ok(Value::from(py_str(&value)?.to_lowercase()))
}

pub fn f_upper(value: Value) -> Result<Value, Error> {
    Ok(Value::from(py_str(&value)?.to_uppercase()))
}

pub fn f_capitalize(value: Value) -> Result<Value, Error> {
    Ok(Value::from(py_capitalize(&py_str(&value)?)))
}

/// Jinja2 `do_title`: word starts after `[-\s({\[<]+`.
pub fn f_title(value: Value) -> Result<Value, Error> {
    let s = py_str(&value)?;
    let is_sep = |c: char| c == '-' || py_isspace(c) || matches!(c, '(' | '{' | '[' | '<');
    let mut out = String::with_capacity(s.len());
    let mut at_start = true;
    for c in s.chars() {
        if is_sep(c) {
            out.push(c);
            at_start = true;
        } else if at_start {
            out.extend(c.to_uppercase());
            at_start = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    Ok(Value::from(out))
}

pub fn f_replace(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_replace", &["old", "new", "count"])?;
    let old = py_str(&args.get(0, "old").ok_or_else(|| invalid("do_replace() missing argument 'old'"))?)?;
    let new = py_str(&args.get(1, "new").ok_or_else(|| invalid("do_replace() missing argument 'new'"))?)?;
    let count = match args.get(2, "count") {
        Some(c) if !c.is_none() => match as_num(&c) {
            Some(Num::Int(i)) => i as i64,
            _ => return Err(invalid("'float' object cannot be interpreted as an integer")),
        },
        _ => -1,
    };
    Ok(Value::from(py_replace(&py_str(&value)?, &old, &new, count)?))
}

pub fn f_trim(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_trim", &["chars"])?;
    let s = py_str(&value)?;
    Ok(Value::from(match args.get(0, "chars").filter(|c| !c.is_none()) {
        Some(c) => py_strip_chars(&s, &py_str(&c)?).to_string(),
        None => py_strip(&s).to_string(),
    }))
}

pub fn f_center(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_center", &["width"])?;
    let width = match args.get(0, "width") {
        Some(w) => match as_num(&w) {
            Some(Num::Int(i)) => i as i64,
            _ => return Err(invalid("integer argument expected")),
        },
        None => 80,
    };
    let s = py_str(&value)?;
    let len = s.chars().count() as i64;
    if width <= len {
        return Ok(Value::from(s));
    }
    check_len(width as usize)?;
    let total = (width - len) as usize;
    let left = total / 2 + (total & (width as usize) & 1);
    Ok(Value::from(format!("{}{s}{}", " ".repeat(left), " ".repeat(total - left))))
}

pub fn f_join(state: &State, value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_join", &["d", "attribute"])?;
    let sep = match args.get(0, "d") {
        Some(d) => py_str(&d)?,
        None => String::new(),
    };
    let attribute = args.get(1, "attribute").filter(|a| !a.is_none());
    let mut out = String::new();
    for (i, item) in value.try_iter()?.enumerate() {
        let item = match &attribute {
            Some(a) => attr_lookup(state, &item, a)?,
            None => item,
        };
        if i > 0 {
            out.push_str(&sep);
        }
        out.push_str(&py_str(&item)?);
        check_len(out.len())?;
    }
    Ok(Value::from(out))
}

fn attr_lookup(_state: &State, item: &Value, attr: &Value) -> Result<Value, Error> {
    let mut cur = item.clone();
    match attr.as_str() {
        Some(path) => {
            for part in path.split('.') {
                cur = match part.parse::<usize>() {
                    Ok(idx) => cur.get_item(&Value::from(idx))?,
                    Err(_) => cur.get_attr(part)?,
                };
            }
            Ok(cur)
        }
        None => cur.get_item(attr),
    }
}

pub fn f_truncate(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_truncate", &["length", "killwords", "end", "leeway"])?;
    let int = |v: Option<Value>, d: i64| -> Result<i64, Error> {
        match v {
            Some(v) => match as_num(&v) {
                Some(Num::Int(i)) => Ok(i as i64),
                _ => Err(invalid("integer argument expected")),
            },
            None => Ok(d),
        }
    };
    let length = int(args.get(0, "length"), 255)?;
    let killwords = args.get(1, "killwords").is_some_and(|v| v.is_true());
    let end = match args.get(2, "end") {
        Some(e) => py_str(&e)?,
        None => "...".into(),
    };
    let leeway = int(args.get(3, "leeway").filter(|v| !v.is_none()), 5)?;
    let end_len = end.chars().count() as i64;
    if length < end_len {
        return Err(invalid(format!("expected length >= {end_len}, got {length}")));
    }
    if leeway < 0 {
        return Err(invalid(format!("expected leeway >= 0, got {leeway}")));
    }
    let s = py_str(&value)?;
    let len = s.chars().count() as i64;
    if len <= length + leeway {
        return Ok(Value::from(s));
    }
    let head = char_slice(&s, 0, (length - end_len) as usize);
    if killwords {
        return Ok(Value::from(format!("{head}{end}")));
    }
    let result = match head.rfind(' ') {
        Some(i) => &head[..i],
        None => head,
    };
    Ok(Value::from(format!("{result}{end}")))
}

pub fn f_wordcount(value: Value) -> Result<Value, Error> {
    let s = py_str(&value)?;
    let mut n = 0i64;
    let mut in_word = false;
    for c in s.chars() {
        let w = c.is_alphanumeric() || c == '_';
        if w && !in_word {
            n += 1;
        }
        in_word = w;
    }
    Ok(Value::from(n))
}

/// Jinja2 `do_int(value, default=0, base=10)`.
pub fn f_int(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_int", &["default", "base"])?;
    let default = args.get(0, "default").unwrap_or(Value::from(0));
    let base = match args.get(1, "base") {
        Some(b) => match as_num(&b) {
            Some(Num::Int(i)) => i as u32,
            _ => 10,
        },
        None => 10,
    };
    if value.is_undefined() {
        return Err(Error::from(minijinja::ErrorKind::UndefinedError));
    }
    if let Some(s) = value.as_str() {
        let t = py_strip(s).replace('_', "");
        let (neg, digits) = match t.strip_prefix('-') {
            Some(r) => (true, r.to_string()),
            None => (false, t.trim_start_matches('+').to_string()),
        };
        let digits = if base == 16 {
            digits.trim_start_matches("0x").trim_start_matches("0X").to_string()
        } else if base == 8 {
            digits.trim_start_matches("0o").trim_start_matches("0O").to_string()
        } else if base == 2 {
            digits.trim_start_matches("0b").trim_start_matches("0B").to_string()
        } else {
            digits
        };
        if (2..=36).contains(&base) && !digits.is_empty()
            && let Ok(i) = i128::from_str_radix(&digits, base) {
                return Ok(Value::from(if neg { -i } else { i }));
            }
        if let Ok(f) = py_strip(s).parse::<f64>()
            && f.is_finite() {
                return Ok(Value::from(f.trunc() as i128));
            }
        return Ok(default);
    }
    match as_num(&value) {
        Some(Num::Int(i)) => Ok(Value::from(i)),
        Some(Num::Float(f)) if f.is_finite() => Ok(Value::from(f.trunc() as i128)),
        _ => Ok(default),
    }
}

/// Jinja2 `do_float(value, default=0.0)`.
pub fn f_float(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_float", &["default"])?;
    let default = args.get(0, "default").unwrap_or(Value::from(0.0));
    if value.is_undefined() {
        return Err(Error::from(minijinja::ErrorKind::UndefinedError));
    }
    if let Some(s) = value.as_str() {
        let t = py_strip(s).replace('_', "").to_lowercase();
        let t = match t.as_str() {
            "infinity" | "+infinity" => "inf".to_string(),
            "-infinity" => "-inf".to_string(),
            _ => t,
        };
        return Ok(t.parse::<f64>().map(Value::from).unwrap_or(default));
    }
    match as_num(&value) {
        Some(Num::Int(i)) => Ok(Value::from(i as f64)),
        Some(Num::Float(f)) => Ok(Value::from(f)),
        _ => Ok(default),
    }
}

/// Jinja2 `do_round(value, precision=0, method="common")`.
pub fn f_round(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_round", &["precision", "method"])?;
    let precision = match args.get(0, "precision") {
        Some(p) => match as_num(&p) {
            Some(Num::Int(i)) => i as i32,
            _ => return Err(invalid("'float' object cannot be interpreted as an integer")),
        },
        None => 0,
    };
    let method = match args.get(1, "method") {
        Some(m) => py_str(&m)?,
        None => "common".into(),
    };
    if !matches!(method.as_str(), "common" | "ceil" | "floor") {
        return Err(invalid("method must be common, ceil or floor"));
    }
    let n = as_num(&value).ok_or_else(|| invalid(format!("type {} doesn't define __round__ method", short(&value))))?;
    let p = 10f64.powi(precision);
    match (method.as_str(), n) {
        ("common", Num::Int(i)) => {
            if precision >= 0 {
                Ok(Value::from(i))
            } else {
                let q = 10i128.pow((-precision) as u32);
                let r = ((i as f64 / q as f64).round_ties_even() as i128) * q;
                Ok(Value::from(r))
            }
        }
        ("common", Num::Float(f)) => Ok(Value::from((f * p).round_ties_even() / p)),
        ("ceil", n) => Ok(Value::from((num_f(n) * p).ceil() / p)),
        (_, n) => Ok(Value::from((num_f(n) * p).floor() / p)),
    }
}

fn num_f(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Float(f) => f,
    }
}

/// `length`/`count`: Python `len()` (also for PlaceholderString).
pub fn f_length(value: Value) -> Result<Value, Error> {
    if let Some(s) = as_pystr(&value) {
        return Ok(Value::from(s.chars().count() as i64));
    }
    minijinja::filters::length(&value).map(|n| Value::from(n as i64))
}

/// Jinja2 `do_format(value, *args, **kwargs)` = `str(value) % args`.
pub fn f_format(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    if !args.positional().is_empty() && !args.kwargs().is_empty() {
        return Err(invalid("can't handle positional and keyword arguments at the same time"));
    }
    let fmt = py_str(&value)?;
    Ok(Value::from(percent_format(&fmt, &args)?))
}

/// Upper limit for `*` width/precision (protection against huge padding; output is limited anyway).
const MAX_PAD: usize = 1_000_000;

/// Python `str % args` (common subset: s r d i u f F e E x X o c %, width/precision also `*`).
pub fn percent_format(fmt: &str, args: &PyArgs) -> Result<String, Error> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut idx = 0;
    let pos = args.positional();
    while i < chars.len() {
        let c = chars[i];
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            return Err(invalid("incomplete format"));
        }
        let mut key: Option<String> = None;
        if chars[i] == '(' {
            let end = chars[i..].iter().position(|c| *c == ')').ok_or_else(|| invalid("incomplete format key"))?;
            key = Some(chars[i + 1..i + end].iter().collect());
            i += end + 1;
        }
        let mut flags = String::new();
        while i < chars.len() && "-+ #0".contains(chars[i]) {
            flags.push(chars[i]);
            i += 1;
        }
        // `*` takes width or precision from the next positional argument (Python `%*d`, `%.*f`).
        let star = |idx: &mut usize| -> Result<i128, Error> {
            if key.is_some() {
                return Err(invalid("* wants int"));
            }
            let v = pos.get(*idx).cloned().ok_or_else(|| invalid("not enough arguments for format string"))?;
            *idx += 1;
            match as_num(&v) {
                Some(Num::Int(n)) => Ok(n),
                _ => Err(invalid("* wants int")),
            }
        };
        let mut width = String::new();
        if i < chars.len() && chars[i] == '*' {
            i += 1;
            let n = star(&mut idx)?;
            if n < 0 {
                // Python: negative width = left-aligned
                flags.push('-');
            }
            let n = n.unsigned_abs().min(MAX_PAD as u128);
            width = n.to_string();
        } else {
            while i < chars.len() && chars[i].is_ascii_digit() {
                width.push(chars[i]);
                i += 1;
            }
        }
        let mut prec: Option<String> = None;
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let mut p = String::new();
            if i < chars.len() && chars[i] == '*' {
                i += 1;
                let n = star(&mut idx)?.clamp(0, MAX_PAD as i128);
                p = n.to_string();
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    p.push(chars[i]);
                    i += 1;
                }
            }
            prec = Some(p);
        }
        if i >= chars.len() {
            return Err(invalid("incomplete format"));
        }
        let conv = chars[i];
        i += 1;
        if conv == '%' {
            out.push('%');
            continue;
        }
        let value = match &key {
            Some(k) => args
                .kwargs()
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| invalid(format!("'{k}'")))?,
            None => {
                let v = pos.get(idx).cloned().ok_or_else(|| invalid("not enough arguments for format string"))?;
                idx += 1;
                v
            }
        };
        let mut spec = String::new();
        if flags.contains('-') {
            spec.push('<');
        } else if !flags.contains('0') || matches!(conv, 's' | 'r' | 'c') {
            spec.push('>');
        }
        if flags.contains('+') {
            spec.push('+');
        } else if flags.contains(' ') {
            spec.push(' ');
        }
        if flags.contains('#') {
            spec.push('#');
        }
        if flags.contains('0') && !flags.contains('-') && !matches!(conv, 's' | 'r' | 'c') {
            spec.push('0');
        }
        spec.push_str(&width);
        let piece = match conv {
            's' | 'r' | 'a' => {
                let s = if conv == 's' { py_str(&value)? } else { py_repr(&value)? };
                if let Some(p) = &prec {
                    spec.push('.');
                    spec.push_str(if p.is_empty() { "0" } else { p });
                }
                crate::methods::py_format_value(&Value::from(s), &spec)?
            }
            'd' | 'i' | 'u' => {
                let n = match as_num(&value) {
                    Some(Num::Int(i)) => Value::from(i),
                    Some(Num::Float(f)) => Value::from(f.trunc() as i128),
                    None => return Err(invalid(format!("%{conv} format: a real number is required, not {}", short(&value)))),
                };
                spec.push('d');
                crate::methods::py_format_value(&n, &spec)?
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'x' | 'X' | 'o' => {
                if as_num(&value).is_none() {
                    return Err(invalid(format!("must be real number, not {}", short(&value))));
                }
                if let Some(p) = &prec {
                    spec.push('.');
                    spec.push_str(if p.is_empty() { "0" } else { p });
                }
                spec.push(conv);
                crate::methods::py_format_value(&value, &spec)?
            }
            'c' => {
                let ch = match (as_num(&value), as_pystr(&value)) {
                    (Some(Num::Int(i)), _) => char::from_u32(i as u32).map(String::from).unwrap_or_default(),
                    (_, Some(s)) if s.chars().count() == 1 => s,
                    _ => return Err(invalid("%c requires int or char")),
                };
                crate::methods::py_format_value(&Value::from(ch), &spec)?
            }
            other => {
                return Err(invalid(format!("unsupported format character '{other}' (0x{:x})", other as u32)));
            }
        };
        out.push_str(&piece);
        check_len(out.len())?;
    }
    if key_free(fmt) && idx < pos.len() {
        return Err(invalid("not all arguments converted during string formatting"));
    }
    Ok(out)
}

fn key_free(fmt: &str) -> bool {
    !fmt.contains("%(")
}

/// Jinja2 `do_filesizeformat(value, binary=False)`.
pub fn f_filesizeformat(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_filesizeformat", &["binary"])?;
    let binary = args.get(0, "binary").is_some_and(|b| b.is_true());
    let bytes = match as_num(&value) {
        Some(n) => num_f(n),
        None => py_str(&value)?.trim().parse::<f64>().map_err(|_| invalid("could not convert string to float"))?,
    };
    let base: f64 = if binary { 1024.0 } else { 1000.0 };
    let prefixes = if binary {
        ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"]
    } else {
        ["kB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"]
    };
    if bytes == 1.0 {
        return Ok(Value::from("1 Byte"));
    }
    if bytes < base {
        return Ok(Value::from(format!("{} Bytes", bytes as i64)));
    }
    let mut unit = base;
    let mut prefix = prefixes[0];
    for (i, p) in prefixes.iter().enumerate() {
        unit = base.powi(i as i32 + 2);
        prefix = p;
        if bytes < unit {
            break;
        }
    }
    Ok(Value::from(format!("{:.1} {prefix}", base * bytes / unit)))
}

/// Jinja2 `do_striptags`: remove tags/comments, collapse whitespace.
pub fn f_striptags(value: Value) -> Result<Value, Error> {
    let s = py_str(&value)?;
    let mut out = String::new();
    let mut rest = s.as_str();
    loop {
        let next_c = rest.find("<!--");
        let next_t = rest.find('<');
        match (next_c, next_t) {
            (Some(c), Some(t)) if c <= t => {
                out.push_str(&rest[..c]);
                match rest[c..].find("-->") {
                    Some(e) => rest = &rest[c + e + 3..],
                    None => {
                        out.push_str(&rest[c..]);
                        break;
                    }
                }
            }
            (_, Some(t)) => {
                out.push_str(&rest[..t]);
                match rest[t..].find('>') {
                    Some(e) if e > 1 => rest = &rest[t + e + 1..],
                    _ => {
                        out.push('<');
                        rest = &rest[t + 1..];
                    }
                }
            }
            _ => {
                out.push_str(rest);
                break;
            }
        }
    }
    let collapsed: Vec<&str> = out.split(py_isspace).filter(|x| !x.is_empty()).collect();
    let joined = collapsed.join(" ");
    Ok(Value::from(
        joined.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&"),
    ))
}

/// Jinja2-Test `callable`.
pub fn t_callable(value: Value) -> bool {
    matches!(value.kind(), ValueKind::Plain) && value.downcast_object_ref::<crate::pyvalue::MatchingModel>().is_none() && {
        let s = format!("{value:?}");
        s.contains("function") || s.contains("macro")
    }
}

/// Jinja2 tests `eq`/`ne` with Python `==`.
pub fn t_eq(value: Value, other: Value) -> Result<bool, Error> {
    crate::pyvalue::py_eq(&value, &other)
}

pub fn t_ne(value: Value, other: Value) -> Result<bool, Error> {
    crate::pyvalue::py_eq(&value, &other).map(|b| !b)
}
