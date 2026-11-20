// upstream: src/documents/classifier.py@v3.4.2 (RE_WORD, _text_analyzer, DocumentClassifier.preprocess_content)
// upstream: src/paperless/settings/__init__.py@v3.4.2 (CLASSIFIER_LANGUAGES, _get_classifier_language_setting)
//
// Two paths, exactly like upstream:
// - language None: NFC, Python `re` `\b[\w]+\b` matches, each lowercased with `str.lower()`.
// - language set: NFC, tantivy-py analyzer = RegexTokenizer(`\w+`, Rust regex semantics) ->
//   LowerCaser (per char, no final-sigma rule) -> stop words -> Snowball stemmer (rust-stemmers).
//   English uses the Snowball list from upstream (split on Python \w), other languages tantivy's lists.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::stopwords;

/// Classifier stemming language (values of upstream `CLASSIFIER_LANGUAGES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Danish,
    Dutch,
    English,
    Finnish,
    French,
    German,
    Italian,
    Norwegian,
    Portuguese,
    Russian,
    Spanish,
    Swedish,
}

impl Language {
    pub const ALL: [Language; 12] = [
        Language::Danish,
        Language::Dutch,
        Language::English,
        Language::Finnish,
        Language::French,
        Language::German,
        Language::Italian,
        Language::Norwegian,
        Language::Portuguese,
        Language::Russian,
        Language::Spanish,
        Language::Swedish,
    ];

    /// Name as used in upstream settings (`settings.CLASSIFIER_LANGUAGE`).
    pub fn name(self) -> &'static str {
        match self {
            Language::Danish => "danish",
            Language::Dutch => "dutch",
            Language::English => "english",
            Language::Finnish => "finnish",
            Language::French => "french",
            Language::German => "german",
            Language::Italian => "italian",
            Language::Norwegian => "norwegian",
            Language::Portuguese => "portuguese",
            Language::Russian => "russian",
            Language::Spanish => "spanish",
            Language::Swedish => "swedish",
        }
    }

    pub fn from_name(name: &str) -> Option<Language> {
        Language::ALL.into_iter().find(|l| l.name() == name)
    }

    /// upstream `_get_classifier_language_setting(PAPERLESS_OCR_LANGUAGE)`: primary Tesseract
    /// language (before the first `+`) mapped through `CLASSIFIER_LANGUAGES`, else `None`.
    pub fn from_ocr_language(ocr_lang: &str) -> Option<Language> {
        let primary = ocr_lang.split('+').next().unwrap_or("");
        Some(match primary {
            "dan" => Language::Danish,
            "nld" => Language::Dutch,
            "eng" => Language::English,
            "fin" => Language::Finnish,
            "fra" => Language::French,
            "deu" => Language::German,
            "ita" => Language::Italian,
            "nor" => Language::Norwegian,
            "por" => Language::Portuguese,
            "rus" => Language::Russian,
            "spa" => Language::Spanish,
            "swe" => Language::Swedish,
            _ => return None,
        })
    }

    fn algorithm(self) -> Algorithm {
        match self {
            Language::Danish => Algorithm::Danish,
            Language::Dutch => Algorithm::Dutch,
            Language::English => Algorithm::English,
            Language::Finnish => Algorithm::Finnish,
            Language::French => Algorithm::French,
            Language::German => Algorithm::German,
            Language::Italian => Algorithm::Italian,
            Language::Norwegian => Algorithm::Norwegian,
            Language::Portuguese => Algorithm::Portuguese,
            Language::Russian => Algorithm::Russian,
            Language::Spanish => Algorithm::Spanish,
            Language::Swedish => Algorithm::Swedish,
        }
    }

    fn stop_words(self) -> &'static [&'static str] {
        match self {
            Language::Danish => stopwords::DANISH,
            Language::Dutch => stopwords::DUTCH,
            Language::English => stopwords::ENGLISH_SNOWBALL,
            Language::Finnish => stopwords::FINNISH,
            Language::French => stopwords::FRENCH,
            Language::German => stopwords::GERMAN,
            Language::Italian => stopwords::ITALIAN,
            Language::Norwegian => stopwords::NORWEGIAN,
            Language::Portuguese => stopwords::PORTUGUESE,
            Language::Russian => stopwords::RUSSIAN,
            Language::Spanish => stopwords::SPANISH,
            Language::Swedish => stopwords::SWEDISH,
        }
    }
}

/// Python `re` (str pattern) `\w`: `str.isalnum()` or `_`, i.e. Unicode categories L* and N*
/// plus underscore. Runs of these chars are what `\b[\w]+\b` and `\b\w\w+\b` match.
fn py_word_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\p{L}\p{N}_]+").unwrap_or_else(|_| unreachable!()))
}

/// Maximal runs of Python `\w` characters with at least `min_chars` characters
/// (`\b[\w]+\b` → 1, sklearn default token pattern `(?u)\b\w\w+\b` → 2). A greedy
/// character-class match is exactly a maximal run, so `\b` needs no extra check.
pub(crate) fn py_word_runs<'a>(text: &'a str, min_chars: usize, mut f: impl FnMut(&'a str)) {
    for m in py_word_regex().find_iter(text) {
        let s = m.as_str();
        if min_chars <= 1 || s.chars().nth(min_chars - 1).is_some() {
            f(s);
        }
    }
}

struct Analyzer {
    stop: HashSet<String>,
    stemmer: Stemmer,
}

fn analyzer(lang: Language) -> &'static Analyzer {
    static CACHE: OnceLock<Vec<Analyzer>> = OnceLock::new();
    let all = CACHE.get_or_init(|| {
        Language::ALL
            .iter()
            .map(|&l| {
                let stop = if l == Language::English {
                    // upstream: sorted({t for word in ENGLISH_STOP_WORDS for t in RE_WORD.findall(word)})
                    let mut s = HashSet::new();
                    for w in l.stop_words() {
                        py_word_runs(w, 1, |t| {
                            s.insert(t.to_owned());
                        });
                    }
                    s
                } else {
                    l.stop_words().iter().map(|w| (*w).to_owned()).collect()
                };
                Analyzer {
                    stop,
                    stemmer: Stemmer::create(l.algorithm()),
                }
            })
            .collect()
    });
    let idx = Language::ALL.iter().position(|&l| l == lang).unwrap_or(0);
    &all[idx]
}

fn rust_word_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\w+").unwrap_or_else(|_| unreachable!()))
}

/// tantivy `LowerCaser`: ASCII fast path, otherwise `char::to_lowercase` per char.
fn tantivy_lowercase(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        s.chars().flat_map(char::to_lowercase).collect()
    }
}

/// tantivy-py `TextAnalyzer.analyze(text)` for the classifier analyzer of `lang`.
pub fn analyze(lang: Language, text: &str) -> Vec<String> {
    let a = analyzer(lang);
    let mut out = Vec::new();
    for m in rust_word_regex().find_iter(text) {
        let tok = tantivy_lowercase(m.as_str());
        if a.stop.contains(&tok) {
            continue;
        }
        out.push(a.stemmer.stem(&tok).into_owned());
    }
    out
}

/// upstream `DocumentClassifier.preprocess_content(content)`.
pub fn preprocess_content(content: &str, language: Option<Language>) -> String {
    let content: String = content.nfc().collect();
    match language {
        None => {
            let mut parts: Vec<String> = Vec::new();
            py_word_runs(&content, 1, |w| parts.push(w.to_lowercase()));
            parts.join(" ")
        }
        Some(lang) => analyze(lang, &content).join(" "),
    }
}

/// upstream `Document.suggestion_content` (src/documents/models.py@v3.4.2): content longer than
/// 1 200 000 characters is cropped to the first 800 000 and last 200 000 characters, joined by a
/// space. Lengths are in Unicode code points like Python `len(str)`.
pub fn suggestion_content(content: &str) -> std::borrow::Cow<'_, str> {
    let n = content.chars().count();
    if n <= 1_200_000 {
        return std::borrow::Cow::Borrowed(content);
    }
    let head_end = content
        .char_indices()
        .nth(800_000)
        .map_or(content.len(), |(i, _)| i);
    let tail_start = content
        .char_indices()
        .nth(n - 200_000)
        .map_or(content.len(), |(i, _)| i);
    let mut s = String::with_capacity(head_end + 1 + content.len() - tail_start);
    s.push_str(&content[..head_end]);
    s.push(' ');
    s.push_str(&content[tail_start..]);
    std::borrow::Cow::Owned(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_removes_snowball_stop_words() {
        // upstream test_classifier.py::TestTextAnalyzer::test_english_removes_snowball_stop_words
        let t = analyze(
            Language::English,
            "They were about to pay the invoices, don't worry",
        );
        assert_eq!(t, vec!["pay", "invoic", "worri"]);
    }

    #[test]
    fn keeps_underscores_within_tokens() {
        assert_eq!(analyze(Language::English, "tax_id"), vec!["tax_id"]);
    }

    #[test]
    fn every_language_produces_tokens() {
        for l in Language::ALL {
            assert!(!analyze(l, "Paperless invoice 2026").is_empty(), "{l:?}");
        }
    }

    #[test]
    fn empty_content() {
        assert_eq!(preprocess_content("", None), "");
        assert_eq!(preprocess_content("", Some(Language::English)), "");
    }

    #[test]
    fn ocr_language_mapping() {
        assert_eq!(
            Language::from_ocr_language("deu+eng"),
            Some(Language::German)
        );
        assert_eq!(Language::from_ocr_language("eng"), Some(Language::English));
        assert_eq!(Language::from_ocr_language("ara"), None);
        assert_eq!(Language::from_ocr_language(""), None);
    }

    #[test]
    fn suggestion_content_crop() {
        let s = "a".repeat(1_200_000);
        assert!(matches!(
            suggestion_content(&s),
            std::borrow::Cow::Borrowed(_)
        ));
        let s = format!("{}{}", "x".repeat(900_000), "y".repeat(400_000));
        let c = suggestion_content(&s);
        assert_eq!(c.chars().count(), 1_000_001);
        assert!(c.starts_with('x') && c.ends_with('y'));
    }
}
