//! Jinja2 filters and globals that minijinja does not bring along (gap W9).
//!
//! upstream: jinja2/filters.py@3.1.6 (do_random, do_urlize, do_wordwrap, do_xmlattr),
//! jinja2/utils.py@3.1.6 (urlize, generate_lorem_ipsum, Cycler, Joiner),
//! jinja2/defaults.py@3.1.6 (DEFAULT_POLICIES: urlize.rel = "noopener", urlize.target = None),
//! Python textwrap (wrap with expand_tabs=False, replace_whitespace=False).
//!
//! The sandbox remains: all results pass through the size limits (`check_len`), objects
//! have only the Jinja2 attributes (`next`, `reset`, `current`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use minijinja::value::{Object, ObjectRepr, Rest, Value, ValueKind};
use minijinja::{Error, State};
use rand::Rng;
use regex::Regex;

use crate::methods::PyArgs;
use crate::pyvalue::{Num, as_num, check_len, invalid, py_str};

/// `markupsafe.escape`.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn int_arg(v: Option<Value>, default: i64, what: &str) -> Result<i64, Error> {
    match v {
        None => Ok(default),
        Some(v) => match as_num(&v) {
            Some(Num::Int(i)) => i64::try_from(i).map_err(|_| invalid("Python int too large to convert to C ssize_t")),
            _ => Err(invalid(format!("{what}: integer argument expected"))),
        },
    }
}

// ---------------------------------------------------------------------------
// random
// ---------------------------------------------------------------------------

/// `do_random(context, seq)`: random element; empty sequence → Undefined (Jinja2:
/// `environment.undefined("No random item, sequence was empty.")`).
pub fn f_random(value: Value) -> Result<Value, Error> {
    let items: Vec<Value> = match value.kind() {
        ValueKind::String => value.as_str().unwrap_or_default().chars().map(|c| Value::from(c.to_string())).collect(),
        ValueKind::Seq | ValueKind::Iterable => value.try_iter()?.collect(),
        ValueKind::Map => return Err(invalid("KeyError")),
        _ => return Err(invalid(format!("object of type '{}' has no len()", crate::pyvalue::py_type(&value)))),
    };
    if items.is_empty() {
        return Ok(Value::UNDEFINED);
    }
    let i = rand::thread_rng().gen_range(0..items.len());
    Ok(items[i].clone())
}

// ---------------------------------------------------------------------------
// urlize
// ---------------------------------------------------------------------------

fn http_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)^(
                (https?://|www\.)(([\w%-]+\.)+)?([a-z]{2,63}|xn--[\w%]{2,59})
              | ([\w%-]{2,63}\.)+(com|net|int|edu|gov|org|info|mil)
              | (https?://)((([\d]{1,3})(\.[\d]{1,3}){3})|(\[([\da-f]{0,4}:){2}([\da-f]{0,4}:?){1,6}\]))
            )(?::[\d]{1,5})?(?:[/?\#]\S*)?$",
        )
        .unwrap_or_else(|_| unreachable_regex())
    })
}

fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\S+@\w[\w.-]*\.\w+$").unwrap_or_else(|_| unreachable_regex()))
}

#[allow(clippy::unwrap_used)]
fn unreachable_regex() -> Regex {
    // An empty pattern is always valid.
    Regex::new("").unwrap()
}

fn lead_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?:[(<]|&lt;)+").unwrap_or_else(|_| unreachable_regex()))
}

fn trail_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:[)>.,\n]|&gt;)+$").unwrap_or_else(|_| unreachable_regex()))
}

/// `jinja2.utils.urlize`.
pub fn urlize(text: &str, trim_url_limit: Option<usize>, rel: Option<&str>, target: Option<&str>, extra_schemes: &[String]) -> String {
    let trim_url = |x: &str| -> String {
        match trim_url_limit {
            Some(n) if x.chars().count() > n => format!("{}...", x.chars().take(n).collect::<String>()),
            _ => x.to_string(),
        }
    };
    let escaped = html_escape(text);
    let rel_attr = rel.filter(|r| !r.is_empty()).map(|r| format!(" rel=\"{}\"", html_escape(r))).unwrap_or_default();
    let target_attr = target.filter(|t| !t.is_empty()).map(|t| format!(" target=\"{}\"", html_escape(t))).unwrap_or_default();

    // re.split(r"(\s+)", …): words and separators alternating
    let mut parts: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut in_ws: Option<bool> = None;
    for (i, c) in escaped.char_indices() {
        let ws = c.is_whitespace();
        match in_ws {
            Some(prev) if prev != ws => {
                parts.push((prev, &escaped[start..i]));
                start = i;
            }
            _ => {}
        }
        in_ws = Some(ws);
    }
    if let Some(prev) = in_ws {
        parts.push((prev, &escaped[start..]));
    }

    let mut out = String::with_capacity(escaped.len());
    for (ws, word) in parts {
        if ws {
            out.push_str(word);
            continue;
        }
        let mut head = String::new();
        let mut middle = word.to_string();
        let mut tail = String::new();
        if let Some(m) = lead_re().find(&middle) {
            head = m.as_str().to_string();
            middle = middle[m.end()..].to_string();
        }
        if [")", ">", ".", ",", "\n", "&gt;"].iter().any(|e| middle.ends_with(e))
            && let Some(m) = trail_re().find(&middle)
        {
            tail = m.as_str().to_string();
            middle.truncate(m.start());
        }
        for (sc, ec) in [("(", ")"), ("<", ">"), ("&lt;", "&gt;")] {
            let start_count = middle.matches(sc).count();
            if start_count <= middle.matches(ec).count() {
                continue;
            }
            for _ in 0..start_count.min(tail.matches(ec).count()) {
                let Some(idx) = tail.find(ec) else { break };
                let end_index = idx + ec.len();
                middle.push_str(&tail[..end_index]);
                tail = tail[end_index..].to_string();
            }
        }
        if http_re().is_match(&middle) {
            if middle.starts_with("https://") || middle.starts_with("http://") {
                middle = format!("<a href=\"{middle}\"{rel_attr}{target_attr}>{}</a>", trim_url(&middle));
            } else {
                middle = format!("<a href=\"https://{middle}\"{rel_attr}{target_attr}>{}</a>", trim_url(&middle));
            }
        } else if let Some(addr) = middle.strip_prefix("mailto:").filter(|a| email_re().is_match(a)) {
            middle = format!("<a href=\"{middle}\">{addr}</a>");
        } else if middle.contains('@')
            && !middle.starts_with("www.")
            && !middle.starts_with('@')
            && !middle.contains(':')
            && email_re().is_match(&middle)
        {
            middle = format!("<a href=\"mailto:{middle}\">{middle}</a>");
        } else {
            for scheme in extra_schemes {
                if middle != *scheme && middle.starts_with(scheme.as_str()) {
                    middle = format!("<a href=\"{middle}\"{rel_attr}{target_attr}>{middle}</a>");
                }
            }
        }
        out.push_str(&head);
        out.push_str(&middle);
        out.push_str(&tail);
    }
    out
}

/// `do_urlize(eval_ctx, value, trim_url_limit=None, nofollow=False, target=None, rel=None, extra_schemes=None)`.
pub fn f_urlize(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_urlize", &["trim_url_limit", "nofollow", "target", "rel", "extra_schemes"])?;
    let trim = match args.get(0, "trim_url_limit").filter(|v| !v.is_none()) {
        Some(v) => Some(usize::try_from(int_arg(Some(v), 0, "trim_url_limit")?.max(0)).unwrap_or(0)),
        None => None,
    };
    let nofollow = args.get(1, "nofollow").is_some_and(|v| v.is_true());
    let target = match args.get(2, "target").filter(|v| !v.is_none()) {
        Some(t) => Some(py_str(&t)?),
        None => None,
    };
    // rel_parts = set((rel or "").split()) | {"nofollow"} | policies["urlize.rel"].split()
    let mut rel_parts: Vec<String> = match args.get(3, "rel").filter(|v| v.is_true()) {
        Some(r) => py_str(&r)?.split_whitespace().map(str::to_string).collect(),
        None => Vec::new(),
    };
    if nofollow {
        rel_parts.push("nofollow".into());
    }
    rel_parts.push("noopener".into());
    rel_parts.sort();
    rel_parts.dedup();
    let rel = rel_parts.join(" ");
    let mut schemes = Vec::new();
    if let Some(s) = args.get(4, "extra_schemes").filter(|v| !v.is_none()) {
        for item in s.try_iter()? {
            let scheme = py_str(&item)?;
            // re.fullmatch(r"^[a-zA-Z][a-zA-Z0-9+.-]*:(//)?$", scheme)
            let ok = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && {
                let body = scheme.strip_suffix("//").unwrap_or(&scheme);
                body.ends_with(':') && body[..body.len() - 1].chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            };
            if !ok {
                return Err(invalid(format!("{scheme} is not a valid URI scheme prefix.")));
            }
            schemes.push(scheme);
        }
    }
    let out = urlize(&py_str(&value)?, trim, Some(&rel), target.as_deref(), &schemes);
    check_len(out.len())?;
    Ok(Value::from(out))
}

// ---------------------------------------------------------------------------
// wordwrap (Python textwrap)
// ---------------------------------------------------------------------------

/// Splitting like `TextWrapper._split` (simplified): whitespace runs and words; with
/// `break_on_hyphens` additionally after hyphens between letters.
fn split_chunks(text: &str, break_on_hyphens: bool) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_ws: Option<bool> = None;
    for c in text.chars() {
        let ws = matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
        if cur_ws.is_some_and(|p| p != ws) {
            chunks.push(std::mem::take(&mut cur));
        }
        cur_ws = Some(ws);
        cur.push(c);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if !break_on_hyphens {
        return chunks;
    }
    let mut out = Vec::new();
    for ch in chunks {
        let chars: Vec<char> = ch.chars().collect();
        if chars.first().is_some_and(|c| c.is_whitespace()) {
            out.push(ch);
            continue;
        }
        let mut piece = String::new();
        for i in 0..chars.len() {
            piece.push(chars[i]);
            let hyphen_break =
                chars[i] == '-' && i > 0 && chars[i - 1].is_alphanumeric() && chars.get(i + 1).is_some_and(|c| c.is_alphabetic());
            if hyphen_break {
                out.push(std::mem::take(&mut piece));
            }
        }
        if !piece.is_empty() {
            out.push(piece);
        }
    }
    out
}

/// `textwrap.wrap(text, width, expand_tabs=False, replace_whitespace=False, break_long_words,
/// break_on_hyphens)` (drop_whitespace=True).
pub fn textwrap(text: &str, width: usize, break_long_words: bool, break_on_hyphens: bool) -> Result<Vec<String>, Error> {
    if width == 0 {
        return Err(invalid(format!("invalid width {width} (must be > 0)")));
    }
    let mut chunks: Vec<String> = split_chunks(text, break_on_hyphens);
    chunks.reverse();
    let is_ws = |s: &str| s.trim().is_empty();
    let mut lines = Vec::new();
    while !chunks.is_empty() {
        let mut cur: Vec<String> = Vec::new();
        let mut cur_len = 0usize;
        if chunks.last().is_some_and(|c| is_ws(c)) && !lines.is_empty() {
            chunks.pop();
        }
        while let Some(last) = chunks.last() {
            let l = last.chars().count();
            if cur_len + l <= width {
                cur_len += l;
                if let Some(c) = chunks.pop() {
                    cur.push(c);
                }
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| c.chars().count() > width) {
            // _handle_long_word
            let space_left = if width > cur_len { width - cur_len } else { 1 };
            if break_long_words {
                if let Some(last) = chunks.pop() {
                    let head: String = last.chars().take(space_left).collect();
                    let tail: String = last.chars().skip(space_left).collect();
                    cur.push(head);
                    chunks.push(tail);
                }
            } else if cur.is_empty()
                && let Some(c) = chunks.pop()
            {
                cur.push(c);
            }
        }
        if cur.last().is_some_and(|c| is_ws(c)) {
            cur.pop();
        }
        if !cur.is_empty() {
            lines.push(cur.concat());
        }
    }
    Ok(lines)
}

/// `do_wordwrap(environment, s, width=79, break_long_words=True, wrapstring=None, break_on_hyphens=True)`.
pub fn f_wordwrap(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_wordwrap", &["width", "break_long_words", "wrapstring", "break_on_hyphens"])?;
    let width = int_arg(args.get(0, "width"), 79, "width")?;
    let break_long = args.get(1, "break_long_words").is_none_or(|v| v.is_true());
    let wrapstring = match args.get(2, "wrapstring").filter(|v| !v.is_none()) {
        Some(w) => py_str(&w)?,
        None => "\n".to_string(),
    };
    let hyphens = args.get(3, "break_on_hyphens").is_none_or(|v| v.is_true());
    if width <= 0 {
        return Err(invalid(format!("invalid width {width} (must be > 0)")));
    }
    let width = usize::try_from(width).unwrap_or(usize::MAX);
    let s = py_str(&value)?;
    let mut paragraphs = Vec::new();
    // str.splitlines()
    for line in s.lines() {
        let wrapped = textwrap(line, width, break_long, hyphens)?;
        paragraphs.push(wrapped.join(&wrapstring));
    }
    let out = paragraphs.join(&wrapstring);
    check_len(out.len())?;
    Ok(Value::from(out))
}

// ---------------------------------------------------------------------------
// xmlattr
// ---------------------------------------------------------------------------

/// `do_xmlattr(eval_ctx, d, autospace=True)`.
pub fn f_xmlattr(value: Value, rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("do_xmlattr", &["autospace"])?;
    let autospace = args.get(0, "autospace").is_none_or(|v| v.is_true());
    if value.kind() != ValueKind::Map {
        return Err(invalid(format!("'{}' object has no attribute 'items'", crate::pyvalue::py_type(&value))));
    }
    let mut items = Vec::new();
    for key in value.try_iter()? {
        let v = value.get_item(&key)?;
        if v.is_none() || v.is_undefined() {
            continue;
        }
        let k = py_str(&key)?;
        if k.chars().any(|c| c.is_ascii_whitespace() || matches!(c, '/' | '>' | '=')) {
            return Err(invalid(format!("Invalid character in attribute name: {}", crate::pystr::py_str_repr(&k))));
        }
        items.push(format!("{}=\"{}\"", html_escape(&k), html_escape(&py_str(&v)?)));
    }
    let mut rv = items.join(" ");
    if autospace && !rv.is_empty() {
        rv.insert(0, ' ');
    }
    check_len(rv.len())?;
    Ok(Value::from(rv))
}

// ---------------------------------------------------------------------------
// Globals: lipsum, cycler, joiner
// ---------------------------------------------------------------------------

const LOREM_IPSUM_WORDS: &str = "a ac accumsan ad adipiscing aenean aliquam aliquet amet ante aptent arcu at auctor augue \
bibendum blandit class commodo condimentum congue consectetuer consequat conubia convallis cras cubilia cum curabitur curae \
cursus dapibus diam dictum dictumst dignissim dis dolor donec dui duis egestas eget eleifend elementum elit enim erat eros est \
et etiam eu euismod facilisi facilisis fames faucibus felis fermentum feugiat fringilla fusce gravida habitant habitasse hac \
hendrerit hymenaeos iaculis id imperdiet in inceptos integer interdum ipsum justo lacinia lacus laoreet lectus leo libero \
ligula litora lobortis lorem luctus maecenas magna magnis malesuada massa mattis mauris metus mi molestie mollis montes morbi \
mus nam nascetur natoque nec neque netus nibh nisi nisl non nonummy nostra nulla nullam nunc odio orci ornare parturient pede \
pellentesque penatibus per pharetra phasellus placerat platea porta porttitor posuere potenti praesent pretium primis proin \
pulvinar purus quam quis quisque rhoncus ridiculus risus rutrum sagittis sapien scelerisque sed sem semper senectus sit \
sociis sociosqu sodales sollicitudin suscipit suspendisse taciti tellus tempor tempus tincidunt torquent tortor tristique \
turpis ullamcorper ultrices ultricies urna ut varius vehicula vel velit venenatis vestibulum vitae vivamus viverra volutpat \
vulputate";

/// `random.randrange(a, b)`; empty range → ValueError like Python.
fn randrange(rng: &mut impl Rng, a: i64, b: i64) -> Result<i64, Error> {
    if a >= b {
        return Err(invalid(format!("empty range in randrange({a}, {b})")));
    }
    Ok(rng.gen_range(a..b))
}

/// `generate_lorem_ipsum(n=5, html=True, min=20, max=100)`.
pub fn fn_lipsum(rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("generate_lorem_ipsum", &["n", "html", "min", "max"])?;
    let n = int_arg(args.get(0, "n"), 5, "n")?;
    let html = args.get(1, "html").is_none_or(|v| v.is_true());
    let min = int_arg(args.get(2, "min"), 20, "min")?;
    let max = int_arg(args.get(3, "max"), 100, "max")?;
    // Sandbox: limit the total size (n × max words)
    if n.saturating_mul(max.max(0)) > 1_000_000 {
        return Err(invalid(format!("{}lipsum output too large", crate::pyvalue::SECURITY_PREFIX)));
    }
    let words: Vec<&str> = LOREM_IPSUM_WORDS.split_whitespace().collect();
    let mut rng = rand::thread_rng();
    let mut result = Vec::new();
    for _ in 0..n.max(0) {
        let mut next_capitalized = true;
        let (mut last_comma, mut last_fullstop) = (0i64, 0i64);
        let mut last: Option<&str> = None;
        let mut p: Vec<String> = Vec::new();
        let count = randrange(&mut rng, min, max)?;
        for idx in 0..count {
            let word = loop {
                let w = words[rng.gen_range(0..words.len())];
                if Some(w) != last {
                    last = Some(w);
                    break w;
                }
            };
            let mut word = if next_capitalized {
                next_capitalized = false;
                let mut c = word.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            } else {
                word.to_string()
            };
            if idx - randrange(&mut rng, 3, 8)? > last_comma {
                last_comma = idx;
                last_fullstop += 2;
                word.push(',');
            }
            if idx - randrange(&mut rng, 10, 20)? > last_fullstop {
                last_comma = idx;
                last_fullstop = idx;
                word.push('.');
                next_capitalized = true;
            }
            p.push(word);
        }
        let mut p_str = p.join(" ");
        if p_str.ends_with(',') {
            p_str.pop();
            p_str.push('.');
        } else if !p_str.ends_with('.') {
            p_str.push('.');
        }
        result.push(p_str);
    }
    let out =
        if html { result.iter().map(|x| format!("<p>{}</p>", html_escape(x))).collect::<Vec<_>>().join("\n") } else { result.join("\n\n") };
    check_len(out.len())?;
    Ok(Value::from(out))
}

/// `jinja2.utils.Cycler`.
#[derive(Debug)]
pub struct Cycler {
    items: Vec<Value>,
    pos: AtomicUsize,
}

impl Object for Cycler {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "current" => self.items.get(self.pos.load(Ordering::Relaxed)).cloned(),
            "items" => Some(Value::from(self.items.clone())),
            "pos" => Some(Value::from(self.pos.load(Ordering::Relaxed))),
            _ => None,
        }
    }

    fn call_method(self: &Arc<Self>, _state: &State<'_, '_>, method: &str, args: &[Value]) -> Result<Value, Error> {
        if !args.is_empty() {
            return Err(invalid(format!("Cycler.{method}() takes 1 positional argument but {} were given", args.len() + 1)));
        }
        match method {
            "next" | "__next__" => {
                let pos = self.pos.load(Ordering::Relaxed);
                let rv = self.items.get(pos).cloned().unwrap_or(Value::UNDEFINED);
                self.pos.store((pos + 1) % self.items.len().max(1), Ordering::Relaxed);
                Ok(rv)
            }
            "reset" => {
                self.pos.store(0, Ordering::Relaxed);
                Ok(Value::from(()))
            }
            _ => Err(Error::from(minijinja::ErrorKind::UnknownMethod)),
        }
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<jinja2.utils.Cycler object>")
    }
}

/// `cycler(*items)`.
pub fn fn_cycler(rest: Rest<Value>) -> Result<Value, Error> {
    if rest.iter().any(Value::is_kwargs) {
        return Err(invalid("Cycler.__init__() got an unexpected keyword argument"));
    }
    if rest.is_empty() {
        return Err(invalid("at least one item has to be provided"));
    }
    Ok(Value::from_object(Cycler { items: rest.to_vec(), pos: AtomicUsize::new(0) }))
}

/// `jinja2.utils.Joiner`.
#[derive(Debug)]
pub struct Joiner {
    sep: String,
    used: AtomicBool,
}

impl Object for Joiner {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "sep" => Some(Value::from(self.sep.clone())),
            "used" => Some(Value::from(self.used.load(Ordering::Relaxed))),
            _ => None,
        }
    }

    fn call(self: &Arc<Self>, _state: &State<'_, '_>, args: &[Value]) -> Result<Value, Error> {
        if !args.is_empty() {
            return Err(invalid(format!("Joiner.__call__() takes 1 positional argument but {} were given", args.len() + 1)));
        }
        if self.used.swap(true, Ordering::Relaxed) { Ok(Value::from(self.sep.clone())) } else { Ok(Value::from("")) }
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<jinja2.utils.Joiner object>")
    }
}

/// `joiner(sep=", ")`.
pub fn fn_joiner(rest: Rest<Value>) -> Result<Value, Error> {
    let args = PyArgs::new(&rest)?;
    args.check("Joiner.__init__", &["sep"])?;
    let sep = match args.get(0, "sep") {
        Some(s) => py_str(&s)?,
        None => ", ".to_string(),
    };
    Ok(Value::from_object(Joiner { sep, used: AtomicBool::new(false) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlize_like_jinja2() {
        assert_eq!(
            urlize("see https://example.com/a?b=1.", None, Some("noopener"), None, &[]),
            "see <a href=\"https://example.com/a?b=1\" rel=\"noopener\">https://example.com/a?b=1</a>."
        );
        assert_eq!(
            urlize("www.example.org", None, Some("noopener"), None, &[]),
            "<a href=\"https://www.example.org\" rel=\"noopener\">www.example.org</a>"
        );
        assert_eq!(urlize("mail a@b.de now", None, None, None, &[]), "mail <a href=\"mailto:a@b.de\">a@b.de</a> now");
        assert_eq!(urlize("x < y", None, None, None, &[]), "x &lt; y");
        assert_eq!(urlize("(https://x.com/a_(b))", None, None, None, &[]), "(<a href=\"https://x.com/a_(b)\">https://x.com/a_(b)</a>)");
        assert_eq!(urlize("https://example.com/long", Some(10), None, None, &[]), "<a href=\"https://example.com/long\">https://ex...</a>");
    }

    #[test]
    fn textwrap_like_python() {
        let w = |s: &str, n: usize| textwrap(s, n, true, true).unwrap_or_default();
        assert_eq!(w("The quick brown fox jumps over the lazy dog", 10), vec!["The quick", "brown fox", "jumps over", "the lazy", "dog"]);
        assert_eq!(w("abcdefghijkl", 5), vec!["abcde", "fghij", "kl"]);
        assert_eq!(w("well-known thing", 6), vec!["well-", "known", "thing"]);
        assert_eq!(textwrap("abcdefghijkl x", 5, false, true).unwrap_or_default(), vec!["abcdefghijkl", "x"]);
        assert_eq!(w("", 5), Vec::<String>::new());
    }

    #[test]
    fn escape_like_markupsafe() {
        assert_eq!(html_escape("<a href=\"x\">'&'</a>"), "&lt;a href=&#34;x&#34;&gt;&#39;&amp;&#39;&lt;/a&gt;");
    }
}
