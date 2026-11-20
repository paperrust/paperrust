// upstream: src/documents/search/_tokenizer.py@v3.4.2
//
// Same analyzer names and filter pipelines as upstream, built from the same
// tantivy (0.26.0) primitives tantivy-py wraps:
//   tantivy.Tokenizer.simple()        -> SimpleTokenizer
//   tantivy.Tokenizer.raw()           -> RawTokenizer
//   tantivy.Tokenizer.ngram(2,2,False)-> NgramTokenizer::new(2, 2, false)
//   tantivy.Tokenizer.regex(r"\S+")   -> RegexTokenizer::new(r"\S+")
//   Filter.remove_long(n)             -> RemoveLongFilter::limit(n)
//   Filter.lowercase()                -> LowerCaser
//   Filter.ascii_fold()               -> AsciiFoldingFilter
//   Filter.stemmer(lang)              -> Stemmer::new(Language::..)

use std::sync::OnceLock;

use tantivy::Index;
use tantivy::tokenizer::{
    AsciiFoldingFilter, Language, LowerCaser, NgramTokenizer, RawTokenizer, RegexTokenizer,
    RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer, TokenStream,
};

/// Document.title is max_length=128, so 129 is the limit for remove_long.
pub const TOKEN_REMOVE_LONG_LIMIT: usize = 129;

/// Tokenizer names registered on the index (schema contract).
pub const PAPERLESS_TEXT: &str = "paperless_text";
pub const SIMPLE_ANALYZER: &str = "simple_analyzer";
pub const BIGRAM_ANALYZER: &str = "bigram_analyzer";
pub const SIMPLE_SEARCH_ANALYZER: &str = "simple_search_analyzer";

/// `_LANGUAGE_MAP`: ISO 639-1 codes and aliases -> Snowball language.
pub fn language_for(code: &str) -> Option<Language> {
    let l = match code.to_lowercase().as_str() {
        "ar" | "arabic" => Language::Arabic,
        "da" | "danish" => Language::Danish,
        "nl" | "dutch" => Language::Dutch,
        "en" | "english" => Language::English,
        "fi" | "finnish" => Language::Finnish,
        "fr" | "french" => Language::French,
        "de" | "german" => Language::German,
        "el" | "greek" => Language::Greek,
        "hu" | "hungarian" => Language::Hungarian,
        "it" | "italian" => Language::Italian,
        "no" | "norwegian" => Language::Norwegian,
        "pt" | "portuguese" => Language::Portuguese,
        "ro" | "romanian" => Language::Romanian,
        "ru" | "russian" => Language::Russian,
        "es" | "spanish" => Language::Spanish,
        "sv" | "swedish" => Language::Swedish,
        "ta" | "tamil" => Language::Tamil,
        "tr" | "turkish" => Language::Turkish,
        _ => return None,
    };
    Some(l)
}

/// Run an analyzer over `text` and collect the token texts
/// (tantivy-py `TextAnalyzer.analyze`).
pub fn analyze(analyzer: &TextAnalyzer, text: &str) -> Vec<String> {
    let mut a = analyzer.clone();
    let mut stream = a.token_stream(text);
    let mut out = Vec::new();
    while stream.advance() {
        out.push(stream.token().text.clone());
    }
    out
}

/// `paperless_text_analyzer`: simple -> remove_long(129) -> lowercase -> ascii_fold [-> stemmer]
///
/// An unsupported language disables stemming (upstream logs a warning).
pub fn paperless_text_analyzer(language: Option<&str>) -> TextAnalyzer {
    let builder = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(TOKEN_REMOVE_LONG_LIMIT))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .dynamic();
    let lang = language.filter(|l| !l.is_empty()).and_then(|l| {
        let found = language_for(l);
        if found.is_none() {
            tracing::warn!(
                "Unsupported search language '{l}' - stemming disabled."
            );
        }
        found
    });
    match lang {
        Some(lang) => builder.filter_dynamic(Stemmer::new(lang)).build(),
        None => builder.build(),
    }
}

/// `_pattern_stemmer`: raw -> remove_long(129) -> stemmer, or None when stemming is off.
fn pattern_stemmer(language: Option<&str>) -> Option<TextAnalyzer> {
    let lang = language.filter(|l| !l.is_empty()).and_then(language_for)?;
    Some(
        TextAnalyzer::builder(RawTokenizer::default())
            .filter(RemoveLongFilter::limit(TOKEN_REMOVE_LONG_LIMIT))
            .filter(Stemmer::new(lang))
            .build(),
    )
}

/// `stem_pattern_text`: stem an already lowercased/folded run; unchanged when
/// stemming is off or the stem step does not yield exactly one token.
pub fn stem_pattern_text(text: &str, language: Option<&str>) -> String {
    let Some(a) = pattern_stemmer(language) else {
        return text.to_string();
    };
    let mut tokens = analyze(&a, text);
    if tokens.len() != 1 {
        return text.to_string();
    }
    tokens.pop().unwrap_or_else(|| text.to_string())
}

/// `_simple_analyzer`: simple -> lowercase -> ascii_fold (sort shadow fields).
pub fn simple_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

/// `_bigram_analyzer`: ngram(2,2,prefix_only=False) -> lowercase.
pub fn bigram_analyzer() -> TextAnalyzer {
    // NgramTokenizer::new only fails for min_gram > max_gram or min_gram == 0.
    // (2, 2) is always valid; the fallback only keeps this function total.
    match NgramTokenizer::new(2, 2, false) {
        Ok(t) => TextAnalyzer::builder(t).filter(LowerCaser).build(),
        Err(_) => TextAnalyzer::builder(RawTokenizer::default())
            .filter(LowerCaser)
            .build(),
    }
}

/// `_simple_search_analyzer`: regex(\S+) -> remove_long(129) -> lowercase -> ascii_fold.
pub fn simple_search_analyzer() -> TextAnalyzer {
    let builder = match RegexTokenizer::new(r"\S+") {
        Ok(tok) => TextAnalyzer::builder(tok).dynamic(),
        // Constant pattern, cannot fail; fall back to whitespace splitting.
        Err(_) => TextAnalyzer::builder(tantivy::tokenizer::WhitespaceTokenizer::default()).dynamic(),
    };
    builder
        .filter_dynamic(RemoveLongFilter::limit(TOKEN_REMOVE_LONG_LIMIT))
        .filter_dynamic(LowerCaser)
        .filter_dynamic(AsciiFoldingFilter)
        .build()
}

/// `register_tokenizers`: must be called on every opened Index.
pub fn register_tokenizers(index: &Index, language: Option<&str>) {
    index
        .tokenizers()
        .register(PAPERLESS_TEXT, paperless_text_analyzer(language));
    index.tokenizers().register(SIMPLE_ANALYZER, simple_analyzer());
    index.tokenizers().register(BIGRAM_ANALYZER, bigram_analyzer());
    index
        .tokenizers()
        .register(SIMPLE_SEARCH_ANALYZER, simple_search_analyzer());
    index
        .fast_field_tokenizer()
        .register(SIMPLE_ANALYZER, simple_analyzer());
}

fn simple_search_shared() -> &'static TextAnalyzer {
    static A: OnceLock<TextAnalyzer> = OnceLock::new();
    A.get_or_init(simple_search_analyzer)
}

fn ascii_fold_shared() -> &'static TextAnalyzer {
    static A: OnceLock<TextAnalyzer> = OnceLock::new();
    A.get_or_init(|| {
        TextAnalyzer::builder(RawTokenizer::default())
            .filter(AsciiFoldingFilter)
            .build()
    })
}

fn autocomplete_shared() -> &'static TextAnalyzer {
    static A: OnceLock<TextAnalyzer> = OnceLock::new();
    A.get_or_init(simple_analyzer)
}

/// `simple_search_tokens`: tokenize like simple_title/simple_content are indexed.
pub fn simple_search_tokens(text: &str) -> Vec<String> {
    analyze(simple_search_shared(), text)
}

/// `autocomplete_tokens`: simple -> lowercase -> ascii_fold.
pub fn autocomplete_tokens(text: &str) -> Vec<String> {
    analyze(autocomplete_shared(), text)
}

/// `ascii_fold`: fold to ASCII like the content tokenizers (raw -> ascii_fold).
pub fn ascii_fold(text: &str) -> String {
    analyze(ascii_fold_shared(), text)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Python's `str.lower()` (full Unicode lowercase mapping), used before `ascii_fold`.
pub fn py_lower(text: &str) -> String {
    text.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paperless_text_pipeline() {
        let a = paperless_text_analyzer(Some("en"));
        assert_eq!(analyze(&a, "Invoices Café"), vec!["invoic", "cafe"]);
        let a = paperless_text_analyzer(None);
        assert_eq!(analyze(&a, "Invoices Café"), vec!["invoices", "cafe"]);
        let a = paperless_text_analyzer(Some("xx"));
        assert_eq!(analyze(&a, "Invoices"), vec!["invoices"]);
    }

    #[test]
    fn folding() {
        assert_eq!(ascii_fold("straße"), "strasse");
        assert_eq!(ascii_fold(""), "");
        assert_eq!(autocomplete_tokens("Ærø Straße"), vec!["aero", "strasse"]);
    }

    #[test]
    fn simple_search_keeps_punctuation() {
        assert_eq!(
            simple_search_tokens("Medical-History  2024/01"),
            vec!["medical-history", "2024/01"]
        );
    }

    #[test]
    fn bigrams() {
        assert_eq!(analyze(&bigram_analyzer(), "東京都"), vec!["東京", "京都"]);
    }

    #[test]
    fn stem_pattern() {
        assert_eq!(stem_pattern_text("invoice", Some("en")), "invoic");
        assert_eq!(stem_pattern_text("invoice", None), "invoice");
    }
}
