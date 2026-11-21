// upstream: src/documents/regex.py@v3.4.2 – user patterns run there with the Python module `regex`
//           (VERSION0). This module reproduces three of its extensions that fancy-regex does not
//           know (K1), before `regex_compat::translate_python_pattern` translates the rest:
//
//   1. Fuzzy matches `(?:text){e<=1}`, `(text){s<=1,i<=1}`, `x{e<2}`: the content must be plain text;
//      it is expanded into an alternation of all spellings with at most k errors (substitution,
//      insertion, deletion) (k ≤ 3). Whether a match exists is thereby the same as with the
//      regex module; which of several is found may differ (error variants ordered by
//      error count, the exact one first). The flags `(?b)`/`(?e)` (BESTMATCH/ENHANCEMATCH)
//      only change the selection and are removed.
//   2. Recursion `(?R)`, `(?0)`, `(?1)`, `(?&name)`, `(?P>name)`: unrolled up to a fixed depth
//      (default 10, less for large patterns); more deeply nested text does not match.
//   3. Lookbehind with alternatives of different fixed length `(?<=a|bc)` → `(?:(?<=a)|(?<=bc))`,
//      `(?<!a|bc)` → `(?:(?<!a)(?<!bc))`. Lookbehinds with `+`, `*` or `{m,n}` stay invalid.

use std::collections::HashSet;

/// Maximum size of the expanded pattern (characters).
const MAX_EXPANDED: usize = 200_000;
/// Maximum number of error variants per fuzzy group.
const MAX_VARIANTS: usize = 20_000;
/// Maximum error count per fuzzy group.
const MAX_ERRORS: usize = 3;
/// Largest recursion depth (reduced if the result gets too large).
const MAX_DEPTH: usize = 10;

/// Expand Python `regex` extensions. `Err(message)` for forms that cannot be reproduced.
pub fn expand(pattern: &str) -> Result<String, String> {
    if !needs_expansion(pattern) {
        return Ok(pattern.to_string());
    }
    let p = strip_best_enhance_flags(pattern);
    let p = expand_fuzzy(&p)?;
    let p = expand_recursion(&p)?;
    fix_lookbehinds(&p)
}

fn needs_expansion(p: &str) -> bool {
    p.contains("(?") || (p.contains('{') && p.contains('<'))
}

// ---------------------------------------------------------------------------
// Decomposition
// ---------------------------------------------------------------------------

/// Index after the closing `]` of a class that starts at `i` (`[`).
fn skip_class(c: &[char], i: usize) -> usize {
    let mut j = i + 1;
    if c.get(j) == Some(&'^') {
        j += 1;
    }
    if c.get(j) == Some(&']') {
        j += 1;
    }
    while j < c.len() {
        match c[j] {
            '\\' => j += 2,
            '[' if c.get(j + 1) == Some(&':') => {
                let rest: String = c[j..].iter().collect();
                match rest.find(":]") {
                    Some(e) => j += rest[..e + 2].chars().count(),
                    None => j += 1,
                }
            }
            ']' => return j + 1,
            _ => j += 1,
        }
    }
    c.len()
}

/// Index of the `)` matching the `(` at `i`.
fn group_end(c: &[char], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    while j < c.len() {
        match c[j] {
            '\\' => {
                j += 2;
                continue;
            }
            '[' => {
                j = skip_class(c, j);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum GroupKind {
    /// Capturing group (number, name), length of the prefix.
    Capture(usize, Option<String>, usize),
    NonCapture,
    Lookbehind(bool),
    /// Call `(?R)` etc.: target (0 = whole pattern) or name.
    Call(CallTarget),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
enum CallTarget {
    Num(usize),
    Name(String),
}

/// Kind of the group at `i` (`(`) and length of its prefix. `next_cap` is incremented for capture groups.
fn group_kind(c: &[char], i: usize, next_cap: &mut usize) -> (GroupKind, usize) {
    let s: String = c[i..c.len().min(i + 64)].iter().collect();
    if !s.starts_with("(?") {
        *next_cap += 1;
        return (GroupKind::Capture(*next_cap, None, 1), 1);
    }
    let rest = &s[2..];
    if rest.starts_with(':') {
        return (GroupKind::NonCapture, 3);
    }
    if rest.starts_with("<=") {
        return (GroupKind::Lookbehind(true), 4);
    }
    if rest.starts_with("<!") {
        return (GroupKind::Lookbehind(false), 4);
    }
    // Prefix length = `(?` + opener + name + closer
    for (open, close, skip) in [("P<", '>', 5), ("<", '>', 4), ("'", '\'', 4)] {
        if let Some(r) = rest.strip_prefix(open)
            && let Some(e) = r.find(close)
        {
            let name = &r[..e];
            if !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                *next_cap += 1;
                return (GroupKind::Capture(*next_cap, Some(name.to_string()), skip + name.chars().count()), skip + name.chars().count());
            }
        }
    }
    if let Some(r) = rest.strip_prefix('R').or_else(|| rest.strip_prefix('0'))
        && r.starts_with(')')
    {
        return (GroupKind::Call(CallTarget::Num(0)), 4);
    }
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if !digits.is_empty() && rest[digits.len()..].starts_with(')') {
        if let Ok(n) = digits.parse() {
            return (GroupKind::Call(CallTarget::Num(n)), 3 + digits.len());
        }
    }
    for open in ["&", "P>"] {
        if let Some(r) = rest.strip_prefix(open)
            && let Some(e) = r.find(')')
        {
            let name = &r[..e];
            if !name.is_empty() && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                return (GroupKind::Call(CallTarget::Name(name.to_string())), 2 + open.len() + name.chars().count() + 1);
            }
        }
    }
    (GroupKind::Other, 2)
}

/// Remove `(?b)`, `(?e)`, `(?be)`; from mixed flag groups only the letters b/e.
fn strip_best_enhance_flags(p: &str) -> String {
    let c: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => {
                out.extend(c[i..(i + 2).min(c.len())].iter());
                i += 2;
            }
            '[' => {
                let e = skip_class(&c, i);
                out.extend(c[i..e].iter());
                i = e;
            }
            '(' if c.get(i + 1) == Some(&'?') => {
                let flags: String = c[i + 2..].iter().take_while(|ch| ch.is_ascii_alphabetic()).collect();
                let after = c.get(i + 2 + flags.chars().count());
                if !flags.is_empty() && matches!(after, Some(')') | Some(':')) && flags.chars().any(|f| f == 'b' || f == 'e') {
                    let kept: String = flags.chars().filter(|f| *f != 'b' && *f != 'e').collect();
                    let n = 2 + flags.chars().count();
                    if kept.is_empty() && after == Some(&')') {
                        i += n + 1;
                    } else if kept.is_empty() {
                        out.push_str("(?:");
                        i += n + 1;
                    } else {
                        out.push_str("(?");
                        out.push_str(&kept);
                        i += n;
                    }
                } else {
                    out.push('(');
                    i += 1;
                }
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Fuzzy matches
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Limits {
    e: usize,
    s: usize,
    i: usize,
    d: usize,
}

/// `e<=1`, `s<2,i<=1` … → bounds. `None` if not a fuzzy expression; `Some(Err)` if a
/// fuzzy expression that is not reproduced here (costs, lower bounds).
fn parse_fuzzy(body: &str) -> Option<Result<Limits, String>> {
    let first = body.chars().next()?;
    if !"esid".contains(first) && !first.is_ascii_digit() {
        return None;
    }
    if !body.contains('<') {
        return None;
    }
    let unsupported = || Some(Err(format!("unsupported fuzzy constraint {{{body}}}")));
    let mut given: [Option<usize>; 4] = [None; 4];
    for part in body.split(',') {
        let part = part.trim();
        let mut ch = part.chars();
        let Some(kind) = ch.next() else { return unsupported() };
        let idx = match kind {
            'e' => 0,
            's' => 1,
            'i' => 2,
            'd' => 3,
            _ => return unsupported(),
        };
        let r = ch.as_str().trim();
        let (n, strict) = if let Some(n) = r.strip_prefix("<=") {
            (n, false)
        } else if let Some(n) = r.strip_prefix('<') {
            (n, true)
        } else {
            return unsupported();
        };
        let Ok(mut n) = n.trim().parse::<usize>() else { return unsupported() };
        if strict {
            if n == 0 {
                return unsupported();
            }
            n -= 1;
        }
        given[idx] = Some(n);
    }
    let e = given[0].unwrap_or_else(|| given[1..].iter().flatten().sum());
    let dflt = if given[0].is_some() { e } else { 0 };
    let l = Limits { e, s: given[1].unwrap_or(dflt).min(e), i: given[2].unwrap_or(dflt).min(e), d: given[3].unwrap_or(dflt).min(e) };
    if l.e > MAX_ERRORS {
        return Some(Err(format!("fuzzy matching supports at most {MAX_ERRORS} errors")));
    }
    Some(Ok(l))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Tok {
    Lit(char),
    Any,
}

/// Content of a fuzzy group as plain text.
fn literal_tokens(c: &[char]) -> Result<Vec<Tok>, String> {
    let err = || Err("fuzzy matching is only supported for plain text".to_string());
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' {
            let Some(&n) = c.get(i + 1) else { return err() };
            let lit = match n {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                _ if n.is_alphanumeric() => return err(),
                _ => n,
            };
            out.push(Tok::Lit(lit));
            i += 2;
            continue;
        }
        if ".^$*+?()[]{}|".contains(ch) {
            return err();
        }
        out.push(Tok::Lit(ch));
        i += 1;
    }
    Ok(out)
}

fn variants(base: Vec<Tok>, l: Limits) -> Result<Vec<Vec<Tok>>, String> {
    let mut seen: HashSet<Vec<Tok>> = HashSet::new();
    seen.insert(base.clone());
    let mut all = vec![base.clone()];
    let mut frontier = vec![(base, 0usize, 0usize, 0usize)];
    for _ in 0..l.e {
        let mut next = Vec::new();
        for (v, s, i, d) in &frontier {
            let mut push = |w: Vec<Tok>, s, i, d| {
                if seen.insert(w.clone()) {
                    all.push(w.clone());
                    next.push((w, s, i, d));
                }
            };
            if *s < l.s {
                for k in 0..v.len() {
                    if matches!(v[k], Tok::Lit(_)) {
                        let mut w = v.clone();
                        w[k] = Tok::Any;
                        push(w, s + 1, *i, *d);
                    }
                }
            }
            if *d < l.d {
                for k in 0..v.len() {
                    let mut w = v.clone();
                    w.remove(k);
                    push(w, *s, *i, d + 1);
                }
            }
            if *i < l.i {
                for k in 0..=v.len() {
                    let mut w = v.clone();
                    w.insert(k, Tok::Any);
                    push(w, *s, i + 1, *d);
                }
            }
        }
        if all.len() > MAX_VARIANTS {
            return Err("fuzzy pattern too large".into());
        }
        frontier = next;
    }
    Ok(all)
}

fn render_variants(vs: &[Vec<Tok>]) -> String {
    let mut out = String::from("(?:");
    for (n, v) in vs.iter().enumerate() {
        if n > 0 {
            out.push('|');
        }
        for t in v {
            match t {
                Tok::Lit(ch) => out.push_str(&regex::escape(&ch.to_string())),
                Tok::Any => out.push_str("(?s:.)"),
            }
        }
    }
    out.push(')');
    out
}

fn expand_fuzzy(p: &str) -> Result<String, String> {
    if !p.contains('{') {
        return Ok(p.to_string());
    }
    let c: Vec<char> = p.chars().collect();
    let mut out = String::new();
    // (start in the original, end in the original, start in `out`) of the last atom
    let mut last: Option<(usize, usize, usize)> = None;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '\\' => {
                let e = (i + 2).min(c.len());
                last = Some((i, e, out.len()));
                out.extend(c[i..e].iter());
                i = e;
            }
            '[' => {
                let e = skip_class(&c, i);
                last = Some((i, e, out.len()));
                out.extend(c[i..e].iter());
                i = e;
            }
            '(' => {
                stack.push((i, out.len()));
                out.push('(');
                last = None;
                i += 1;
            }
            ')' => {
                out.push(')');
                last = stack.pop().map(|(s, o)| (s, i + 1, o));
                i += 1;
            }
            '{' => {
                let rest: String = c[i + 1..].iter().collect();
                let fuzzy = rest.find('}').and_then(|e| parse_fuzzy(&rest[..e]).map(|r| (e, r)));
                match fuzzy {
                    Some((e, lim)) => {
                        let lim = lim?;
                        let Some((s, end, o)) = last.filter(|(_, end, _)| *end == i) else {
                            return Err("fuzzy constraint without a preceding item".into());
                        };
                        let atom = &c[s..end];
                        let replaced = if atom[0] == '(' {
                            let mut cap = 0usize;
                            let (kind, prefix) = group_kind(&c, s, &mut cap);
                            let inner = literal_tokens(&atom[prefix..atom.len() - 1])?;
                            let alts = render_variants(&variants(inner, lim)?);
                            match kind {
                                GroupKind::Capture(..) => {
                                    let head: String = atom[..prefix].iter().collect();
                                    format!("{head}{alts})")
                                }
                                GroupKind::NonCapture => alts,
                                _ => return Err("fuzzy matching is only supported for plain text".into()),
                            }
                        } else {
                            render_variants(&variants(literal_tokens(atom)?, lim)?)
                        };
                        out.truncate(o);
                        out.push_str(&replaced);
                        if out.len() > MAX_EXPANDED {
                            return Err("fuzzy pattern too large".into());
                        }
                        last = None;
                        i += 1 + rest[..e].chars().count() + 1;
                    }
                    None => {
                        out.push('{');
                        i += 1;
                    }
                }
            }
            _ => {
                last = Some((i, i + 1, out.len()));
                let mut b = [0u8; 4];
                out.push_str(ch.encode_utf8(&mut b));
                i += 1;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 2. Recursion
// ---------------------------------------------------------------------------

/// Convert capture groups to non-capturing ones (when inlining, so that numbers stay the same).
fn decapture(p: &str) -> String {
    let c: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut cap = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => {
                out.extend(c[i..(i + 2).min(c.len())].iter());
                i += 2;
            }
            '[' => {
                let e = skip_class(&c, i);
                out.extend(c[i..e].iter());
                i = e;
            }
            '(' => {
                let (kind, prefix) = group_kind(&c, i, &mut cap);
                if matches!(kind, GroupKind::Capture(..)) {
                    out.push_str("(?:");
                } else {
                    out.extend(c[i..i + prefix].iter());
                }
                i += prefix;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

struct Groups {
    /// Content per group number (index 0 = whole pattern), already without capture groups.
    content: Vec<Option<String>>,
    names: Vec<(String, usize)>,
}

fn collect_groups(p: &str) -> Groups {
    let c: Vec<char> = p.chars().collect();
    let mut content = vec![Some(decapture(p))];
    let mut names = Vec::new();
    let mut cap = 0usize;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '[' => i = skip_class(&c, i),
            '(' => {
                let (kind, prefix) = group_kind(&c, i, &mut cap);
                if let GroupKind::Capture(n, name, _) = kind {
                    if let Some(e) = group_end(&c, i) {
                        let inner: String = c[i + prefix..e].iter().collect();
                        if content.len() <= n {
                            content.resize(n + 1, None);
                        }
                        content[n] = Some(decapture(&inner));
                    }
                    if let Some(name) = name {
                        names.push((name, n));
                    }
                }
                i += prefix;
            }
            _ => i += 1,
        }
    }
    Groups { content, names }
}

fn inline_calls(p: &str, g: &Groups, depth: usize, max_depth: usize, budget: &mut usize) -> Result<String, String> {
    let c: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut cap = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => {
                out.extend(c[i..(i + 2).min(c.len())].iter());
                i += 2;
            }
            '[' => {
                let e = skip_class(&c, i);
                out.extend(c[i..e].iter());
                i = e;
            }
            '(' => {
                let (kind, prefix) = group_kind(&c, i, &mut cap);
                if let GroupKind::Call(t) = kind {
                    let n = match t {
                        CallTarget::Num(n) => n,
                        CallTarget::Name(name) => {
                            g.names.iter().find(|(k, _)| *k == name).map(|(_, n)| *n).ok_or_else(|| format!("unknown group name {name}"))?
                        }
                    };
                    let body = g.content.get(n).cloned().flatten().ok_or_else(|| format!("invalid group reference {n}"))?;
                    if depth >= max_depth {
                        out.push_str("(?!)");
                    } else {
                        out.push_str("(?:");
                        out.push_str(&inline_calls(&body, g, depth + 1, max_depth, budget)?);
                        out.push(')');
                    }
                } else {
                    out.extend(c[i..i + prefix].iter());
                }
                i += prefix;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
        if out.len() > *budget {
            return Err("recursive pattern too large".into());
        }
    }
    Ok(out)
}

fn has_calls(p: &str) -> bool {
    let c: Vec<char> = p.chars().collect();
    let mut cap = 0usize;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '[' => i = skip_class(&c, i),
            '(' => {
                let (kind, prefix) = group_kind(&c, i, &mut cap);
                if matches!(kind, GroupKind::Call(_)) {
                    return true;
                }
                i += prefix;
            }
            _ => i += 1,
        }
    }
    false
}

fn expand_recursion(p: &str) -> Result<String, String> {
    if !has_calls(p) {
        return Ok(p.to_string());
    }
    let g = collect_groups(p);
    let mut last_err = String::from("recursive pattern too large");
    for depth in (2..=MAX_DEPTH).rev() {
        let mut budget = MAX_EXPANDED;
        match inline_calls(p, &g, 0, depth, &mut budget) {
            Ok(s) => return Ok(s),
            Err(e) if e.contains("too large") => last_err = e,
            Err(e) => return Err(e),
        }
    }
    Err(last_err)
}

// ---------------------------------------------------------------------------
// 3. Lookbehind with alternatives
// ---------------------------------------------------------------------------

/// Splits `s` at top-level `|`.
fn split_top_alternatives(c: &[char]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => {
                cur.extend(c[i..(i + 2).min(c.len())].iter());
                i += 2;
                continue;
            }
            '[' => {
                let e = skip_class(c, i);
                cur.extend(c[i..e].iter());
                i = e;
                continue;
            }
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
                i += 1;
                continue;
            }
            _ => {}
        }
        cur.push(c[i]);
        i += 1;
    }
    parts.push(cur);
    parts
}

fn fix_lookbehinds(p: &str) -> Result<String, String> {
    if !p.contains("(?<=") && !p.contains("(?<!") {
        return Ok(p.to_string());
    }
    let c: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut cap = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => {
                out.extend(c[i..(i + 2).min(c.len())].iter());
                i += 2;
            }
            '[' => {
                let e = skip_class(&c, i);
                out.extend(c[i..e].iter());
                i = e;
            }
            '(' => {
                let (kind, prefix) = group_kind(&c, i, &mut cap);
                if let (GroupKind::Lookbehind(pos), Some(e)) = (&kind, group_end(&c, i)) {
                    let inner: String = c[i + prefix..e].iter().collect();
                    let inner = fix_lookbehinds(&inner)?;
                    let ic: Vec<char> = inner.chars().collect();
                    let alts = split_top_alternatives(&ic);
                    let op = if *pos { "(?<=" } else { "(?<!" };
                    if alts.len() > 1 {
                        let sep = if *pos { "|" } else { "" };
                        let pieces: Vec<String> = alts.iter().map(|a| format!("{op}{a})")).collect();
                        out.push_str(&format!("(?:{})", pieces.join(sep)));
                    } else {
                        out.push_str(&format!("{op}{inner})"));
                    }
                    i = e + 1;
                } else {
                    out.extend(c[i..i + prefix].iter());
                    i += prefix;
                }
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_parse() {
        assert_eq!(parse_fuzzy("e<=1"), Some(Ok(Limits { e: 1, s: 1, i: 1, d: 1 })));
        assert_eq!(parse_fuzzy("e<2"), Some(Ok(Limits { e: 1, s: 1, i: 1, d: 1 })));
        assert_eq!(parse_fuzzy("s<=1"), Some(Ok(Limits { e: 1, s: 1, i: 0, d: 0 })));
        assert_eq!(parse_fuzzy("e<=2,s<=1"), Some(Ok(Limits { e: 2, s: 1, i: 2, d: 2 })));
        assert!(matches!(parse_fuzzy("1i+1d<5"), Some(Err(_))));
        assert!(matches!(parse_fuzzy("e<=9"), Some(Err(_))));
        assert_eq!(parse_fuzzy("2,3"), None);
    }

    #[test]
    fn fuzzy_expand() {
        let s = expand("(?:ab){s<=1}").unwrap();
        assert_eq!(s, "(?:ab|(?s:.)b|a(?s:.))");
        let s = expand("x(ab){d<=1}y").unwrap();
        assert_eq!(s, "x((?:ab|b|a))y");
        assert!(expand(r"(?:\d+){e<=1}").is_err());
        assert!(expand("(?=ab){e<=1}").is_err());
        assert_eq!(expand("(?b)(?:a){i<=1}").unwrap(), "(?:a|(?s:.)a|a(?s:.))");
        assert_eq!(expand("(?ie)x").unwrap(), "(?i)x");
    }

    #[test]
    fn recursion_expand() {
        let s = expand(r"\((?:[^()]|(?R))*\)").unwrap();
        assert!(s.contains("(?!)") && !s.contains("(?R)"), "{s}");
        let s = expand(r"(?P<p>\[(?:[^\[\]]|(?&p))*\])x").unwrap();
        assert!(s.starts_with("(?P<p>") && !s.contains("(?&p)"), "{s}");
        assert!(expand("(?&nope)").is_err());
    }

    #[test]
    fn lookbehind_expand() {
        assert_eq!(expand("(?<=a|bc)x").unwrap(), "(?:(?<=a)|(?<=bc))x");
        assert_eq!(expand("(?<!a|bc)x").unwrap(), "(?:(?<!a)(?<!bc))x");
        assert_eq!(expand("(?<=ab)x").unwrap(), "(?<=ab)x");
        assert_eq!(expand("(?<=(?:a|bc))x").unwrap(), "(?<=(?:a|bc))x");
    }
}
