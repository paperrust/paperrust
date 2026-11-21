//! Sentence splitting like nltk 3.9 `PunktSentenceTokenizer()` **without training** (empty
//! `PunktParameters`), as llama_index `split_by_sentence_tokenizer()` uses it
//! (`globals_helper.punkt_tokenizer`, `span_tokenize` + "start of the next sentence as the end").
//!
//! Source: `nltk/tokenize/punkt.py` (version in `reference/venv`): `PunktLanguageVars`
//! (`word_tokenize`, `period_context_re`, `re_boundary_realignment`), `PunktToken`,
//! `_first_pass_annotation`, `_second_pass_annotation`, `_ortho_heuristic`,
//! `_match_potential_end_contexts`, `_slices_from_text`, `_realign_boundaries`.
//!
//! With empty parameters the algorithm simplifies exactly to:
//! * every token `.`/`?`/`!` and every token ending in `.` (not `..`) is a sentence end;
//! * exception (second pass): initial (`J.`) or number with a dot (`12.`) before a
//!   lowercase word or punctuation `;:,.!?` (and initial before a capitalized
//!   word) are not a sentence end.
//!
//! Regular expressions with lookahead are rebuilt by hand (Python `\s` = `str.isspace`).

use crate::compat::{py_is_word, py_isspace};

const SENT_END: [char; 3] = ['.', '?', '!'];

/// `_re_non_word_chars`: `[)";}\]*:@'({[‘’“”«»?!]`.
fn is_non_word(c: char) -> bool {
    matches!(
        c,
        ')' | '"'
            | ';'
            | '}'
            | ']'
            | '*'
            | ':'
            | '@'
            | '\''
            | '('
            | '{'
            | '['
            | '\u{2018}'
            | '\u{2019}'
            | '\u{201c}'
            | '\u{201d}'
            | '\u{ab}'
            | '\u{bb}'
            | '?'
            | '!'
    )
}

/// `_re_word_start`: everything except `(" `{[:;&#*@)}]-,`.
fn is_word_start(c: char) -> bool {
    !matches!(c, '(' | '"' | '`' | '{' | '[' | ':' | ';' | '&' | '#' | '*' | '@' | ')' | '}' | ']' | '-' | ',')
}

/// Length (in characters) of a multi-character punctuation `(?:\-{2,}|\.{2,}|(?:\.\s){2,}\.)` at `i`.
fn multi_char_at(c: &[char], i: usize) -> Option<usize> {
    let run = |ch: char| c[i..].iter().take_while(|x| **x == ch).count();
    if c.get(i) == Some(&'-') {
        let n = run('-');
        if n >= 2 {
            return Some(n);
        }
        return None;
    }
    if c.get(i) == Some(&'.') {
        let n = run('.');
        if n >= 2 {
            return Some(n);
        }
        // (?:\.\s){2,}\.  (greedy)
        let mut j = i;
        let mut reps = 0;
        while j + 1 < c.len() && c[j] == '.' && py_isspace(c[j + 1]) {
            j += 2;
            reps += 1;
        }
        // Back off until a '.' follows (backtracking of {2,})
        while reps >= 2 {
            if c.get(i + reps * 2) == Some(&'.') {
                return Some(reps * 2 + 1);
            }
            reps -= 1;
        }
    }
    None
}

/// Word-end lookahead at position `j`: `\s|$|NonWord|MultiChar|,(?=$|\s|NonWord|MultiChar)`.
fn word_end_at(c: &[char], j: usize) -> bool {
    match c.get(j) {
        None => true,
        Some(&ch) if py_isspace(ch) || is_non_word(ch) => true,
        Some(_) if multi_char_at(c, j).is_some() => true,
        Some(&',') => match c.get(j + 1) {
            None => true,
            Some(&n) => py_isspace(n) || is_non_word(n) || multi_char_at(c, j + 1).is_some(),
        },
        _ => false,
    }
}

/// `PunktLanguageVars.word_tokenize(line)` (`findall` of the word regex).
pub fn word_tokenize(line: &str) -> Vec<String> {
    let c: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if let Some(n) = multi_char_at(&c, i) {
            out.push(c[i..i + n].iter().collect());
            i += n;
            continue;
        }
        if py_isspace(c[i]) {
            i += 1;
            continue;
        }
        if is_word_start(c[i]) {
            // \S+? up to the first word end
            let mut j = i + 1;
            let mut found = None;
            while j <= c.len() {
                if word_end_at(&c, j) {
                    found = Some(j);
                    break;
                }
                if j < c.len() && py_isspace(c[j]) {
                    break;
                }
                j += 1;
            }
            if let Some(j) = found {
                out.push(c[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        // \S
        out.push(c[i].to_string());
        i += 1;
    }
    out
}

struct Tok {
    tok: String,
    sentbreak: bool,
    abbr: bool,
    ellipsis: bool,
}

fn re(pat: &'static str, cell: &'static std::sync::OnceLock<Option<regex::Regex>>) -> Option<&'static regex::Regex> {
    cell.get_or_init(|| regex::Regex::new(pat).ok()).as_ref()
}

/// `_RE_NUMERIC.sub("##number##", tok.lower())` matches (then `type == "##number##"`).
fn is_numeric_type(lower: &str) -> bool {
    static R: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    re(r"^-?[\.,]?\d[\d,\.-]*\.?$", &R).is_some_and(|r| r.is_match(lower))
}

/// Python `\d` (Unicode Nd).
fn is_decimal(ch: char) -> bool {
    static R: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let mut b = [0u8; 4];
    re(r"^\d$", &R).is_some_and(|r| r.is_match(ch.encode_utf8(&mut b)))
}

fn first_upper(t: &str) -> bool {
    t.chars().next().is_some_and(char::is_uppercase)
}
fn first_lower(t: &str) -> bool {
    t.chars().next().is_some_and(char::is_lowercase)
}

/// `_RE_INITIAL`: `[^\W\d]\.$` (re.match).
fn is_initial(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    c.len() == 2 && c[1] == '.' && py_is_word(c[0]) && !is_decimal(c[0])
}

/// `_ortho_heuristic` with empty `ortho_context`: `Some(false)` = not a sentence start, `None` = unknown.
fn ortho_heuristic(next: &Tok) -> Option<bool> {
    if [";", ":", ",", ".", "!", "?"].contains(&next.tok.as_str()) {
        return Some(false);
    }
    if first_lower(&next.tok) {
        return Some(false);
    }
    None
}

fn annotate(words: Vec<String>) -> Vec<Tok> {
    let mut toks: Vec<Tok> = words
        .into_iter()
        .map(|w| {
            let mut t = Tok { tok: w, sentbreak: false, abbr: false, ellipsis: false };
            let s = t.tok.as_str();
            let is_ellipsis = s.chars().count() >= 2 && s.chars().all(|c| c == '.');
            if s.len() == 1 && SENT_END.contains(&s.chars().next().unwrap_or(' ')) {
                t.sentbreak = true;
            } else if is_ellipsis {
                t.ellipsis = true;
            } else if s.ends_with('.') && !s.ends_with("..") {
                t.sentbreak = true; // abbrev_types empty
            }
            t
        })
        .collect();
    for i in 0..toks.len() {
        if i + 1 >= toks.len() {
            break;
        }
        if !toks[i].tok.ends_with('.') {
            continue;
        }
        let lower = toks[i].tok.to_lowercase();
        let typ_number = is_numeric_type(&lower);
        let initial = is_initial(&toks[i].tok);
        // 4.2: abbr/ellipsis – without parameters never an additional sentence end
        if initial || typ_number {
            let o = ortho_heuristic(&toks[i + 1]);
            if o == Some(false) || (o.is_none() && initial && first_upper(&toks[i + 1].tok)) {
                toks[i].sentbreak = false;
                toks[i].abbr = true;
            }
        }
    }
    toks
}

/// `text_contains_sentbreak(context)`.
fn text_contains_sentbreak(text: &str) -> bool {
    let mut words = Vec::new();
    for line in text.split('\n') {
        if !crate::tokenizer::py_strip(line).is_empty() {
            words.extend(word_tokenize(line));
        }
    }
    let toks = annotate(words);
    let mut found = false;
    for t in &toks {
        if found {
            return true;
        }
        if t.sentbreak {
            found = true;
        }
    }
    false
}

struct PeriodMatch {
    start: usize,
    end: usize,
    after_tok_end: usize,
    next_tok: Option<usize>,
}

/// `period_context_re().finditer(text)`.
fn period_matches(c: &[char]) -> Vec<PeriodMatch> {
    let mut out = Vec::new();
    for i in 0..c.len() {
        if !SENT_END.contains(&c[i]) {
            continue;
        }
        let j = i + 1;
        match c.get(j) {
            Some(&n) if is_non_word(n) => out.push(PeriodMatch { start: i, end: j, after_tok_end: j + 1, next_tok: None }),
            Some(&n) if py_isspace(n) => {
                let mut k = j;
                while k < c.len() && py_isspace(c[k]) {
                    k += 1;
                }
                if k < c.len() {
                    let ns = k;
                    while k < c.len() && !py_isspace(c[k]) {
                        k += 1;
                    }
                    out.push(PeriodMatch { start: i, end: j, after_tok_end: k, next_tok: Some(ns) });
                }
            }
            _ => {}
        }
    }
    out
}

fn last_ascii_ws(c: &[char]) -> usize {
    for i in (0..c.len()).rev() {
        if matches!(c[i], ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            return i;
        }
    }
    0
}

/// `_slices_from_text` (character indices).
fn slices_from_text(c: &[char]) -> Vec<(usize, usize)> {
    let s = |a: usize, b: usize| -> String { c[a..b.max(a)].iter().collect() };
    // _match_potential_end_contexts
    let mut contexts: Vec<(usize, String)> = Vec::new(); // (index in matches, context)
    let matches = period_matches(c);
    let mut prev_slice = (0usize, 0usize);
    let mut prev: Option<usize> = None;
    for (mi, m) in matches.iter().enumerate() {
        let before = &c[prev_slice.1.min(m.start)..m.start];
        let mut idx = last_ascii_ws(before);
        if idx != 0 {
            idx += prev_slice.1 + 1;
        } else {
            idx = prev_slice.0;
        }
        let word = (idx, m.start);
        if let Some(p) = prev
            && prev_slice.1 <= word.0
        {
            let pm = &matches[p];
            contexts.push((p, format!("{}{}{}", s(prev_slice.0, prev_slice.1), s(pm.start, pm.end), s(pm.end, pm.after_tok_end))));
        }
        prev = Some(mi);
        prev_slice = word;
    }
    if let Some(p) = prev {
        let pm = &matches[p];
        contexts.push((p, format!("{}{}{}", s(prev_slice.0, prev_slice.1), s(pm.start, pm.end), s(pm.end, pm.after_tok_end))));
    }
    let mut out = Vec::new();
    let mut last_break = 0usize;
    for (mi, ctx) in contexts {
        if text_contains_sentbreak(&ctx) {
            let m = &matches[mi];
            out.push((last_break, m.end));
            last_break = m.next_tok.unwrap_or(m.end);
        }
    }
    // len(text.rstrip())
    let mut end = c.len();
    while end > 0 && py_isspace(c[end - 1]) {
        end -= 1;
    }
    out.push((last_break, end));
    out
}

fn is_realign_char(ch: char) -> bool {
    matches!(ch, '"' | '\'' | ')' | ']' | '}' | '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}' | '\u{ab}' | '\u{bb}')
}

/// `re_boundary_realignment.match(text[s2])` → (length of group(0).rstrip(), m.end()).
fn realign_match(c: &[char]) -> Option<(usize, usize)> {
    let mut k = 0;
    while k < c.len() && is_realign_char(c[k]) {
        k += 1;
        // after k characters: (?:\s+|(?=--)|$)  ($ with MULTILINE)
        match c.get(k) {
            Some(&n) if py_isspace(n) => {
                let mut e = k;
                while e < c.len() && py_isspace(c[e]) {
                    e += 1;
                }
                return Some((k, e));
            }
            None => return Some((k, k)),
            Some('-') if c.get(k + 1) == Some(&'-') => return Some((k, k)),
            _ => {}
        }
    }
    None
}

/// `span_tokenize(text)` (character indices).
pub fn span_tokenize(text: &str) -> Vec<(usize, usize)> {
    let c: Vec<char> = text.chars().collect();
    let slices = slices_from_text(&c);
    let mut out = Vec::new();
    let mut realign = 0usize;
    for i in 0..slices.len() {
        let s1 = (slices[i].0 + realign, slices[i].1);
        let nonempty = |a: (usize, usize)| a.1 > a.0;
        let Some(&s2) = slices.get(i + 1) else {
            if nonempty(s1) {
                out.push(s1);
            }
            continue;
        };
        let seg = &c[s2.0.min(c.len())..s2.1.min(c.len()).max(s2.0.min(c.len()))];
        if let Some((rlen, end)) = realign_match(seg) {
            out.push((s1.0, s2.0 + rlen));
            realign = end;
        } else {
            realign = 0;
            if nonempty(s1) {
                out.push(s1);
            }
        }
    }
    out
}

/// llama_index `split_by_sentence_tokenizer_internal(text, PunktSentenceTokenizer())`.
pub fn split_sentences(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let spans = span_tokenize(text);
    let mut out = Vec::with_capacity(spans.len());
    for (i, sp) in spans.iter().enumerate() {
        let start = sp.0;
        let end = spans.get(i + 1).map(|n| n.0).unwrap_or(c.len());
        out.push(c[start.min(c.len())..end.max(start).min(c.len())].iter().collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(word_tokenize("Hello, world."), vec!["Hello", ",", "world."]);
        assert_eq!(word_tokenize("(a) b--c..."), vec!["(", "a", ")", "b", "--", "c", "..."]);
    }

    #[test]
    fn sentences() {
        assert_eq!(split_sentences("Hello world. This is it."), vec!["Hello world. ", "This is it."]);
        assert_eq!(split_sentences("J. Smith went home."), vec!["J. Smith went home."]);
    }
}
