// upstream: src/documents/regex.py@v3.4.2 (validate_regex_pattern, safe_regex_search,
//           safe_regex_match, safe_regex_sub)
//
// Upstream uses the third-party Python `regex` module (VERSION0) with a 0.1 s timeout
// (PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS). Here user patterns are compiled with
// `fancy-regex` (backtracking for lookaround/backreferences/atomic groups/possessive
// quantifiers, linear-time `regex` crate for everything else).
//
// Before that, `crate::pyext::expand` resolves three `regex`-module extensions (K1): fuzzy
// matching `(?:text){e<=1}` on plain text, bounded recursion `(?R)`/`(?1)`/`(?&name)` and
// lookbehinds with alternatives of different fixed length.
//
// Translation of Python syntax that means something different in Rust:
//   - `\Z`               -> `\z`              (absolute end)
//   - `$` (no MULTILINE) -> `(?=\n?\z)`       (end or before a final newline)
//   - `{,n}`             -> `{0,n}`
//   - `{` that is not a quantifier -> `\{` (Python treats it literally)
//   - inside `[...]`: `[` (not POSIX `[:`), `&`, `~` are literal in Python -> escaped
// The word/digit/space classes of the `regex` module are identical to Rust's
// Unicode classes (verified: `\w` == [\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control}]).
//
// Timeout (K1): `MATCH_REGEX_TIMEOUT_SECONDS` (env `PAPERRUST_/PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS`,
// default 0.1) is a real wall-clock limit for patterns that need backtracking: the search
// runs on a helper thread and the caller stops waiting after the timeout ("timed out"
// warning, no match). The helper itself is bounded by a backtrack budget scaled to the
// timeout, so an abandoned search always ends. Patterns the linear `regex` crate accepts
// run inline (linear time, no timeout needed).
//
// Remaining limitation: fancy-regex has no cancellation hook, so a helper the caller gave up
// on keeps running until it finishes or exhausts its backtrack budget (about the timeout at
// ~50 M steps/s, longer on a slow CPU). To keep such runaways from piling up under load, at
// most `HELPER_LIMIT` helpers exist at once; when all are busy (typically with abandoned
// searches) the search runs inline on the caller's thread, bounded by the same budget. That is
// backpressure instead of unbounded thread growth.

use crate::pyre::shorten;

/// Minimum backtracking budget (bounds abandoned helper threads, see [`backtrack_limit`]).
pub const BACKTRACK_LIMIT: usize = 1_000_000;

/// Upper bound for the scaled budget (about a minute of backtracking).
const MAX_BACKTRACK_LIMIT: usize = 5_000_000_000;

/// `settings.MATCH_REGEX_TIMEOUT_SECONDS` (`get_float_from_env(..., 0.1)`); invalid -> 0.1,
/// zero or negative -> no wall-clock limit (only the backtrack budget).
pub fn timeout_seconds() -> f64 {
    let raw =
        ["PAPERRUST_MATCH_REGEX_TIMEOUT_SECONDS", "PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS", "PAPERLESS_RS_MATCH_REGEX_TIMEOUT_SECONDS"]
            .iter()
            .find_map(|k| pl_envlive::var(k));
    match raw.map(|v| v.trim().parse::<f64>()) {
        Some(Ok(v)) if v.is_finite() && v > 0.0 => v,
        Some(Ok(_)) => 0.0,
        Some(Err(_)) => {
            tracing::warn!(target: "paperless.regex", "Invalid MATCH_REGEX_TIMEOUT_SECONDS, using 0.1");
            0.1
        }
        None => 0.1,
    }
}

/// Backtrack budget for a timeout: generous (about 50 M steps per second, never below
/// [`BACKTRACK_LIMIT`]) so the wall clock decides, but finite so helper threads end.
pub fn backtrack_limit(timeout: f64) -> usize {
    let scaled = (timeout * 50_000_000.0).min(MAX_BACKTRACK_LIMIT as f64);
    (scaled as usize).max(BACKTRACK_LIMIT)
}

/// Compiled user pattern plus whether it can run without backtracking.
#[derive(Debug)]
pub struct Compiled {
    pub re: fancy_regex::Regex,
    /// `true` when the linear `regex` crate accepts the translated pattern (no timeout needed).
    pub linear: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegexError {
    /// Compile error (upstream: regex.error -> ValueError(exc.msg)).
    Invalid(String),
    /// Backtrack limit exceeded (upstream: TimeoutError).
    Timeout,
}

/// Translate a Python `regex`-module pattern into fancy-regex syntax.
pub fn translate_python_pattern(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let multiline = has_inline_flag(pattern, 'm');
    let verbose = has_inline_flag(pattern, 'x');
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    let mut in_class = false;
    let mut class_start = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            if i + 1 < chars.len() {
                let n = chars[i + 1];
                if n == 'Z' && !in_class {
                    out.push_str(r"\z");
                } else {
                    out.push(c);
                    out.push(n);
                }
                i += 2;
            } else {
                out.push(c);
                i += 1;
            }
            continue;
        }
        if in_class {
            match c {
                ']' if i > class_start => {
                    in_class = false;
                    out.push(c);
                }
                '[' => {
                    if chars.get(i + 1) == Some(&':') {
                        // POSIX class [:alpha:]
                        let rest: String = chars[i..].iter().collect();
                        if let Some(end) = rest.find(":]") {
                            out.push_str(&rest[..end + 2]);
                            i += rest[..end + 2].chars().count();
                            continue;
                        }
                    }
                    out.push_str(r"\[");
                }
                '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
            i += 1;
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push(c);
                i += 1;
                // a leading '^' and a leading ']' belong to the class
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                class_start = i;
                if chars.get(i) == Some(&']') {
                    out.push_str(r"\]");
                    i += 1;
                }
                continue;
            }
            '$' if !multiline && !verbose => out.push_str(r"(?=\n?\z)"),
            '{' => {
                // valid quantifier forms: {n} {n,} {n,m} {,m}
                let rest: String = chars[i + 1..].iter().collect();
                if let Some(close) = rest.find('}') {
                    let body = &rest[..close];
                    let valid = !body.is_empty()
                        && body.chars().all(|c| c.is_ascii_digit() || c == ',')
                        && body.matches(',').count() <= 1
                        && body != ",";
                    if valid && i > 0 {
                        if let Some(stripped) = body.strip_prefix(',') {
                            out.push_str(&format!("{{0,{stripped}}}"));
                        } else {
                            out.push('{');
                            out.push_str(body);
                            out.push('}');
                        }
                        i += 1 + body.chars().count() + 1;
                        continue;
                    }
                }
                out.push_str(r"\{");
            }
            '}' => out.push_str(r"\}"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

fn has_inline_flag(pattern: &str, flag: char) -> bool {
    let mut rest = pattern;
    while let Some(pos) = rest.find("(?") {
        let after = &rest[pos + 2..];
        let flags: String = after.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let next = after[flags.len()..].chars().next();
        if !flags.is_empty() && matches!(next, Some(')') | Some(':') | Some('-')) && flags.contains(flag) {
            return true;
        }
        rest = after;
    }
    false
}

/// Compile a user pattern like `regex.compile(pattern, flags)`.
pub fn compile(pattern: &str, ignore_case: bool) -> Result<fancy_regex::Regex, RegexError> {
    compile_with(pattern, ignore_case, BACKTRACK_LIMIT)
}

/// Expand Python `regex` extensions (K1, [`crate::pyext`]) and translate to fancy-regex syntax.
/// `anchored` = `regex.match` (anchored at the text start; `(?R)` still means the pattern without the anchor).
pub fn prepare_pattern(pattern: &str, anchored: bool) -> Result<String, RegexError> {
    let expanded = crate::pyext::expand(pattern).map_err(RegexError::Invalid)?;
    let translated = translate_python_pattern(&expanded);
    Ok(if anchored { format!(r"\A(?:{translated})") } else { translated })
}

fn compile_with(pattern: &str, ignore_case: bool, limit: usize) -> Result<fancy_regex::Regex, RegexError> {
    build(&prepare_pattern(pattern, false)?, ignore_case, limit)
}

fn build(translated: &str, ignore_case: bool, limit: usize) -> Result<fancy_regex::Regex, RegexError> {
    // Inline (?i) instead of RegexBuilder::case_insensitive: fancy-regex 0.14 drops that flag as
    // soon as the pattern needs its backtracking engine (lookaround, atomic groups), so
    // `Invoice(?= )` did not match "invoice 1" although Python's re.IGNORECASE does.
    let src = if ignore_case { format!("(?i){translated}") } else { translated.to_string() };
    fancy_regex::RegexBuilder::new(&src).backtrack_limit(limit).build().map_err(|e| RegexError::Invalid(e.to_string()))
}

/// Compiled user patterns, keyed by (pattern, anchored, ignore_case, budget) (PERF-SCALE Q8, [`crate::cache`]).
static COMPILED: crate::cache::PatternCache<(String, bool, bool, usize), Compiled> = crate::cache::PatternCache::new();

/// [`compile`] through the process-wide cache, with the budget of the current timeout.
pub fn compile_cached(pattern: &str, ignore_case: bool) -> Result<std::sync::Arc<Compiled>, RegexError> {
    compile_cached_as(pattern, false, ignore_case)
}

fn compile_cached_as(pattern: &str, anchored: bool, ignore_case: bool) -> Result<std::sync::Arc<Compiled>, RegexError> {
    let limit = backtrack_limit(timeout_seconds());
    COMPILED.get_or_try((pattern.to_string(), anchored, ignore_case, limit), || {
        let prepared = prepare_pattern(pattern, anchored)?;
        let re = build(&prepared, ignore_case, limit)?;
        let linear = regex::RegexBuilder::new(&prepared).case_insensitive(ignore_case).build().is_ok();
        Ok(Compiled { re, linear })
    })
}

/// Outcome of a bounded search.
enum Run<T> {
    Done(T),
    TimedOut,
}

/// Counter of live helper threads with a fixed upper bound.
struct HelperPool {
    in_flight: std::sync::atomic::AtomicUsize,
    max: usize,
}

impl HelperPool {
    const fn new(max: usize) -> Self {
        Self { in_flight: std::sync::atomic::AtomicUsize::new(0), max }
    }

    /// Reserve a helper slot; `None` when all `max` slots are taken.
    fn try_acquire(&'static self) -> Option<HelperSlot> {
        use std::sync::atomic::Ordering;
        let mut n = self.in_flight.load(Ordering::Acquire);
        while n < self.max {
            match self.in_flight.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(HelperSlot(self)),
                Err(cur) => n = cur,
            }
        }
        None
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        self.in_flight.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// Releases its slot when the helper thread ends (also on panic).
struct HelperSlot(&'static HelperPool);

impl Drop for HelperSlot {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// At most this many regex helper threads at once (process-wide).
const HELPER_LIMIT: usize = 8;

static HELPERS: HelperPool = HelperPool::new(HELPER_LIMIT);

/// `compiled.search(text, timeout=...)`: inline for linear patterns, otherwise on a helper thread
/// with a wall-clock limit. `Err` from fancy-regex (budget exhausted) counts as timeout.
fn run_bounded<T: Send + 'static>(c: &std::sync::Arc<Compiled>, text: &str, f: fn(&fancy_regex::Regex, &str) -> Result<T, ()>) -> Run<T> {
    run_bounded_in(&HELPERS, timeout_seconds(), c, text, f)
}

fn run_bounded_in<T: Send + 'static>(
    pool: &'static HelperPool,
    timeout: f64,
    c: &std::sync::Arc<Compiled>,
    text: &str,
    f: fn(&fancy_regex::Regex, &str) -> Result<T, ()>,
) -> Run<T> {
    let inline = |c: &Compiled| match f(&c.re, text) {
        Ok(v) => Run::Done(v),
        Err(_) => Run::TimedOut,
    };
    if c.linear || timeout <= 0.0 {
        return inline(c);
    }
    // all helpers busy (e.g. with abandoned runaway searches): run here, bounded by the budget
    let Some(slot) = pool.try_acquire() else {
        tracing::debug!(target: "paperless.regex", "all regex helper threads busy, matching inline");
        return inline(c);
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let (c2, owned) = (std::sync::Arc::clone(c), text.to_string());
    let spawned = std::thread::Builder::new().name("pl-regex".into()).spawn(move || {
        let _slot = slot;
        let _ = tx.send(f(&c2.re, &owned));
    });
    if spawned.is_err() {
        // no thread available: run here (bounded by the backtrack budget)
        return inline(c);
    }
    match rx.recv_timeout(std::time::Duration::from_secs_f64(timeout)) {
        Ok(Ok(v)) => Run::Done(v),
        Ok(Err(_)) | Err(_) => Run::TimedOut,
    }
}

fn find_owned(re: &fancy_regex::Regex, text: &str) -> Result<Option<RegexMatch>, ()> {
    re.find(text).map(|m| m.map(|m| RegexMatch { start: m.start(), end: m.end(), text: m.as_str().to_string() })).map_err(|_| ())
}

/// upstream `validate_regex_pattern`: Err(message) on compile failure.
pub fn validate_regex_pattern(pattern: &str) -> Result<(), String> {
    compile(pattern, false).map(|_| ()).map_err(|e| match e {
        RegexError::Invalid(m) => m,
        RegexError::Timeout => "timeout".into(),
    })
}

fn log_compile_error(pattern: &str, msg: &str) {
    tracing::error!(
        target: "paperless.regex",
        "Error while processing regular expression {}: {}",
        shorten(pattern, 80),
        msg
    );
}

fn log_timeout(pattern: &str, what: &str) {
    tracing::warn!(
        target: "paperless.regex",
        "Regular expression {} timed out for pattern {}",
        what,
        shorten(pattern, 80)
    );
}

/// A successful search: matched text and byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexMatch {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// upstream `safe_regex_search`: errors and timeouts are logged and yield `None`.
pub fn safe_regex_search(pattern: &str, text: &str, ignore_case: bool) -> Option<RegexMatch> {
    let re = match compile_cached(pattern, ignore_case) {
        Ok(r) => r,
        Err(RegexError::Invalid(m)) => {
            log_compile_error(pattern, &m);
            return None;
        }
        Err(RegexError::Timeout) => return None,
    };
    match run_bounded(&re, text, find_owned) {
        Run::Done(m) => m,
        Run::TimedOut => {
            // upstream logs "Regular expression matching timed out for pattern %s"
            log_timeout(pattern, "matching");
            None
        }
    }
}

/// upstream `safe_regex_match` (anchored at the start of `text`).
pub fn safe_regex_match(pattern: &str, text: &str, ignore_case: bool) -> Option<RegexMatch> {
    let re = match compile_cached_as(pattern, true, ignore_case) {
        Ok(r) => r,
        Err(RegexError::Invalid(m)) => {
            log_compile_error(pattern, &m);
            return None;
        }
        Err(RegexError::Timeout) => return None,
    };
    match run_bounded(&re, text, find_owned) {
        Run::Done(m) => m,
        Run::TimedOut => {
            log_timeout(pattern, "matching");
            None
        }
    }
}

/// Translate a Python replacement template (`\1`, `\g<1>`, `\g<name>`, `\n`, `\\`) into
/// a literal-safe fancy-regex/`regex` replacement string (`${1}`, `$$`).
pub fn translate_replacement(repl: &str) -> String {
    let chars: Vec<char> = repl.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '$' => out.push_str("$$"),
            '\\' if i + 1 < chars.len() => {
                let n = chars[i + 1];
                if n.is_ascii_digit() {
                    let mut j = i + 1;
                    let mut num = String::new();
                    while j < chars.len() && chars[j].is_ascii_digit() && num.len() < 2 {
                        num.push(chars[j]);
                        j += 1;
                    }
                    out.push_str(&format!("${{{num}}}"));
                    i = j;
                    continue;
                }
                if n == 'g' && chars.get(i + 2) == Some(&'<') {
                    if let Some(close) = chars[i + 3..].iter().position(|&c| c == '>') {
                        let name: String = chars[i + 3..i + 3 + close].iter().collect();
                        out.push_str(&format!("${{{name}}}"));
                        i += 3 + close + 1;
                        continue;
                    }
                }
                let lit = match n {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    '\\' => Some('\\'),
                    _ => None,
                };
                match lit {
                    Some(l) => out.push(l),
                    None => {
                        out.push('\\');
                        out.push(n);
                    }
                }
                i += 2;
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// upstream `safe_regex_sub`: `None` on error/timeout.
pub fn safe_regex_sub(pattern: &str, repl: &str, text: &str, ignore_case: bool) -> Option<String> {
    let re = match compile(pattern, ignore_case) {
        Ok(r) => r,
        Err(RegexError::Invalid(m)) => {
            log_compile_error(pattern, &m);
            return None;
        }
        Err(RegexError::Timeout) => return None,
    };
    let rep = translate_replacement(repl);
    match re.try_replacen(text, 0, rep.as_str()) {
        Ok(s) => Some(s.into_owned()),
        Err(_) => {
            log_timeout(pattern, "substitution");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_case_also_with_lookaround_and_atomic_groups() {
        for (pat, hay) in [("Invoice(?= )", "invoice 1"), ("(?<=x)ABC", "xabc"), ("ABC", "abc"), ("(?>AB)C", "abc")] {
            assert_eq!(compile(pat, true).unwrap().is_match(hay).ok(), Some(true), "{pat} on {hay}");
            assert_eq!(compile(pat, false).unwrap().is_match(hay).ok(), Some(false), "{pat} on {hay} (case-sensitive)");
        }
    }

    #[test]
    fn k1_wall_clock_timeout() {
        // catastrophic backtracking (backreference forces fancy-regex): must give up quickly
        let text = format!("{}!", "a".repeat(40));
        let t0 = std::time::Instant::now();
        assert!(safe_regex_search(r"^(a|aa)+\1$", &text, false).is_none());
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "{:?}", t0.elapsed());
        // linear patterns run inline and still match
        assert!(compile_cached("abc", false).map(|c| c.linear).unwrap_or(false));
        assert!(!compile_cached(r"(a)\1", false).map(|c| c.linear).unwrap_or(true));
        assert_eq!(safe_regex_search(r"(b)\1", "abbc", false).map(|m| m.start), Some(1));
        assert_eq!(backtrack_limit(0.1), 5_000_000);
        assert_eq!(backtrack_limit(0.0), BACKTRACK_LIMIT);
    }

    #[test]
    fn translation() {
        assert_eq!(translate_python_pattern(r"abc\Z"), r"abc\z");
        assert_eq!(translate_python_pattern(r"abc$"), r"abc(?=\n?\z)");
        assert_eq!(translate_python_pattern(r"(?m)abc$"), r"(?m)abc$");
        assert_eq!(translate_python_pattern(r"a{,3}"), r"a{0,3}");
        assert_eq!(translate_python_pattern(r"a{2,3}"), r"a{2,3}");
        assert_eq!(translate_python_pattern(r"{foo}"), r"\{foo\}");
        assert_eq!(translate_python_pattern(r"[a&&b]"), r"[a\&\&b]");
        assert_eq!(translate_python_pattern(r"[][]"), r"[\]\[]");
        assert_eq!(translate_python_pattern(r"[[:alpha:]]"), r"[[:alpha:]]");
        assert_eq!(translate_python_pattern(r"[$]"), r"[$]");
    }

    #[test]
    fn python_end_anchor() {
        assert!(safe_regex_search(r"abc$", "xabc\n", false).is_some());
        assert!(safe_regex_search(r"abc\Z", "xabc\n", false).is_none());
        assert!(safe_regex_search(r"abc$", "xabc\nd", false).is_none());
    }

    #[test]
    fn lookaround_and_backrefs() {
        assert!(safe_regex_search(r"(?<=foo)bar", "foobar", false).is_some());
        assert!(safe_regex_search(r"(\w)\1", "hello", false).is_some());
        assert!(safe_regex_search(r"(?P<x>a)(?P=x)", "baab", false).is_some());
        assert!(safe_regex_search(r"[", "x", false).is_none());
    }

    #[test]
    fn k1_python_regex_extensions() {
        // fuzzy: one typo (OCR) still matches, two do not
        assert!(safe_regex_search(r"(?:rechnung){e<=1}", "Ihre Rechnunq vom", true).is_some());
        assert!(safe_regex_search(r"(?:rechnung){e<=1}", "Ihre Reclmunq vom", true).is_none());
        assert!(safe_regex_search(r"\b(?:invoice){e<=2}\b", "lnvoce 42", true).is_some());
        // like the regex module without BESTMATCH: first (leftmost) match, even if it contains errors
        assert_eq!(safe_regex_search(r"(?:abc){e<=1}", "xxabcxx", false).map(|m| m.text), Some("xabc".into()));
        assert!(validate_regex_pattern(r"(?:abc){1i+1d<5}").is_err());
        // Recursion: balanced brackets
        assert_eq!(safe_regex_match(r"\((?:[^()]|(?R))*\)", "(a(b)c)", false).map(|m| m.text), Some("(a(b)c)".into()));
        assert_eq!(safe_regex_search(r"\((?:[^()]|(?R))*\)", "x(a(b(c))", false).map(|m| m.text), Some("(b(c))".into()));
        assert!(safe_regex_search(r"^(?P<b>\[(?:[^\[\]]|(?&b))*\])$", "[[x][y]]", false).is_some());
        // variable lookbehinds (alternatives of fixed length)
        assert!(safe_regex_search(r"(?<=Nr\.|Nummer) ?\d+", "Nummer 12", false).is_some());
        assert!(safe_regex_search(r"(?<!Nr\.|Nummer) \d+", "Nummer 12", false).is_none());
        assert!(safe_regex_search(r"(?<!Nr\.|Nummer) \d+", "Seite 12", false).is_some());
        // unbounded lookbehinds stay invalid
        assert!(validate_regex_pattern(r"(?<=a+)b").is_err());
    }

    #[test]
    fn replacement() {
        assert_eq!(translate_replacement(r"\1 month"), "${1} month");
        assert_eq!(translate_replacement(r"\g<n>$"), "${n}$$");
        assert_eq!(safe_regex_sub(r"(\d+)h", r"\1:00", "5h", false).as_deref(), Some("5:00"));
    }

    /// Stand-in for a runaway backtracking search: takes 400 ms, then reports "no match".
    fn slow_find(_: &fancy_regex::Regex, _: &str) -> Result<Option<RegexMatch>, ()> {
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(None)
    }

    #[test]
    fn abandoned_helpers_cannot_pile_up() {
        // Regression: every timed-out search left its helper thread running and the next search
        // spawned another one, so runaway threads accumulated without bound under load.
        static POOL: HelperPool = HelperPool::new(1);
        let c = compile_cached(r"(a)\1", false).unwrap();
        assert!(!c.linear);
        // first search times out after 20 ms; its helper keeps running (holds the only slot)
        assert!(matches!(run_bounded_in(&POOL, 0.02, &c, "aa", slow_find), Run::TimedOut));
        assert_eq!(POOL.in_flight(), 1);
        // second search: no second helper, it runs inline and therefore completes
        let t0 = std::time::Instant::now();
        assert!(matches!(run_bounded_in(&POOL, 0.02, &c, "aa", slow_find), Run::Done(None)));
        assert!(t0.elapsed() >= std::time::Duration::from_millis(300), "ran inline, not on a helper");
        assert!(POOL.in_flight() <= 1);
        // the abandoned helper ends and frees its slot; then helpers are used again
        let t0 = std::time::Instant::now();
        while POOL.in_flight() > 0 && t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(POOL.in_flight(), 0);
        assert!(matches!(run_bounded_in(&POOL, 2.0, &c, "aa", find_owned), Run::Done(Some(_))));
    }
}
