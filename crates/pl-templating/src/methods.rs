//! Python methods on values as the Jinja2 sandbox allows them (`title.upper()`,
//! `custom_fields.items()`, `created.strftime('%Y')`, `"{:04d}".format(42)` ...).
//!
//! upstream: jinja2/sandbox.py@3.1.6 (SandboxedEnvironment: str/dict/list/date methods are
//! callable; `str.format` goes through the SandboxedFormatter; `save`/`delete`/`update` are blocked in
//! src/documents/templating/environment.py@v3.4.2).

use minijinja::value::{Value, ValueKind};
use minijinja::{Error, ErrorKind, State};

use crate::pystr::{py_isspace, py_strip, py_strip_chars};
use crate::pyvalue::{
    as_num, as_pystr, check_len, date_iso, invalid, py_eq_inner, py_repr, py_str, security_error, Num, PyDate,
    PyDateTime, MAX_SEQ_ITEMS,
};
use crate::strftime::{strftime, TzInfo};

/// Bind positional and keyword arguments the Python way.
pub(crate) struct PyArgs {
    pos: Vec<Value>,
    kw: Vec<(String, Value)>,
}

impl PyArgs {
    pub fn new(args: &[Value]) -> Result<PyArgs, Error> {
        let mut pos = Vec::new();
        let mut kw = Vec::new();
        for a in args {
            if a.is_kwargs() {
                for k in a.try_iter()? {
                    let v = a.get_item(&k)?;
                    kw.push((k.as_str().unwrap_or_default().to_string(), v));
                }
            } else {
                pos.push(a.clone());
            }
        }
        Ok(PyArgs { pos, kw })
    }

    /// Argument `idx` or keyword `name`.
    pub fn get(&self, idx: usize, name: &str) -> Option<Value> {
        self.pos.get(idx).cloned().or_else(|| self.kw.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    pub fn positional(&self) -> &[Value] {
        &self.pos
    }

    pub fn kwargs(&self) -> &[(String, Value)] {
        &self.kw
    }

    /// Checks that at most `names.len()` arguments and only known names were passed.
    pub fn check(&self, func: &str, names: &[&str]) -> Result<(), Error> {
        if self.pos.len() > names.len() {
            return Err(invalid(format!(
                "{func}() takes at most {} arguments ({} given)",
                names.len(),
                self.pos.len()
            )));
        }
        for (k, _) in &self.kw {
            if !names.contains(&k.as_str()) {
                return Err(invalid(format!("{func}() got an unexpected keyword argument '{k}'")));
            }
        }
        Ok(())
    }
}

fn req(args: &PyArgs, idx: usize, name: &str, func: &str) -> Result<Value, Error> {
    args.get(idx, name)
        .ok_or_else(|| invalid(format!("{func}() missing required argument '{name}' (pos {})", idx + 1)))
}

fn int_arg(v: &Value, func: &str) -> Result<i64, Error> {
    match as_num(v) {
        Some(Num::Int(i)) => i64::try_from(i).map_err(|_| invalid("Python int too large to convert to C ssize_t")),
        _ => Err(invalid(format!("{func}(): '{}' object cannot be interpreted as an integer", type_short(v)))),
    }
}

fn str_arg(v: &Value, func: &str) -> Result<String, Error> {
    as_pystr(v).ok_or_else(|| invalid(format!("{func}() argument must be str, not {}", type_short(v))))
}

fn type_short(v: &Value) -> String {
    let t = crate::pyvalue::py_type(v);
    t.trim_start_matches("<class '").trim_end_matches("'>").rsplit('.').next().unwrap_or("").to_string()
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn byte_index_of_char(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(s.len())
}

/// Python `str.title()`.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    out
}

/// Python `str.capitalize()`.
pub fn py_capitalize(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(f) => {
            let mut out: String = f.to_uppercase().collect();
            out.push_str(&it.as_str().to_lowercase());
            out
        }
        None => String::new(),
    }
}

/// Python `str.split(sep=None, maxsplit=-1)`.
pub fn py_split(s: &str, sep: Option<&str>, maxsplit: i64) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    match sep {
        Some("") => return Err(invalid("empty separator")),
        Some(sep) => {
            let mut rest = s;
            let mut n = 0;
            while maxsplit < 0 || n < maxsplit {
                match rest.find(sep) {
                    Some(i) => {
                        out.push(rest[..i].to_string());
                        rest = &rest[i + sep.len()..];
                        n += 1;
                    }
                    None => break,
                }
            }
            out.push(rest.to_string());
        }
        None => {
            let mut rest = s.trim_start_matches(py_isspace);
            let mut n = 0;
            while !rest.is_empty() {
                if maxsplit >= 0 && n >= maxsplit {
                    out.push(rest.trim_end_matches(py_isspace).to_string());
                    break;
                }
                let end = rest.find(py_isspace).unwrap_or(rest.len());
                out.push(rest[..end].to_string());
                rest = rest[end..].trim_start_matches(py_isspace);
                n += 1;
            }
        }
    }
    if out.len() > MAX_SEQ_ITEMS {
        return Err(crate::pyvalue::too_large());
    }
    Ok(out)
}

fn py_rsplit(s: &str, sep: Option<&str>, maxsplit: i64) -> Result<Vec<String>, Error> {
    if maxsplit < 0 {
        return py_split(s, sep, maxsplit);
    }
    let rev: String = s.chars().rev().collect();
    let rsep: Option<String> = sep.map(|x| x.chars().rev().collect());
    let mut parts = py_split(&rev, rsep.as_deref(), maxsplit)?;
    parts.reverse();
    Ok(parts.into_iter().map(|p| p.chars().rev().collect()).collect())
}

/// Python `str.replace(old, new, count=-1)` with size check.
pub fn py_replace(s: &str, old: &str, new: &str, count: i64) -> Result<String, Error> {
    let occurrences = if old.is_empty() { char_len(s) + 1 } else { s.matches(old).count() };
    let n = if count < 0 { occurrences } else { occurrences.min(count as usize) };
    let est = s.len() as u128 + (n as u128) * (new.len() as u128);
    if est > crate::pyvalue::MAX_STRING_BYTES as u128 {
        return Err(crate::pyvalue::too_large());
    }
    if old.is_empty() {
        let mut out = String::new();
        let mut done = 0;
        for c in s.chars() {
            if done < n {
                out.push_str(new);
                done += 1;
            }
            out.push(c);
        }
        if done < n {
            out.push_str(new);
        }
        return Ok(out);
    }
    Ok(s.replacen(old, new, n))
}

fn pad_to(s: &str, width: i64, fill: char, align: char) -> Result<String, Error> {
    let len = char_len(s) as i64;
    if width <= len {
        return Ok(s.to_string());
    }
    check_len(width as usize * fill.len_utf8() + s.len())?;
    let total = (width - len) as usize;
    let (left, right) = match align {
        '<' => (0, total),
        '>' => (total, 0),
        _ => {
            // CPython str.center: links = total/2 + (total & width & 1)
            let left = total / 2 + (total & (width as usize) & 1);
            (left, total - left)
        }
    };
    let f = fill.to_string();
    Ok(format!("{}{s}{}", f.repeat(left), f.repeat(right)))
}

fn fill_arg(args: &PyArgs, idx: usize) -> Result<char, Error> {
    match args.get(idx, "fillchar") {
        None => Ok(' '),
        Some(v) => {
            let s = str_arg(&v, "fill")?;
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(invalid("The fill character must be exactly one character long")),
            }
        }
    }
}

fn strs(v: Vec<String>) -> Value {
    Value::from(v.into_iter().map(Value::from).collect::<Vec<_>>())
}

fn char_find(s: &str, sub: &str, reverse: bool) -> i64 {
    let r = if reverse { s.rfind(sub) } else { s.find(sub) };
    match r {
        Some(b) => s[..b].chars().count() as i64,
        None => -1,
    }
}

fn affix_any(v: &Value, f: impl Fn(&str) -> bool) -> Result<bool, Error> {
    if let Some(s) = as_pystr(v) {
        return Ok(f(&s));
    }
    if matches!(v.kind(), ValueKind::Seq) {
        for item in v.try_iter()? {
            let s = str_arg(&item, "startswith")?;
            if f(&s) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Err(invalid("startswith first arg must be str or a tuple of str"))
}

fn str_method(s: &str, method: &str, args: &PyArgs) -> Result<Value, Error> {
    let a0 = || req(args, 0, "arg", method);
    Ok(match method {
        "upper" => Value::from(s.to_uppercase()),
        "lower" | "casefold" => Value::from(s.to_lowercase()),
        "title" => Value::from(py_title(s)),
        "capitalize" => Value::from(py_capitalize(s)),
        "swapcase" => Value::from(
            s.chars()
                .flat_map(|c| -> Vec<char> {
                    if c.is_uppercase() {
                        c.to_lowercase().collect()
                    } else {
                        c.to_uppercase().collect()
                    }
                })
                .collect::<String>(),
        ),
        "strip" | "lstrip" | "rstrip" => {
            let chars = args.get(0, "chars").filter(|v| !v.is_none());
            let r = match chars {
                None => match method {
                    "strip" => py_strip(s),
                    "lstrip" => s.trim_start_matches(py_isspace),
                    _ => s.trim_end_matches(py_isspace),
                },
                Some(c) => {
                    let c = str_arg(&c, method)?;
                    match method {
                        "strip" => py_strip_chars(s, &c),
                        "lstrip" => s.trim_start_matches(|x| c.contains(x)),
                        _ => s.trim_end_matches(|x| c.contains(x)),
                    }
                }
            };
            Value::from(r)
        }
        "replace" => {
            let old = str_arg(&a0()?, method)?;
            let new = str_arg(&req(args, 1, "new", method)?, method)?;
            let count = match args.get(2, "count") {
                Some(c) => int_arg(&c, method)?,
                None => -1,
            };
            Value::from(py_replace(s, &old, &new, count)?)
        }
        "split" | "rsplit" => {
            let sep = match args.get(0, "sep") {
                Some(v) if !v.is_none() => Some(str_arg(&v, method)?),
                _ => None,
            };
            let max = match args.get(1, "maxsplit") {
                Some(v) => int_arg(&v, method)?,
                None => -1,
            };
            if method == "split" {
                strs(py_split(s, sep.as_deref(), max)?)
            } else {
                strs(py_rsplit(s, sep.as_deref(), max)?)
            }
        }
        "splitlines" => strs(s.lines().map(str::to_string).collect()),
        "startswith" => Value::from(affix_any(&a0()?, |p| s.starts_with(p))?),
        "endswith" => Value::from(affix_any(&a0()?, |p| s.ends_with(p))?),
        "find" | "index" | "rfind" | "rindex" => {
            let sub = str_arg(&a0()?, method)?;
            let r = char_find(s, &sub, method.starts_with('r'));
            if r < 0 && method.ends_with("index") {
                return Err(invalid("substring not found"));
            }
            Value::from(r)
        }
        "count" => {
            let sub = str_arg(&a0()?, method)?;
            Value::from(if sub.is_empty() { char_len(s) + 1 } else { s.matches(sub.as_str()).count() } as i64)
        }
        "join" => {
            let it = a0()?;
            let mut out = String::new();
            for (i, item) in it.try_iter()?.enumerate() {
                if i > 0 {
                    out.push_str(s);
                }
                out.push_str(&str_arg(&item, "join")?);
                check_len(out.len())?;
            }
            Value::from(out)
        }
        "zfill" => {
            let w = int_arg(&a0()?, method)?;
            let len = char_len(s) as i64;
            if w <= len {
                Value::from(s)
            } else {
                check_len(w as usize)?;
                let (sign, rest) = match s.chars().next() {
                    Some(c @ ('+' | '-')) => (c.to_string(), &s[1..]),
                    _ => (String::new(), s),
                };
                Value::from(format!("{sign}{}{rest}", "0".repeat((w - len) as usize)))
            }
        }
        "center" | "ljust" | "rjust" => {
            let w = int_arg(&a0()?, method)?;
            let fill = fill_arg(args, 1)?;
            let align = match method {
                "ljust" => '<',
                "rjust" => '>',
                _ => '^',
            };
            Value::from(pad_to(s, w, fill, align)?)
        }
        "isdigit" | "isdecimal" | "isnumeric" => {
            Value::from(!s.is_empty() && s.chars().all(|c| c.is_numeric()))
        }
        "isalpha" => Value::from(!s.is_empty() && s.chars().all(char::is_alphabetic)),
        "isalnum" => Value::from(!s.is_empty() && s.chars().all(char::is_alphanumeric)),
        "isspace" => Value::from(!s.is_empty() && s.chars().all(py_isspace)),
        "isupper" => Value::from(s.chars().any(char::is_uppercase) && !s.chars().any(char::is_lowercase)),
        "islower" => Value::from(s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase)),
        "istitle" => Value::from(!s.is_empty() && py_title(s) == s && s.chars().any(char::is_alphabetic)),
        "partition" | "rpartition" => {
            let sep = str_arg(&a0()?, method)?;
            if sep.is_empty() {
                return Err(invalid("empty separator"));
            }
            let found = if method == "partition" { s.find(&sep) } else { s.rfind(&sep) };
            let parts = match found {
                Some(i) => vec![s[..i].to_string(), sep.clone(), s[i + sep.len()..].to_string()],
                None if method == "partition" => vec![s.to_string(), String::new(), String::new()],
                None => vec![String::new(), String::new(), s.to_string()],
            };
            strs(parts)
        }
        "removeprefix" => {
            let p = str_arg(&a0()?, method)?;
            Value::from(s.strip_prefix(p.as_str()).unwrap_or(s))
        }
        "removesuffix" => {
            let p = str_arg(&a0()?, method)?;
            Value::from(s.strip_suffix(p.as_str()).unwrap_or(s))
        }
        "format" | "format_map" => Value::from(py_format(s, args)?),
        _ => return Err(Error::from(ErrorKind::UnknownMethod)),
    })
}

// ---------------------------------------------------------------------------
// str.format (format specification per PEP 3101, common subset)
// ---------------------------------------------------------------------------

struct Spec {
    fill: char,
    align: Option<char>,
    sign: char,
    alt: bool,
    zero: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(spec: &str) -> Result<Spec, Error> {
    let c: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let mut s = Spec {
        fill: ' ',
        align: None,
        sign: '-',
        alt: false,
        zero: false,
        width: 0,
        grouping: None,
        precision: None,
        ty: None,
    };
    let is_align = |x: char| matches!(x, '<' | '>' | '^' | '=');
    if c.len() >= 2 && is_align(c[1]) {
        s.fill = c[0];
        s.align = Some(c[1]);
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        s.align = Some(c[0]);
        i = 1;
    }
    if i < c.len() && matches!(c[i], '+' | '-' | ' ') {
        s.sign = c[i];
        i += 1;
    }
    if i < c.len() && c[i] == '#' {
        s.alt = true;
        i += 1;
    }
    if i < c.len() && c[i] == '0' {
        s.zero = true;
        i += 1;
    }
    let mut w = String::new();
    while i < c.len() && c[i].is_ascii_digit() {
        w.push(c[i]);
        i += 1;
    }
    if !w.is_empty() {
        s.width = w.parse().map_err(|_| invalid("Too many decimal digits in format string"))?;
        check_len(s.width)?;
    }
    if i < c.len() && matches!(c[i], ',' | '_') {
        s.grouping = Some(c[i]);
        i += 1;
    }
    if i < c.len() && c[i] == '.' {
        i += 1;
        let mut p = String::new();
        while i < c.len() && c[i].is_ascii_digit() {
            p.push(c[i]);
            i += 1;
        }
        if p.is_empty() {
            return Err(invalid("Format specifier missing precision"));
        }
        let prec: usize = p.parse().map_err(|_| invalid("Too many decimal digits in format string"))?;
        check_len(prec)?;
        s.precision = Some(prec);
    }
    if i < c.len() {
        s.ty = Some(c[i]);
        i += 1;
    }
    if i < c.len() {
        return Err(invalid("Invalid format specifier"));
    }
    Ok(s)
}

fn group(digits: &str, sep: char) -> String {
    let n = digits.len();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

fn apply_padding(body: String, sign: &str, spec: &Spec, default_align: char) -> Result<String, Error> {
    let len = char_len(&body) + sign.len();
    let align = spec.align.unwrap_or(if spec.zero { '=' } else { default_align });
    let fill = if spec.zero && spec.align.is_none() { '0' } else { spec.fill };
    if spec.width <= len {
        return Ok(format!("{sign}{body}"));
    }
    let pad = spec.width - len;
    let f = fill.to_string();
    Ok(match align {
        '<' => format!("{sign}{body}{}", f.repeat(pad)),
        '>' => format!("{}{sign}{body}", f.repeat(pad)),
        '=' => format!("{sign}{}{body}", f.repeat(pad)),
        _ => {
            let left = pad / 2;
            format!("{}{sign}{body}{}", f.repeat(left), f.repeat(pad - left))
        }
    })
}

/// Python `format(value, spec)`.
pub fn py_format_value(v: &Value, spec_str: &str) -> Result<String, Error> {
    let spec = parse_spec(spec_str)?;
    if let Some(Num::Int(i)) = as_num(v).filter(|_| v.kind() != ValueKind::Bool || spec.ty.is_some()) {
        let ty = spec.ty.unwrap_or('d');
        let sign = if i < 0 {
            "-"
        } else {
            match spec.sign {
                '+' => "+",
                ' ' => " ",
                _ => "",
            }
        };
        let a = i.unsigned_abs();
        let body = match ty {
            'd' | 'n' => {
                let d = a.to_string();
                match spec.grouping {
                    Some(g) => group(&d, g),
                    None => d,
                }
            }
            'x' => format!("{}{a:x}", if spec.alt { "0x" } else { "" }),
            'X' => format!("{}{a:X}", if spec.alt { "0X" } else { "" }),
            'o' => format!("{}{a:o}", if spec.alt { "0o" } else { "" }),
            'b' => format!("{}{a:b}", if spec.alt { "0b" } else { "" }),
            'c' => char::from_u32(a as u32).map(String::from).unwrap_or_default(),
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' | '%' => {
                return py_format_value(&Value::from(i as f64), spec_str);
            }
            _ => return Err(invalid(format!("Unknown format code '{ty}' for object of type 'int'"))),
        };
        return apply_padding(body, sign, &spec, '>');
    }
    if let Some(n) = as_num(v).filter(|_| v.kind() != ValueKind::Bool) {
        let f = match n {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        };
        let sign = if f.is_sign_negative() && f != 0.0 || f < 0.0 {
            "-"
        } else {
            match spec.sign {
                '+' => "+",
                ' ' => " ",
                _ => "",
            }
        };
        let a = f.abs();
        let body = match spec.ty {
            Some('f') | Some('F') => format!("{:.*}", spec.precision.unwrap_or(6), a),
            Some('e') | Some('E') => {
                let s = format!("{:.*e}", spec.precision.unwrap_or(6), a);
                let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
                let e: i32 = e.parse().unwrap_or(0);
                let r = format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
                if spec.ty == Some('E') {
                    r.to_uppercase()
                } else {
                    r
                }
            }
            Some('%') => format!("{:.*}%", spec.precision.unwrap_or(6), a * 100.0),
            None if spec.precision.is_none() => crate::pystr::py_float_repr(a),
            _ => {
                // 'g' / None with precision: simplified general representation
                let p = spec.precision.unwrap_or(6).max(1);
                let s = format!("{:.*e}", p - 1, a);
                let exp: i32 = s.split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0);
                if exp < -4 || exp >= p as i32 {
                    s
                } else {
                    let t = format!("{:.*}", (p as i32 - 1 - exp).max(0) as usize, a);
                    if t.contains('.') {
                        t.trim_end_matches('0').trim_end_matches('.').to_string()
                    } else {
                        t
                    }
                }
            }
        };
        let body = match spec.grouping {
            Some(g) => {
                let (int, frac) = body.split_once('.').map(|(a, b)| (a.to_string(), format!(".{b}"))).unwrap_or((body.clone(), String::new()));
                if int.chars().all(|c| c.is_ascii_digit()) {
                    format!("{}{frac}", group(&int, g))
                } else {
                    body
                }
            }
            None => body,
        };
        return apply_padding(body, sign, &spec, '>');
    }
    // str() for everything else (incl. bool with empty type)
    if spec.ty.is_some_and(|t| t != 's') {
        return Err(invalid(format!(
            "Unknown format code '{}' for object of type '{}'",
            spec.ty.unwrap_or('s'),
            type_short(v)
        )));
    }
    let mut s = py_str(v)?;
    if let Some(p) = spec.precision {
        s = s.chars().take(p).collect();
    }
    apply_padding(s, "", &spec, '<')
}

/// Python `str.format(*args, **kwargs)`.
pub fn py_format(fmt: &str, args: &PyArgs) -> Result<String, Error> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut auto_idx = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                out.push('{');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            let mut depth = 1;
            while j < chars.len() {
                if chars[j] == '{' {
                    depth += 1;
                } else if chars[j] == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            if j >= chars.len() {
                return Err(invalid("Single '{' encountered in format string"));
            }
            let field: String = chars[i + 1..j].iter().collect();
            let (head, spec) = match field.split_once(':') {
                Some((h, s)) => (h.to_string(), s.to_string()),
                None => (field.clone(), String::new()),
            };
            let (name, conv) = match head.split_once('!') {
                Some((n, c)) => (n.to_string(), Some(c.to_string())),
                None => (head, None),
            };
            let value = resolve_field(&name, args, &mut auto_idx)?;
            // Nested fields in the format specification (`{:{width}}`, `{:>{}}`), one level like Python.
            let spec = if spec.contains('{') { expand_spec(&spec, args, &mut auto_idx)? } else { spec };
            let piece = match conv.as_deref() {
                Some("r") => py_format_value(&Value::from(py_repr(&value)?), &spec)?,
                Some("s") => py_format_value(&Value::from(py_str(&value)?), &spec)?,
                Some(other) => return Err(invalid(format!("Unknown conversion specifier {other}"))),
                None => py_format_value(&value, &spec)?,
            };
            out.push_str(&piece);
            check_len(out.len())?;
            i = j + 1;
        } else if c == '}' {
            if i + 1 < chars.len() && chars[i + 1] == '}' {
                out.push('}');
                i += 2;
                continue;
            }
            return Err(invalid("Single '}' encountered in format string"));
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(out)
}

/// Resolve the field name of a `str.format` replacement field: index/name, then `.attr` and `[key]`
/// (jinja2 SandboxedFormatter.get_field: `env.getattr` or `env.getitem`; `_` attributes are blocked).
fn resolve_field(name: &str, args: &PyArgs, auto_idx: &mut usize) -> Result<Value, Error> {
    let first_end = name.find(['.', '[']).unwrap_or(name.len());
    let (first, mut rest) = name.split_at(first_end);
    let mut value = if first.is_empty() {
        let v = args
            .positional()
            .get(*auto_idx)
            .cloned()
            .ok_or_else(|| invalid(format!("Replacement index {} out of range for positional args tuple", *auto_idx)))?;
        *auto_idx += 1;
        v
    } else if let Ok(idx) = first.parse::<usize>() {
        args.positional()
            .get(idx)
            .cloned()
            .ok_or_else(|| invalid(format!("Replacement index {idx} out of range for positional args tuple")))?
    } else {
        args.kwargs()
            .iter()
            .find(|(k, _)| *k == first)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| invalid(format!("'{first}'")))?
    };
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            let attr = &r[..end];
            if attr.is_empty() {
                return Err(invalid("Empty attribute in format string"));
            }
            if attr.starts_with('_') {
                return Err(security_error(format!(
                    "access to attribute '{attr}' of '{}' object is unsafe.",
                    crate::pyvalue::py_type(&value).trim_start_matches("<class '").trim_end_matches("'>")
                )));
            }
            value = value.get_attr(attr)?;
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').ok_or_else(|| invalid("Missing ']' in format string"))?;
            let key = &r[..end];
            if key.is_empty() {
                return Err(invalid("Empty attribute in format string"));
            }
            let k = match key.parse::<i64>() {
                Ok(n) if !key.starts_with(['-', '+']) => Value::from(n),
                _ => Value::from(key),
            };
            value = value.get_item(&k)?;
            rest = &r[end + 1..];
            if !rest.is_empty() && !rest.starts_with(['.', '[']) {
                return Err(invalid("Only '.' or '[' may follow ']' in format field specifier"));
            }
        } else {
            return Err(invalid("Only '.' or '[' may follow ']' in format field specifier"));
        }
        if value.is_undefined() {
            return Err(Error::from(ErrorKind::UndefinedError));
        }
    }
    Ok(value)
}

/// Replace `{...}` inside a format specification with `str()` of the field.
fn expand_spec(spec: &str, args: &PyArgs, auto_idx: &mut usize) -> Result<String, Error> {
    let mut out = String::new();
    let mut rest = spec;
    while let Some(a) = rest.find('{') {
        out.push_str(&rest[..a]);
        let b = rest[a..].find('}').ok_or_else(|| invalid("unmatched '{' in format spec"))? + a;
        let inner = &rest[a + 1..b];
        if inner.contains('{') {
            return Err(invalid("Max string recursion exceeded"));
        }
        out.push_str(&py_str(&resolve_field(inner, args, auto_idx)?)?);
        rest = &rest[b + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Callback for unknown methods
// ---------------------------------------------------------------------------

fn date_method(v: &Value, method: &str, args: &PyArgs) -> Option<Result<Value, Error>> {
    if let Some(d) = v.downcast_object_ref::<PyDate>() {
        let d = d.0;
        return Some(match method {
            "isoformat" => Ok(Value::from(date_iso(d))),
            "strftime" | "__format__" => match args.get(0, "format").map(|f| str_arg(&f, "strftime")) {
                Some(Ok(f)) => Ok(Value::from(strftime(&d.and_time(chrono::NaiveTime::MIN), TzInfo::Naive, &f))),
                Some(Err(e)) => Err(e),
                None => Err(invalid("strftime() missing required argument 'format' (pos 1)")),
            },
            "weekday" => Ok(Value::from(chrono::Datelike::weekday(&d).num_days_from_monday() as i64)),
            "isoweekday" => Ok(Value::from(chrono::Datelike::weekday(&d).number_from_monday() as i64)),
            "toordinal" => Ok(Value::from(chrono::Datelike::num_days_from_ce(&d) as i64)),
            _ => return None,
        });
    }
    if let Some(dt) = v.downcast_object_ref::<PyDateTime>() {
        let dt = *dt;
        return Some(match method {
            "isoformat" => {
                let sep = match args.get(0, "sep") {
                    Some(s) => match str_arg(&s, "isoformat") {
                        Ok(s) => s,
                        Err(e) => return Some(Err(e)),
                    },
                    None => "T".into(),
                };
                Ok(Value::from(dt.isoformat(&sep)))
            }
            "strftime" | "__format__" => match args.get(0, "format").map(|f| str_arg(&f, "strftime")) {
                Some(Ok(f)) => Ok(Value::from(strftime(&dt.naive, dt.tzinfo(), &f))),
                Some(Err(e)) => Err(e),
                None => Err(invalid("strftime() missing required argument 'format' (pos 1)")),
            },
            "date" => Ok(crate::pyvalue::date_value(dt.naive.date())),
            "weekday" => Ok(Value::from(chrono::Datelike::weekday(&dt.naive).num_days_from_monday() as i64)),
            "isoweekday" => Ok(Value::from(chrono::Datelike::weekday(&dt.naive).number_from_monday() as i64)),
            _ => return None,
        });
    }
    None
}

/// `Environment::set_unknown_method_callback`.
pub(crate) fn unknown_method(_state: &State, value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    // upstream: JinjaEnvironment.is_safe_callable blocks save/delete/update
    if matches!(method, "save" | "delete" | "update") {
        return Err(security_error(format!("{method} is not safely callable")));
    }
    if method.starts_with('_') && method != "__format__" {
        return Err(security_error(format!("access to attribute '{method}' is unsafe")));
    }
    let args = PyArgs::new(args)?;
    if let Some(r) = date_method(value, method, &args) {
        return r;
    }
    if let Some(s) = as_pystr(value) {
        return str_method(&s, method, &args);
    }
    match value.kind() {
        ValueKind::Map => match method {
            "items" => {
                let mut out = Vec::new();
                for k in value.try_iter()? {
                    let v = value.get_item(&k)?;
                    out.push(Value::from(vec![k, v]));
                }
                Ok(Value::from(out))
            }
            "keys" => Ok(Value::from(value.try_iter()?.collect::<Vec<_>>())),
            "values" => {
                let mut out = Vec::new();
                for k in value.try_iter()? {
                    out.push(value.get_item(&k)?);
                }
                Ok(Value::from(out))
            }
            "get" => {
                let key = req(&args, 0, "key", "get")?;
                let v = value.get_item(&key)?;
                if v.is_undefined() {
                    Ok(args.get(1, "default").unwrap_or(Value::from(())))
                } else {
                    Ok(v)
                }
            }
            "copy" => Ok(value.clone()),
            _ => Err(Error::from(ErrorKind::UnknownMethod)),
        },
        ValueKind::Seq => match method {
            "index" => {
                let x = req(&args, 0, "value", "index")?;
                for (i, item) in value.try_iter()?.enumerate() {
                    if py_eq_inner(&item, &x) {
                        return Ok(Value::from(i as i64));
                    }
                }
                Err(invalid(format!("{} is not in list", py_repr(&x)?)))
            }
            "count" => {
                let x = req(&args, 0, "value", "count")?;
                Ok(Value::from(value.try_iter()?.filter(|i| py_eq_inner(i, &x)).count() as i64))
            }
            "copy" => Ok(value.clone()),
            _ => Err(Error::from(ErrorKind::UnknownMethod)),
        },
        _ => Err(Error::from(ErrorKind::UnknownMethod)),
    }
}

/// Byte index to character index (for filters that need Python indices).
pub(crate) fn char_slice(s: &str, start: usize, end: usize) -> &str {
    let a = byte_index_of_char(s, start);
    let b = byte_index_of_char(s, end);
    &s[a..b.max(a)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(f: &str, pos: Vec<Value>) -> String {
        py_format(f, &PyArgs { pos, kw: vec![] }).unwrap()
    }

    #[test]
    fn format() {
        assert_eq!(fmt("{:04d}", vec![Value::from(42)]), "0042");
        assert_eq!(fmt("{} and {}", vec![Value::from("a"), Value::from(1.5)]), "a and 1.5");
        assert_eq!(fmt("{0:>5}|{0:<5}|{0:^5}", vec![Value::from("ab")]), "   ab|ab   | ab  ");
        assert_eq!(fmt("{:.2f}", vec![Value::from(1.23456)]), "1.23");
        assert_eq!(fmt("{:,}", vec![Value::from(1234567)]), "1,234,567");
        assert_eq!(fmt("{{x}}", vec![]), "{x}");
    }

    #[test]
    fn split_title() {
        assert_eq!(py_split("  a  b c ", None, -1).unwrap(), vec!["a", "b", "c"]);
        assert_eq!(py_split("a-b-c", Some("-"), 1).unwrap(), vec!["a", "b-c"]);
        assert_eq!(py_rsplit("a-b-c", Some("-"), 1).unwrap(), vec!["a-b", "c"]);
        assert_eq!(py_title("hello wORLD it's"), "Hello World It'S");
        assert_eq!(py_capitalize("hello WORLD"), "Hello world");
    }
}
