//! Port of `langdetect` 1.0.9 (Python, Apache-2.0, © 2014-2015 Michal Danilak; originally
//! language-detection by Nakatani Shuyo, Cybozu Labs). Profiles and `messages.properties`
//! are kept unchanged under `data/langdetect/` (with LICENSE/NOTICE).
//!
//! upstream: langdetect/detector.py (`Detector.append`, `cleaning_text`, `_detect_block`,
//! `_extract_ngrams`, `_update_lang_prob`, `_normalize_prob`, `_sort_probability`),
//! langdetect/detector_factory.py (`load_profile`, `add_profile`, `detect`),
//! langdetect/utils/ngram.py (`NGram.add_char`, `normalize`, `normalize_vi`, CJK map),
//! langdetect/utils/unicode_block.py (only the blocks that `normalize` distinguishes).
//!
//! Deviation (contract, Known differences): upstream draws a random seed per call
//! (`random.Random()` without `DetectorFactory.seed`) – for borderline texts the result can
//! change between calls. Here the random generator is seeded with a fixed value (deterministic).
//! Profile order = sorted file names (upstream: `os.listdir`, sorted on Windows).

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

const ALPHA_DEFAULT: f64 = 0.5;
const ALPHA_WIDTH: f64 = 0.05;
const ITERATION_LIMIT: usize = 1000;
const PROB_THRESHOLD: f64 = 0.1;
const CONV_THRESHOLD: f64 = 0.99999;
const BASE_FREQ: f64 = 10000.0;
const N_TRIAL: usize = 7;
const MAX_TEXT_LENGTH: usize = 10000;
const N_GRAM: usize = 3;

macro_rules! profiles {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../data/langdetect/profiles/", $name)))),*]
    };
}

/// All 55 profiles (sorted like `os.listdir` on Windows).
const PROFILES: &[(&str, &str)] = profiles!(
    "af", "ar", "bg", "bn", "ca", "cs", "cy", "da", "de", "el", "en", "es", "et", "fa", "fi", "fr", "gu", "he",
    "hi", "hr", "hu", "id", "it", "ja", "kn", "ko", "lt", "lv", "mk", "ml", "mr", "ne", "nl", "no", "pa", "pl",
    "pt", "ro", "ru", "sk", "sl", "so", "sq", "sv", "sw", "ta", "te", "th", "tl", "tr", "uk", "ur", "vi",
    "zh-cn", "zh-tw",
);

const MESSAGES: &str = include_str!("../data/langdetect/messages.properties");

struct Factory {
    langlist: Vec<String>,
    /// n-gram → (language index, probability) – sparse instead of 55 floats per n-gram.
    word_lang_prob: NgramTable,
    latin1_excluded: Vec<char>,
    cjk_map: HashMap<char, char>,
    vi_re: Regex,
    to_normalize_vi: Vec<char>,
    dmark_class: Vec<char>,
    normalized_vi: Vec<Vec<char>>,
    url_re: Regex,
    mail_re: Regex,
}

/// `value.encode().decode('unicode_escape')` for the `\uXXXX` escapes of the file.
fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut it = v.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&'u') {
            it.next();
            let hex: String = it.by_ref().take(4).collect();
            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(ch);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Compact table n-gram (1–3 characters) → entries `(language index, probability)`.
///
/// Memory (measured 2026-11-18): before, `HashMap<String, Vec<(u8, f64)>>` with one
/// String and one Vec allocation each for 87 598 n-grams; now a `u64` key (three characters
/// packed) pointing to a range in **one** shared vector. Same values, same order
/// (language index ascending as on insertion), faster lookup (no string per n-gram).
struct NgramTable {
    index: HashMap<u64, (u32, u32)>,
    entries: Vec<(u8, f64)>,
}

impl NgramTable {
    /// Packs up to three characters (21 bits each, offset by 1 so the length is unambiguous).
    fn key(chars: impl IntoIterator<Item = char>) -> Option<u64> {
        let mut k = 0u64;
        let mut n = 0;
        for c in chars {
            if n == N_GRAM {
                return None;
            }
            k = (k << 21) | (c as u64 + 1);
            n += 1;
        }
        (n > 0).then_some(k)
    }

    fn get(&self, key: u64) -> Option<&[(u8, f64)]> {
        let &(start, len) = self.index.get(&key)?;
        self.entries.get(start as usize..(start + len) as usize)
    }

    #[cfg(test)]
    fn contains(&self, word: &str) -> bool {
        Self::key(word.chars()).is_some_and(|k| self.index.contains_key(&k))
    }

    /// Build from `(n-gram, language index, probability)`. Sorted by n-gram and
    /// language index = insertion order per n-gram (profiles in index order). Everything in a few
    /// large blocks (no allocation per n-gram), so that no fragmented
    /// heap is left behind after the build.
    fn build(mut triples: Vec<(u64, u8, f64)>) -> Self {
        triples.sort_unstable_by_key(|t| (t.0, t.1));
        let unique = triples.windows(2).filter(|w| w[0].0 != w[1].0).count() + usize::from(!triples.is_empty());
        let mut index = HashMap::with_capacity(unique);
        let mut entries = Vec::with_capacity(triples.len());
        let mut start = 0usize;
        for (i, &(key, lang, prob)) in triples.iter().enumerate() {
            entries.push((lang, prob));
            if triples.get(i + 1).is_none_or(|n| n.0 != key) {
                index.insert(key, (start as u32, (i + 1 - start) as u32));
                start = i + 1;
            }
        }
        Self { index, entries }
    }
}

fn regex_class(chars: &[char]) -> String {
    chars.iter().map(|c| regex::escape(&c.to_string())).collect()
}

#[derive(serde::Deserialize)]
struct Profile<'a> {
    name: String,
    /// Keys without escape sequences are borrowed from the embedded text (no copy).
    #[serde(borrow)]
    freq: HashMap<std::borrow::Cow<'a, str>, f64>,
    n_words: Vec<f64>,
}

fn factory() -> Option<&'static Factory> {
    static F: OnceLock<Option<Factory>> = OnceLock::new();
    F.get_or_init(build_factory).as_ref()
}

fn build_factory() -> Option<Factory> {
    let mut messages: HashMap<String, String> = HashMap::new();
    for line in MESSAGES.lines() {
        let line = line.trim();
        let (k, v) = line.split_once('=').unwrap_or((line, ""));
        messages.insert(k.to_string(), unescape(v));
    }
    let get = |k: &str| messages.get(k).cloned().unwrap_or_else(|| format!("!{k}!"));
    let mut cjk_map = HashMap::new();
    let mut kanji_keys: Vec<&String> = messages.keys().filter(|k| k.starts_with("NGram.KANJI_")).collect();
    kanji_keys.sort();
    for k in kanji_keys {
        let list: Vec<char> = messages[k].chars().collect();
        if let Some(rep) = list.first().copied() {
            for ch in list {
                cjk_map.insert(ch, rep);
            }
        }
    }
    let to_normalize_vi: Vec<char> = get("TO_NORMALIZE_VI_CHARS").chars().collect();
    let dmark_class: Vec<char> = get("DMARK_CLASS").chars().collect();
    let normalized_vi = ["0300", "0301", "0303", "0309", "0323"]
        .iter()
        .map(|s| get(&format!("NORMALIZED_VI_CHARS_{s}")).chars().collect())
        .collect();
    let vi_re = Regex::new(&format!("([{}])([{}])", regex_class(&to_normalize_vi), regex_class(&dmark_class))).ok()?;
    let langsize = PROFILES.len();
    let mut langlist = Vec::with_capacity(langsize);
    // Longer words (upstream: entry with all zeros) are never looked up –
    // `extract_ngrams` builds at most 3-grams – and are therefore omitted.
    let mut triples: Vec<(u64, u8, f64)> = Vec::with_capacity(190_000);
    for (index, (_, json)) in PROFILES.iter().enumerate() {
        let p: Profile<'_> = serde_json::from_str(json).ok()?;
        langlist.push(p.name);
        for (word, f) in p.freq {
            let len = word.chars().count();
            if let Some(key) = NgramTable::key(word.chars())
                && (1..=3).contains(&len)
            {
                let n = p.n_words.get(len - 1).copied().unwrap_or(0.0);
                triples.push((key, index as u8, 1.0 * f / n));
            }
        }
    }
    let word_lang_prob = NgramTable::build(triples);
    Some(Factory {
        langlist,
        word_lang_prob,
        latin1_excluded: get("NGram.LATIN1_EXCLUDE").chars().collect(),
        cjk_map,
        vi_re,
        to_normalize_vi,
        dmark_class,
        normalized_vi,
        url_re: Regex::new(r"https?://[-_.?&~;+=/#0-9A-Za-z]{1,2076}").ok()?,
        mail_re: Regex::new(r"[-_.0-9A-Za-z]{1,64}@[-_0-9A-Za-z]{1,255}[-_.0-9A-Za-z]{1,255}").ok()?,
    })
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Block {
    BasicLatin,
    Latin1Supplement,
    LatinExtendedB,
    Arabic,
    LatinExtendedAdditional,
    GeneralPunctuation,
    Hiragana,
    Katakana,
    Bopomofo,
    CjkUnified,
    HangulSyllables,
    Other,
}

fn unicode_block(ch: char) -> Block {
    match ch as u32 {
        0x0000..=0x007F => Block::BasicLatin,
        0x0080..=0x00FF => Block::Latin1Supplement,
        0x0180..=0x024F => Block::LatinExtendedB,
        0x0600..=0x06FF => Block::Arabic,
        0x1E00..=0x1EFF => Block::LatinExtendedAdditional,
        0x2000..=0x206F => Block::GeneralPunctuation,
        0x3040..=0x309F => Block::Hiragana,
        0x30A0..=0x30FF => Block::Katakana,
        0x3100..=0x312F | 0x31A0..=0x31BF => Block::Bopomofo,
        0x4E00..=0x9FFF => Block::CjkUnified,
        0xAC00..=0xD7AF => Block::HangulSyllables,
        _ => Block::Other,
    }
}

impl Factory {
    /// `NGram.normalize(ch)`.
    fn normalize(&self, ch: char) -> char {
        match unicode_block(ch) {
            Block::BasicLatin => {
                if ch < 'A' || ('Z' < ch && ch < 'a') || 'z' < ch {
                    ' '
                } else {
                    ch
                }
            }
            Block::Latin1Supplement => {
                if self.latin1_excluded.contains(&ch) {
                    ' '
                } else {
                    ch
                }
            }
            Block::LatinExtendedB => match ch {
                '\u{0219}' => '\u{015f}',
                '\u{021b}' => '\u{0163}',
                c => c,
            },
            Block::GeneralPunctuation => ' ',
            Block::Arabic => {
                if ch == '\u{06cc}' {
                    '\u{064a}'
                } else {
                    ch
                }
            }
            Block::LatinExtendedAdditional => {
                if ch >= '\u{1ea0}' {
                    '\u{1ec3}'
                } else {
                    ch
                }
            }
            Block::Hiragana => '\u{3042}',
            Block::Katakana => '\u{30a2}',
            Block::Bopomofo => '\u{3105}',
            Block::CjkUnified => self.cjk_map.get(&ch).copied().unwrap_or(ch),
            Block::HangulSyllables => '\u{ac00}',
            Block::Other => ch,
        }
    }

    /// `NGram.normalize_vi(text)`.
    fn normalize_vi(&self, text: &str) -> String {
        self.vi_re
            .replace_all(text, |c: &regex::Captures<'_>| {
                let a = c.get(1).and_then(|m| m.as_str().chars().next());
                let d = c.get(2).and_then(|m| m.as_str().chars().next());
                let ai = a.and_then(|a| self.to_normalize_vi.iter().position(|x| *x == a));
                let di = d.and_then(|d| self.dmark_class.iter().position(|x| *x == d));
                match (ai, di) {
                    (Some(ai), Some(di)) => self
                        .normalized_vi
                        .get(di)
                        .and_then(|s| s.get(ai))
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                    _ => c[0].to_string(),
                }
            })
            .into_owned()
    }

    /// `Detector.append(text)`.
    fn append(&self, text: &str) -> Vec<char> {
        let t = self.url_re.replace_all(text, " ");
        let t = self.mail_re.replace_all(&t, " ");
        let t = self.normalize_vi(&t);
        let mut out = Vec::new();
        let mut pre = '\0';
        for ch in t.chars().take(MAX_TEXT_LENGTH) {
            if ch != ' ' || pre != ' ' {
                out.push(ch);
            }
            pre = ch;
        }
        out
    }

    /// `Detector.cleaning_text()`. Upstream compares the block code (int) with the string
    /// `'Latin Extended Additional'` – always unequal, so every character from U+0300 on counts.
    fn cleaning_text(text: Vec<char>) -> Vec<char> {
        let mut latin = 0usize;
        let mut non_latin = 0usize;
        for &ch in &text {
            if ('A'..='z').contains(&ch) {
                latin += 1;
            } else if ch >= '\u{0300}' {
                non_latin += 1;
            }
        }
        if latin * 2 < non_latin {
            text.into_iter().filter(|ch| !('A'..='z').contains(ch)).collect()
        } else {
            text
        }
    }

    /// `Detector._extract_ngrams()`.
    fn extract_ngrams(&self, text: &[char]) -> Vec<&[(u8, f64)]> {
        let mut result = Vec::new();
        let mut grams: Vec<char> = vec![' '];
        let mut capitalword = false;
        for &raw in text {
            // NGram.add_char
            let ch = self.normalize(raw);
            let last = grams.last().copied().unwrap_or(' ');
            let mut skip_add = false;
            if last == ' ' {
                grams = vec![' '];
                capitalword = false;
                if ch == ' ' {
                    skip_add = true;
                }
            } else if grams.len() >= N_GRAM {
                grams.remove(0);
            }
            if !skip_add {
                grams.push(ch);
                if ch.is_uppercase() {
                    if last.is_uppercase() {
                        capitalword = true;
                    }
                } else {
                    capitalword = false;
                }
            }
            if capitalword {
                continue;
            }
            for n in 1..=N_GRAM {
                if grams.len() < n {
                    break;
                }
                let w = &grams[grams.len() - n..];
                if !w.is_empty()
                    && w != [' ']
                    && let Some(p) = NgramTable::key(w.iter().copied()).and_then(|k| self.word_lang_prob.get(k))
                {
                    result.push(p);
                }
            }
        }
        result
    }
}

/// Fixed-seed random generator (splitmix64) for `choice` and `gauss`.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// Box-Muller (Python `random.gauss` uses the same method).
    fn gauss(&mut self) -> f64 {
        let x2pi = self.random() * std::f64::consts::TAU;
        let g2rad = (-2.0 * (1.0 - self.random()).ln()).sqrt();
        x2pi.cos() * g2rad
    }
}

fn normalize_prob(prob: &mut [f64]) -> f64 {
    let sum: f64 = prob.iter().sum();
    let mut maxp = 0.0;
    for p in prob.iter_mut() {
        *p /= sum;
        if maxp < *p {
            maxp = *p;
        }
    }
    maxp
}

/// `langdetect.detect(text)`: language code with the highest probability, `None` on
/// `LangDetectException` ("No features in text.").
pub fn detect(text: &str) -> Option<String> {
    let f = factory()?;
    let text = Factory::cleaning_text(f.append(text));
    let ngrams = f.extract_ngrams(&text);
    if ngrams.is_empty() {
        return None;
    }
    let n = f.langlist.len();
    let mut langprob = vec![0.0f64; n];
    let mut rng = Rng(0x5EED_1A9D_E7EC_7000);
    let mut factors = vec![0.0f64; n];
    for _ in 0..N_TRIAL {
        let mut prob = vec![1.0 / n as f64; n];
        let alpha = ALPHA_DEFAULT + rng.gauss() * ALPHA_WIDTH;
        let weight = alpha / BASE_FREQ;
        let mut i = 0usize;
        loop {
            let word = ngrams[rng.below(ngrams.len())];
            factors.iter_mut().for_each(|x| *x = weight);
            for &(li, p) in word {
                if let Some(x) = factors.get_mut(li as usize) {
                    *x = weight + p;
                }
            }
            for (p, fct) in prob.iter_mut().zip(&factors) {
                *p *= fct;
            }
            if i.is_multiple_of(5) && (normalize_prob(&mut prob) > CONV_THRESHOLD || i >= ITERATION_LIMIT) {
                break;
            }
            i += 1;
        }
        for (lp, p) in langprob.iter_mut().zip(&prob) {
            *lp += p / N_TRIAL as f64;
        }
    }
    // _sort_probability: > PROB_THRESHOLD, descending (stable like Python's sort(reverse=True))
    let mut best: Option<(usize, f64)> = None;
    for (i, &p) in langprob.iter().enumerate() {
        if p > PROB_THRESHOLD && best.is_none_or(|(_, b)| p > b) {
            best = Some((i, p));
        }
    }
    Some(match best {
        Some((i, _)) => f.langlist[i].clone(),
        None => "unknown".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_load() {
        let f = factory().expect("factory");
        assert_eq!(f.langlist.len(), 55);
        assert_eq!(f.langlist[8], "de");
        assert!(f.word_lang_prob.contains("der"));
        assert!(!f.word_lang_prob.contains("derx"));
    }

    #[test]
    fn detects_common_languages() {
        assert_eq!(detect("This is a simple English sentence about the weather and the invoice.").as_deref(), Some("en"));
        assert_eq!(detect("Dies ist eine Rechnung für die Lieferung der bestellten Waren im November.").as_deref(), Some("de"));
        assert_eq!(detect("Ceci est une facture pour la livraison des marchandises commandées.").as_deref(), Some("fr"));
        assert_eq!(detect("").as_deref(), None);
        assert_eq!(detect("1234 5678 !!!").as_deref(), None);
    }

    #[test]
    fn deterministic() {
        let t = "Das ist ein Test mit ein paar Wörtern.";
        assert_eq!(detect(t), detect(t));
    }

    #[test]
    fn unescape_works() {
        assert_eq!(unescape(r" «x"), "\u{a0}\u{ab}x");
    }
}
