// upstream: dateparser 1.4.3 (reference/venv) – the subset used by paperless-ngx v3.4.2:
//   dateparser/languages/loader.py   (LocaleDataLoader._load_data, _isvalidlocale)
//   dateparser/languages/locale.py   (Locale.is_applicable, translate, _translate_numerals,
//                                     _simplify, _join, _clear_future_words)
//   dateparser/languages/dictionary.py (Dictionary/NormalizedDictionary split,
//                                     _split_by_known_words, are_tokens_valid)
//   dateparser/utils/__init__.py     (normalize_unicode)
//
// The per-locale data is NOT re-derived here: tools/gen_dateparser_data.py dumps the
// structures dateparser builds at runtime (NORMALIZE=True, SKIP_TOKENS=["t"]) into
// data/dateparser.json, which is embedded and compiled lazily per locale.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, OnceLock};

use regex::Regex;
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

#[derive(Deserialize)]
struct RawData {
    language_order: Vec<String>,
    language_locale_dict: HashMap<String, Vec<String>>,
    locales: HashMap<String, (usize, String)>,
    datasets: Vec<RawDataset>,
    tz: Vec<(String, String, i64)>,
    nd_zero: Vec<u32>,
    digit_not_decimal: Vec<u32>,
}

#[derive(Deserialize)]
struct RawDataset {
    nws: bool,
    dict: Vec<(String, Option<String>)>,
    rel: Vec<String>,
    rel_tr: Vec<(String, String)>,
    simpl: Vec<(String, String)>,
}

pub(crate) struct DpData {
    pub language_order: Vec<String>,
    language_index: HashMap<String, usize>,
    language_locale_dict: HashMap<String, Vec<String>>,
    locales: HashMap<String, (usize, String)>,
    datasets: Vec<RawDataset>,
    compiled: Vec<OnceLock<Result<CompiledLocale, String>>>,
    pub tz: Vec<(String, String, i64)>,
    nd_zero: Vec<u32>,
    digit_not_decimal: HashSet<u32>,
}

pub(crate) static DATA: LazyLock<DpData> = LazyLock::new(|| {
    let raw: RawData = serde_json::from_str(include_str!("../../data/dateparser.json")).expect("embedded dateparser.json is valid");
    let language_index = raw.language_order.iter().enumerate().map(|(i, l)| (l.clone(), i)).collect();
    let compiled = (0..raw.datasets.len()).map(|_| OnceLock::new()).collect();
    DpData {
        language_order: raw.language_order,
        language_index,
        language_locale_dict: raw.language_locale_dict,
        locales: raw.locales,
        datasets: raw.datasets,
        compiled,
        tz: raw.tz,
        nd_zero: raw.nd_zero,
        digit_not_decimal: raw.digit_not_decimal.into_iter().collect(),
    }
});

// ---------------------------------------------------------------------------
// Unicode helpers (Python str semantics)
// ---------------------------------------------------------------------------

static RE_MN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Mn}+").unwrap());
static RE_ND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").unwrap());

/// dateparser.utils.normalize_unicode: NFKD and drop Mn.
pub(crate) fn normalize_unicode(s: &str) -> String {
    let nfkd: String = s.nfkd().collect();
    RE_MN.replace_all(&nfkd, "").into_owned()
}

fn is_nd(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    let mut b = [0u8; 4];
    RE_ND.is_match(c.encode_utf8(&mut b))
}

/// Decimal value of an Nd char (Python unicodedata.decimal).
pub(crate) fn nd_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    if !is_nd(c) {
        return None;
    }
    let cp = c as u32;
    let zeros = &DATA.nd_zero;
    let idx = zeros.partition_point(|&z| z <= cp);
    if idx == 0 {
        return None;
    }
    let z = zeros[idx - 1];
    (cp - z < 10).then_some(cp - z)
}

/// Python `str.isdigit()`.
pub(crate) fn py_isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| is_nd(c) || DATA.digit_not_decimal.contains(&(c as u32)))
}

/// Python `re.split` with one capturing group, regex crate.
pub(crate) fn split_with_digits(s: &str) -> Vec<String> {
    // NUMERAL_PATTERN = (\d+)  (regex module, Unicode \d)
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
    let mut out = Vec::new();
    let mut last = 0;
    for m in RE.find_iter(s) {
        out.push(s[last..m.start()].to_string());
        out.push(m.as_str().to_string());
        last = m.end();
    }
    out.push(s[last..].to_string());
    out
}

/// Python `re.split` (regex module, Python >= 3.7 semantics incl. empty matches) for a
/// fancy regex with exactly one capturing group.
fn fancy_split(re: &fancy_regex::Regex, s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0usize;
    let mut pos = 0usize;
    let mut prev_empty_at: Option<usize> = None;
    while pos <= s.len() {
        let caps = match re.captures_from_pos(s, pos) {
            Ok(Some(c)) => c,
            _ => break,
        };
        let Some(m) = caps.get(0) else { break };
        if m.start() == m.end() && prev_empty_at == Some(m.start()) {
            // must advance (approximation of sre must_advance, see contract)
            let next = m.start() + s[m.start()..].chars().next().map_or(1, char::len_utf8);
            pos = next;
            continue;
        }
        out.push(s[last..m.start()].to_string());
        out.push(caps.get(1).map_or(String::new(), |g| g.as_str().to_string()));
        last = m.end();
        if m.start() == m.end() {
            prev_empty_at = Some(m.start());
            pos = m.end();
        } else {
            prev_empty_at = None;
            pos = m.end();
        }
    }
    out.push(s[last..].to_string());
    out
}

// ---------------------------------------------------------------------------
// Pattern translation (Python `regex` -> fancy-regex) for the embedded data
// ---------------------------------------------------------------------------

/// Rewrite the lookbehinds dateparser's data uses (variable length in Python) into
/// fixed-length forms fancy-regex accepts. Inputs are sanitized (single spaces), so
/// `\s+` inside a lookbehind is equivalent to `\s` (see contract, Known differences).
pub(crate) fn rewrite_data_pattern(p: &str) -> String {
    let mut s = p.to_string();
    s = s.replace(r"(?<=(?:\A|\W|_))", r"(?:\A|(?<=[\W_]))");
    s = s.replace(r"(?<=\A|\W|_)", r"(?:\A|(?<=[\W_]))");
    s = s.replace(r"(?<=(через|спустя|в течение)\s+)", r"(?:(?<=через\s)|(?<=спустя\s)|(?<=в течение\s))");
    s = s.replace(r"(?<=[^\d]\s+|^)", r"(?:(?<=[^\d]\s)|^)");
    s = s.replace(r"(?<=(za|pred)\s)", r"(?:(?<=za\s)|(?<=pred\s))");
    s = s.replace(r"\Z", r"\z");
    s
}

fn fancy(p: &str) -> Result<fancy_regex::Regex, String> {
    fancy_regex::RegexBuilder::new(&rewrite_data_pattern(p))
        .case_insensitive(true)
        .delegate_size_limit(256 * 1024 * 1024)
        .build()
        .map_err(|e| format!("{e} in {p}"))
}

// ---------------------------------------------------------------------------
// Compiled locale
// ---------------------------------------------------------------------------

pub(crate) const ALWAYS_KEEP_TOKENS: [&str; 6] = ["+", ":", ".", " ", "-", "/"];
const SKIP_TOKENS: [&str; 1] = ["t"];

pub(crate) struct CompiledLocale {
    nws: bool,
    dict: HashMap<String, Option<String>>,
    split_regex: Regex,
    split_rel: fancy_regex::Regex,
    match_rel: fancy_regex::Regex,
    rel_tr: Vec<(fancy_regex::Regex, String)>,
    simpl: Vec<(fancy_regex::Regex, String)>,
}

fn compile_dataset(ds: &RawDataset) -> Result<CompiledLocale, String> {
    let dict: HashMap<String, Option<String>> = ds.dict.iter().cloned().collect();
    // sorted([key for key in self], key=len, reverse=True); iteration = SKIP_TOKENS + dict
    let mut words: Vec<&str> = SKIP_TOKENS.iter().copied().chain(ds.dict.iter().map(|(k, _)| k.as_str())).collect();
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let group = words.iter().map(|w| regex::escape(w)).collect::<Vec<_>>().join("|");
    let split_src = if ds.nws { format!(r"^(.*?)({group})(.*)\z") } else { format!(r"^(.*?(?:\A|[\W_\d]))({group})((?:\z|[\W_\d]).*)\z") };
    let split_regex = regex::RegexBuilder::new(&split_src)
        .case_insensitive(true)
        .size_limit(256 * 1024 * 1024)
        .dfa_size_limit(64 * 1024 * 1024)
        .build()
        .map_err(|e| e.to_string())?;

    let rel_group = ds.rel.join("|");
    let split_rel =
        if ds.nws { fancy(&format!("({rel_group})"))? } else { fancy(&format!(r"(?<=(?:\A|\W|_))({rel_group})(?=(?:\Z|\W|_))"))? };
    let match_rel = fancy(&format!(r"^({rel_group})\z"))?;

    let mut rel_tr = Vec::new();
    for (p, r) in &ds.rel_tr {
        // duplicate (?P<n>...) groups share one number in Python `regex`; Rust forbids
        // duplicates -> plain groups, `\1` = first participating group (see apply_rel_tr)
        let p2 = p.replace("(?P<n>", "(").replace('$', r"\z");
        rel_tr.push((fancy(&p2)?, r.clone()));
    }
    let mut simpl = Vec::new();
    for (p, r) in &ds.simpl {
        simpl.push((fancy(p)?, crate::regex_compat::translate_replacement(r)));
    }
    Ok(CompiledLocale { nws: ds.nws, dict, split_regex, split_rel, match_rel, rel_tr, simpl })
}

/// One locale (shortname, its `info["name"]`, compiled data).
#[derive(Clone)]
pub(crate) struct Locale {
    pub shortname: String,
    pub name: String,
    dataset: usize,
}

impl Locale {
    fn compiled(&self) -> Result<&'static CompiledLocale, String> {
        let data: &'static DpData = &DATA;
        data.compiled[self.dataset].get_or_init(|| compile_dataset(&data.datasets[self.dataset])).as_ref().map_err(Clone::clone)
    }
}

static LOCALE_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-([A-Z0-9]+)$").unwrap());

fn language_of(locale: &str) -> String {
    // LOCALE_SPLIT_PATTERN = -(?=[A-Z0-9]+$)
    match LOCALE_SPLIT.find(locale) {
        Some(m) => locale[..m.start()].to_string(),
        None => locale.to_string(),
    }
}

/// loader._isvalidlocale
pub(crate) fn is_valid_locale(locale: &str) -> bool {
    let language = language_of(locale);
    if !DATA.language_index.contains_key(&language) {
        return false;
    }
    locale == language || DATA.language_locale_dict.get(&language).is_some_and(|l| l.iter().any(|x| x == locale))
}

/// LocaleDataLoader._load_data(locales=..., use_given_order=False) or all languages.
pub(crate) fn load_locales(locales: &[String]) -> Result<Vec<Locale>, String> {
    let mut entries: Vec<(String, String)> = Vec::new(); // (shortname, language)
    if !locales.is_empty() {
        let invalid: Vec<String> = locales.iter().filter(|l| !is_valid_locale(l)).map(|l| format!("'{l}'")).collect();
        if !invalid.is_empty() {
            return Err(format!("Unknown locale(s): {}", invalid.join(", ")));
        }
        for l in locales {
            if !entries.iter().any(|(s, _)| s == l) {
                entries.push((l.clone(), language_of(l)));
            }
        }
        let uniq_locales: HashSet<&String> = locales.iter().collect();
        let uniq_langs: HashSet<&String> = entries.iter().map(|(_, l)| l).collect();
        if uniq_locales.len() > uniq_langs.len() {
            return Err("Locales should not have same language and different region".into());
        }
    } else {
        entries = DATA.language_order.iter().map(|l| (l.clone(), l.clone())).collect();
    }
    entries.sort_by_key(|(_, lang)| DATA.language_index.get(lang).copied().unwrap_or(usize::MAX));
    entries
        .into_iter()
        .map(|(short, _)| {
            let (ds, name) = DATA.locales.get(&short).cloned().ok_or_else(|| format!("no data for locale {short}"))?;
            Ok(Locale { shortname: short, name, dataset: ds })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Locale operations
// ---------------------------------------------------------------------------

fn translate_numerals(s: &str) -> String {
    // tokens that are decimal -> str(int(token)).zfill(len(token)) == per-char ASCII
    s.chars().map(|c| if c.is_ascii() { c } else { nd_value(c).and_then(|v| char::from_digit(v, 10)).unwrap_or(c) }).collect()
}

fn should_capture(token: &str, keep_formatting: bool) -> bool {
    // KEEP_TOKEN_PATTERN = ^.*[^\W_].*$
    static KEEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*[^\W_].*$").unwrap());
    keep_formatting || ALWAYS_KEEP_TOKENS.contains(&token) || KEEP.is_match(token)
}

impl CompiledLocale {
    fn contains(&self, key: &str) -> bool {
        SKIP_TOKENS.contains(&key) || self.dict.contains_key(key)
    }

    fn get(&self, key: &str) -> Option<Option<&str>> {
        if SKIP_TOKENS.contains(&key) {
            return Some(None);
        }
        self.dict.get(key).map(|v| v.as_deref())
    }

    fn split_by_numerals(&self, s: &str, kf: bool) -> Vec<String> {
        split_with_digits(s).into_iter().filter(|t| should_capture(t, kf)).collect()
    }

    fn split_by_known_words(&self, input: &str, kf: bool) -> Vec<String> {
        let mut splitted: Vec<String> = Vec::new();
        let mut string = input.to_string();
        let mut unknown = input.to_string();
        while !unknown.is_empty() {
            let curr_split: Vec<String>;
            match self.split_regex.captures(&string) {
                None => {
                    curr_split = if should_capture(&string, kf) { self.split_by_numerals(&string, kf) } else { vec![] };
                    unknown = String::new();
                }
                Some(c) => {
                    let unparsed = c.get(1).map_or("", |m| m.as_str()).to_string();
                    let known = c.get(2).map_or("", |m| m.as_str()).to_string();
                    let unk = c.get(3).map_or("", |m| m.as_str()).to_string();
                    let mut cs = if should_capture(&known, kf) { vec![known] } else { vec![] };
                    if !unparsed.is_empty() && should_capture(&unparsed, kf) {
                        let mut v = self.split_by_numerals(&unparsed, kf);
                        v.extend(cs);
                        cs = v;
                    }
                    curr_split = cs;
                    if !unk.is_empty() {
                        string = if string != unk { unk.clone() } else { String::new() };
                    }
                    unknown = unk;
                }
            }
            for token in curr_split {
                if splitted.last().is_some_and(|l| py_isdigit(l)) && ["st", "nd", "rd", "th"].contains(&token.as_str()) {
                    continue;
                }
                splitted.push(token);
            }
        }
        splitted
    }

    /// Dictionary.split
    fn split(&self, s: &str, kf: bool) -> Vec<String> {
        if s.is_empty() {
            return vec![];
        }
        let mut out = Vec::new();
        for token in fancy_split(&self.split_rel, s) {
            if self.match_rel.is_match(&token).unwrap_or(false) {
                if !token.is_empty() {
                    out.push(token);
                }
                continue;
            }
            for t in self.split_by_known_words(&token, kf) {
                if !t.is_empty() {
                    out.push(t);
                }
            }
        }
        out
    }

    fn are_tokens_valid(&self, tokens: &[String]) -> bool {
        let only_keep = tokens.iter().all(|t| ALWAYS_KEEP_TOKENS.contains(&t.as_str()));
        if only_keep {
            return false;
        }
        tokens.iter().all(|t| py_isdigit(t) || self.match_rel.is_match(t).unwrap_or(false) || self.contains(t))
    }

    fn apply_simplifications(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (re, rep) in &self.simpl {
            out = match re.try_replacen(&out, 0, rep.as_str()) {
                Ok(r) => r.to_lowercase(),
                Err(_) => out.to_lowercase(),
            };
        }
        out
    }

    fn join(tokens: &[String], sep: &str) -> String {
        let Some(first) = tokens.first() else { return String::new() };
        let mut joined = first.clone();
        for i in 1..tokens.len() {
            let (l, r) = (&tokens[i - 1], &tokens[i]);
            if !ALWAYS_KEEP_TOKENS.contains(&l.as_str()) && !ALWAYS_KEEP_TOKENS.contains(&r.as_str()) {
                joined.push_str(sep);
            }
            joined.push_str(r);
        }
        joined
    }
}

/// `\1` of a relative-translation pattern = first participating capture group.
fn apply_rel_tr(re: &fancy_regex::Regex, repl: &str, word: &str) -> String {
    let caps = match re.captures(word) {
        Ok(Some(c)) => c,
        _ => return word.to_string(),
    };
    let g1 = (1..caps.len()).find_map(|i| caps.get(i)).map_or("", |m| m.as_str());
    let m0 = caps.get(0).map_or(0..0, |m| m.range());
    let replaced = repl.replace(r"\1", g1);
    // pattern is anchored ^...$, so the whole word is replaced
    format!("{}{}{}", &word[..m0.start], replaced, &word[m0.end..])
}

impl Locale {
    fn simplify(&self, c: &CompiledLocale, s: &str) -> String {
        let lowered = s.to_lowercase();
        if self.name == "ru" {
            let s2 = c.apply_simplifications(&lowered);
            static PAIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d+)\s+(\d+)\b").unwrap());
            PAIR.replace_all(&s2, |caps: &regex::Captures| {
                let a: i64 = caps[1].parse().unwrap_or(-1);
                let b: i64 = caps[2].parse().unwrap_or(-1);
                let r = a + b;
                if (1..=31).contains(&r) && (a == 20 || a == 30) && (1..=9).contains(&b) { r.to_string() } else { caps[0].to_string() }
            })
            .into_owned()
        } else {
            c.apply_simplifications(&lowered)
        }
    }

    /// Locale.is_applicable(date_string, strip_timezone=False)
    pub(crate) fn is_applicable(&self, date_string: &str) -> Result<bool, String> {
        let c = self.compiled()?;
        let s = translate_numerals(date_string);
        let s = normalize_unicode(&s);
        let s = self.simplify(c, &s);
        let tokens = c.split(&s, false);
        Ok(c.are_tokens_valid(&tokens))
    }

    /// Locale.translate(date_string, keep_formatting=False)
    pub(crate) fn translate(&self, date_string: &str) -> Result<String, String> {
        let c = self.compiled()?;
        let s = translate_numerals(date_string);
        let s = normalize_unicode(&s);
        let s = self.simplify(c, &s);
        let mut tokens = c.split(&s, false);

        for tok in tokens.iter_mut() {
            let word = tok.to_lowercase();
            let mut done = false;
            for (re, repl) in &c.rel_tr {
                if re.is_match(&word).unwrap_or(false) {
                    *tok = apply_rel_tr(re, repl, &word);
                    done = true;
                    break;
                }
            }
            if !done {
                if let Some(v) = c.get(&word) {
                    *tok = v.unwrap_or("").to_string();
                }
            }
        }
        if tokens.iter().any(|t| t == "in") {
            const FRESH: [&str; 7] = ["day", "week", "month", "year", "hour", "minute", "second"];
            if !tokens.iter().any(|t| FRESH.contains(&t.as_str())) {
                if let Some(i) = tokens.iter().position(|t| t == "in") {
                    tokens[i] = String::new();
                }
            }
        }

        let mut filtered: Vec<String> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if token.is_empty() {
                let prev_spaces = filtered.iter().rev().take_while(|t| *t == " ").count();
                let next_spaces = tokens[i + 1..].iter().take_while(|t| *t == " ").count();
                if prev_spaces > 0 && next_spaces > 0 {
                    for _ in 0..prev_spaces {
                        filtered.pop();
                    }
                    for _ in 0..prev_spaces.max(next_spaces) {
                        filtered.push(" ".into());
                    }
                    i += next_spaces + 1;
                    continue;
                }
                i += 1;
                continue;
            }
            filtered.push(token.clone());
            i += 1;
        }
        let _ = c.nws;
        Ok(CompiledLocale::join(&filtered, " "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_locale_data_compiles() {
        // every dataset must compile (otherwise that locale could never be used)
        let mut failures = Vec::new();
        for (i, ds) in DATA.datasets.iter().enumerate() {
            if let Err(e) = compile_dataset(ds) {
                failures.push(format!("{i}: {e}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn translate_basic() {
        let en = load_locales(&["en".into()]).unwrap().remove(0);
        assert_eq!(en.translate("15 jan 2024").unwrap(), "15 january 2024");
        assert!(en.is_applicable("15 jan 2024").unwrap());
        assert!(!en.is_applicable("total 2024").unwrap());
        let de = load_locales(&["de".into()]).unwrap().remove(0);
        assert_eq!(de.translate("15. März 2024").unwrap(), "15. march 2024");
        assert_eq!(de.translate("gestern 2024").unwrap(), "1 day ago 2024");
    }

    #[test]
    fn locale_loading() {
        assert!(load_locales(&["xx".into()]).is_err());
        assert!(load_locales(&["en".into(), "en-001".into()]).is_err());
        let l = load_locales(&["de".into(), "en".into()]).unwrap();
        assert_eq!(l[0].shortname, "en");
        assert_eq!(load_locales(&[]).unwrap().len(), DATA.language_order.len());
        assert!(is_valid_locale("az-Cyrl"));
        assert!(!is_valid_locale("zh-Tra"));
    }

    #[test]
    fn unicode_helpers() {
        assert_eq!(nd_value('٣'), Some(3));
        assert!(py_isdigit("²"));
        assert!(!py_isdigit(""));
        assert_eq!(normalize_unicode("März"), "Marz");
    }
}
