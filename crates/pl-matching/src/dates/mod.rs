// upstream: src/documents/plugins/date_parsing/base.py@v3.4.2 (DateParserConfig,
//           DateParserPluginBase._parse_string/_filter_date)
//           src/documents/plugins/date_parsing/regex_parser.py@v3.4.2 (RegexDateParserPlugin)
//           src/documents/plugins/date_parsing/__init__.py@v3.4.2 (get_date_parser)
//           src/paperless/settings/custom.py@v3.4.2 (parse_dateparser_languages, parse_ignore_dates)
//           src/paperless/utils.py@v3.4.2 (ocr_to_dateparser_languages, OCR_TO_DATEPARSER_LANGUAGES)
//
// Date-plugin discovery (entry points) is not ported: the Rust worker always uses the
// regex parser (the upstream default when no plugin is installed).

mod dp;
mod dp_fresh;
mod dp_locale;
mod dp_parser;
mod dp_tz;

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use regex::Regex;

pub use dp::{DateparserError, DpSettings, dateparser_parse};
pub use dp_parser::PreferDayOfMonth;

/// `DateParserConfig`
#[derive(Debug, Clone)]
pub struct DateParserConfig {
    pub languages: Vec<String>,
    pub timezone_str: String,
    pub ignore_dates: HashSet<NaiveDate>,
    /// "now" for filtering future dates (`timezone.now()` when the parser is created)
    pub reference_time: DateTime<FixedOffset>,
    pub filename_date_order: Option<String>,
    pub content_date_order: String,
}

/// `RegexDateParserPlugin.DATE_REGEX`, rewritten for the regex crate.
///
/// Upstream (Python `regex`, IGNORECASE) wraps each of the 7 alternatives in
/// `(\b|(?!=([_-])))` ... `(\b|(?=([_-])))`. The leading group always matches empty
/// (every alternative starts with a digit or letter, never "="), so it is dropped.
/// The trailing group after the final digit means "next char is a non-word char, `_`
/// or end"; it is expressed by consuming `(?:[\W_]|\z)` after the capture group `d`,
/// whose end is used as match end. Leftmost-first priorities are unchanged, so the
/// matches are identical to upstream's.
static DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    let l = "[a-zéûäëčžúřěáíóńźçŞğü]";
    let alts = [
        r"(\d{1,2})[\./-](\d{1,2})[\./-](\d{4}|\d{2})".to_string(),
        r"(\d{4}|\d{2})[\./-](\d{1,2})[\./-](\d{1,2})".to_string(),
        format!(r"(\d{{1,2}}[\. ]+{l}{{3,9}} \d{{4}}|{l}{{3,9}} \d{{1,2}}, \d{{4}})"),
        r"([^\W\d_]{3,9} \d{1,2}, (\d{4}))".to_string(),
        r"([^\W\d_]{3,9} \d{4})".to_string(),
        r"(\d{1,2}[^ 0-9]{2}[\. ]+[^ ]{3,9}[ \./-]\d{4})".to_string(),
        format!(r"(\b\d{{1,2}}[ \./-]{l}{{3}}[ \./-]\d{{4}})"),
    ];
    let src = format!(r"(?i)(?P<d>{})(?:[\W_]|\z)", alts.join("|"));
    Regex::new(&src).unwrap()
});

/// All DATE_REGEX match strings in `content` (== `match.group(0)` of `finditer`).
pub fn date_regex_matches(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= content.len() {
        let Some(c) = DATE_REGEX.captures_at(content, pos) else { break };
        let Some(d) = c.name("d") else { break };
        out.push(d.as_str());
        pos = d.end();
    }
    out
}

/// `RegexDateParserPlugin` (default date parser).
pub struct RegexDateParser {
    pub config: DateParserConfig,
}

impl RegexDateParser {
    pub fn new(config: DateParserConfig) -> Self {
        Self { config }
    }

    fn settings(&self, date_order: &str) -> DpSettings {
        DpSettings {
            date_order: date_order.to_string(),
            prefer_day_of_month: PreferDayOfMonth::First,
            timezone: self.config.timezone_str.clone(),
            return_aware: true,
            now: self.config.reference_time.with_timezone(&Utc),
        }
    }

    /// `_parse_string(date_string, date_order)`: dateparser errors are logged, -> None.
    pub fn parse_string(&self, date_string: &str, date_order: &str) -> Option<DateTime<FixedOffset>> {
        match dateparser_parse(date_string, &self.settings(date_order), &self.config.languages) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(
                    target: "documents.plugins.date_parsing.base",
                    "Error while parsing date string '{}': {}",
                    date_string,
                    e.0
                );
                None
            }
        }
    }

    /// `_filter_date(date)`
    pub fn filter_date(&self, date: Option<DateTime<FixedOffset>>) -> Option<DateTime<FixedOffset>> {
        let d = date?;
        (d.year() > 1900 && d <= self.config.reference_time && !self.config.ignore_dates.contains(&d.date_naive()))
            .then_some(d)
    }

    fn process_content(&self, content: &str, date_order: &str) -> Vec<DateTime<FixedOffset>> {
        date_regex_matches(content)
            .into_iter()
            .filter_map(|m| self.filter_date(self.parse_string(m, date_order)))
            .collect()
    }

    /// `parse(filename, content)`: filename dates first (if FILENAME_DATE_ORDER is set),
    /// then content dates, each in text order. Lazy like the Python generator.
    pub fn parse<'a>(&'a self, filename: &'a str, content: &'a str) -> impl Iterator<Item = DateTime<FixedOffset>> + 'a {
        let fname = self
            .config
            .filename_date_order
            .as_deref()
            .map(|order| date_regex_matches(filename).into_iter().map(move |m| (m, order)))
            .into_iter()
            .flatten();
        let content_order = self.config.content_date_order.as_str();
        let body = date_regex_matches(content).into_iter().map(move |m| (m, content_order));
        fname.chain(body).filter_map(move |(m, order)| self.filter_date(self.parse_string(m, order)))
    }

    /// Convenience: all dates (`list(parser.parse(...))`).
    pub fn parse_all(&self, filename: &str, content: &str) -> Vec<DateTime<FixedOffset>> {
        let mut v = Vec::new();
        if let Some(order) = &self.config.filename_date_order {
            v.extend(self.process_content(filename, order));
        }
        v.extend(self.process_content(content, &self.config.content_date_order));
        v
    }
}

/// `OCR_TO_DATEPARSER_LANGUAGES`
pub const OCR_TO_DATEPARSER_LANGUAGES: &[(&str, &str)] = &[
    ("afr", "af"), ("amh", "am"), ("ara", "ar"), ("asm", "as"), ("ast", "ast"), ("aze", "az"), ("bel", "be"),
    ("bul", "bg"), ("ben", "bn"), ("bod", "bo"), ("bre", "br"), ("bos", "bs"), ("cat", "ca"), ("cher", "chr"),
    ("ces", "cs"), ("cym", "cy"), ("dan", "da"), ("deu", "de"), ("dzo", "dz"), ("ell", "el"), ("eng", "en"),
    ("epo", "eo"), ("spa", "es"), ("est", "et"), ("eus", "eu"), ("fas", "fa"), ("fin", "fi"), ("fil", "fil"),
    ("fao", "fo"), ("fra", "fr"), ("fry", "fy"), ("gle", "ga"), ("gla", "gd"), ("glg", "gl"), ("guj", "gu"),
    ("heb", "he"), ("hin", "hi"), ("hrv", "hr"), ("hun", "hu"), ("hye", "hy"), ("ind", "id"), ("isl", "is"),
    ("ita", "it"), ("jpn", "ja"), ("kat", "ka"), ("kaz", "kk"), ("khm", "km"), ("knda", "kn"), ("kor", "ko"),
    ("kir", "ky"), ("ltz", "lb"), ("lao", "lo"), ("lit", "lt"), ("lav", "lv"), ("mal", "ml"), ("mon", "mn"),
    ("mar", "mr"), ("msa", "ms"), ("mlt", "mt"), ("mya", "my"), ("nep", "ne"), ("nld", "nl"), ("ori", "or"),
    ("pan", "pa"), ("pol", "pl"), ("pus", "ps"), ("por", "pt"), ("que", "qu"), ("ron", "ro"), ("rus", "ru"),
    ("sin", "si"), ("slk", "sk"), ("slv", "sl"), ("sqi", "sq"), ("srp", "sr"), ("swe", "sv"), ("swa", "sw"),
    ("tam", "ta"), ("tel", "te"), ("tha", "th"), ("tir", "ti"), ("tgl", "tl"), ("ton", "to"), ("tur", "tr"),
    ("uig", "ug"), ("ukr", "uk"), ("urd", "ur"), ("uzb", "uz"), ("via", "vi"), ("yid", "yi"), ("yor", "yo"),
    ("chi", "zh"),
];

fn title_case(s: &str) -> String {
    // Python str.title() for a single word
    let mut out = String::new();
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_alphabetic();
    }
    out
}

/// `ocr_to_dateparser_languages(ocr_languages)`
pub fn ocr_to_dateparser_languages(ocr_languages: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for ocr_language in ocr_languages.split('+') {
        let mut parts = ocr_language.split('_');
        let lang_part = parts.next().unwrap_or("");
        let script = parts.next();
        let Some((_, language_part)) = OCR_TO_DATEPARSER_LANGUAGES.iter().find(|(k, _)| *k == lang_part) else {
            tracing::debug!(
                target: "paperless.utils",
                "Unable to map OCR language \"{lang_part}\" to dateparser locale. "
            );
            continue;
        };
        if !dp_locale::is_valid_locale(language_part) {
            tracing::warn!(
                target: "paperless.utils",
                "Error auto-configuring dateparser languages. Set PAPERRUST_DATE_PARSER_LANGUAGES parameter to avoid this. Detail: Unknown locale(s): '{language_part}'"
            );
            return vec![];
        }
        let dl = match script {
            Some(sc) => {
                let candidate = format!("{language_part}-{}", title_case(sc));
                if dp_locale::is_valid_locale(&candidate) {
                    candidate
                } else {
                    tracing::info!(
                        target: "paperless.utils",
                        "Language variant '{candidate}' not supported by dateparser; falling back to base language '{language_part}'. You can manually set PAPERRUST_DATE_PARSER_LANGUAGES if needed."
                    );
                    language_part.to_string()
                }
            }
            None => language_part.to_string(),
        };
        if !result.contains(&dl) {
            result.push(dl);
        }
    }
    if result.is_empty() {
        tracing::info!(
            target: "paperless.utils",
            "Unable to automatically determine dateparser languages from OCR_LANGUAGE, falling back to multi-language support."
        );
    }
    result
}

/// `parse_dateparser_languages(languages)` (PAPERLESS_DATE_PARSER_LANGUAGES).
/// Err = upstream ValueError at settings import.
pub fn parse_dateparser_languages(languages: Option<&str>) -> Result<Vec<String>, String> {
    let mut list: Vec<String> = match languages {
        Some(l) if !l.is_empty() => l.split('+').map(str::to_string).collect(),
        _ => vec![],
    };
    let mut i = 0;
    while i < list.len() {
        if list[i].starts_with("zh-") && !list.iter().any(|x| x == "zh") {
            tracing::warn!(
                target: "paperless.settings",
                "Chinese locale detected: {}. dateparser might fail to parse some dates with this locale, so Chinese (\"zh\") will be used as a fallback.",
                list[i]
            );
            list.push("zh".into());
        }
        i += 1;
    }
    let locs = dp_locale::load_locales(&list)?;
    if list.is_empty() {
        // get_locale_map(locales=[]) loads all languages
        return Ok(locs.into_iter().map(|l| l.shortname).collect());
    }
    Ok(locs.into_iter().map(|l| l.shortname).collect())
}

/// `parse_ignore_dates(env_ignore, date_order)` (PAPERLESS_IGNORE_DATES).
/// dateparser defaults apply: all languages, TIMEZONE local, PREFER_DAY_OF_MONTH current.
pub fn parse_ignore_dates(env_ignore: &str, date_order: &str, now: DateTime<Utc>) -> HashSet<NaiveDate> {
    let mut out = HashSet::new();
    let st = DpSettings {
        date_order: date_order.to_string(),
        prefer_day_of_month: PreferDayOfMonth::Current,
        timezone: "local".into(),
        return_aware: false,
        now,
    };
    for s in env_ignore.split(',') {
        if let Ok(Some(d)) = dateparser_parse(s, &st, &[]) {
            out.insert(d.date_naive());
        }
    }
    out
}

/// `get_date_parser()` configuration: DATE_PARSER_LANGUAGES or derived from OCR language.
pub fn resolve_languages(date_parser_languages: Option<&[String]>, ocr_language: &str) -> Vec<String> {
    match date_parser_languages {
        Some(l) if !l.is_empty() => l.to_vec(),
        _ => ocr_to_dateparser_languages(ocr_language),
    }
}
