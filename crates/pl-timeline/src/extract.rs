//! Deadlines and terms in the document content (version [`EXTRACTOR_VERSION`]).
//!
//! Procedure per hit of a key phrase (German + English): in the window after it (at most
//! [`WINDOW`] characters up to the start of the date, no blank line in between) the first date of the
//! **upstream date detection** (`DATE_REGEX` + dateparser from `pl-matching`, same
//! `DATE_ORDER`/languages as on ingestion). Unlike the document date, future dates are
//! allowed (deadlines are mostly in the future); `IGNORE_DATES` does not apply.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate, Utc};
use pl_matching::dates::{date_regex_matches, parse_dateparser_languages, resolve_languages};
use pl_matching::{DateParserConfig, RegexDateParser};
use regex::Regex;
use serde::Serialize;

use crate::DateKind;

/// Increase when the detection changes (outdated results are recomputed).
pub const EXTRACTOR_VERSION: i32 = 1;
/// Largest distance phrase → start of date (characters).
pub const WINDOW: usize = 48;
/// At most this many results per document.
pub const MAX_PER_DOCUMENT: usize = 20;
/// Only the first characters of the content.
pub const MAX_CHARS: usize = 200_000;

/// A detected date.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub kind: DateKind,
    pub date: NaiveDate,
    pub confidence: f32,
    /// Phrase up to and including the date (whitespace collapsed, ≤ 120 characters).
    pub phrase: String,
    /// Page (1-based) if the content has page breaks (`\x0c`).
    pub page: Option<i32>,
}

struct Pattern {
    kind: DateKind,
    re: Regex,
}

fn p(kind: DateKind, src: &str) -> Pattern {
    Pattern { kind, re: Regex::new(&format!(r"(?i)\b(?:{src})\b")).expect("timeline pattern") }
}

/// Generic phrases (lower confidence).
static GENERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:garantie|warranty|expires?|expiry|expiration|end date|endet am|k(?:ü|ue)ndigungsfrist|f(?:ä|ae)llig)$").unwrap()
});

/// Blank line (ends the window).
static BLANK_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n[ \t\r\x0c]*\n").unwrap());

/// Words between phrase and date that point to a start rather than an end.
static START_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:ab|seit|vom|from|since)\b").unwrap());

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    vec![
        p(
            DateKind::Due,
            r"f(?:ä|ae)lligkeit(?:sdatum)?|f(?:ä|ae)llig(?:\s+(?:am|bis(?:\s+(?:zum|am|sp(?:ä|ae)testens))?|zum|per))?|zahlbar\s+(?:bis(?:\s+(?:zum|am|sp(?:ä|ae)testens))?|am|innert|per)|zahlungsziel|zahlungsfrist(?:\s+bis)?|zahlungstermin|(?:(?:ü|ue)berweisen|bezahlen|zahlen)\s+(?:sie\s+)?(?:den\s+betrag\s+)?bis(?:\s+(?:zum|sp(?:ä|ae)testens))?|due\s+(?:date|on|by)|payment\s+due(?:\s+(?:date|on|by))?|payable\s+(?:by|until|on|before)|please\s+pay\s+(?:by|before)|pay\s+by",
        ),
        p(
            DateKind::CancelBy,
            r"k(?:ü|ue)ndigung\s+(?:bis|sp(?:ä|ae)testens|zum|per)(?:\s+(?:zum|am))?|k(?:ü|ue)ndbar\s+(?:bis|zum|per)(?:\s+(?:zum|am))?|k(?:ü|ue)ndigungstermin|k(?:ü|ue)ndigungsfrist(?:\s+bis)?|cancel(?:lation)?\s+(?:by|before|deadline)|notice\s+(?:by|before|deadline)|terminate\s+(?:by|before)",
        ),
        p(
            DateKind::ValidFrom,
            r"g(?:ü|ue)ltig\s+(?:ab|von|vom)|vertragsbeginn|versicherungsbeginn|laufzeit\s+(?:ab|vom|von)|valid\s+from|start\s+date|effective\s+(?:date|from|as\s+of)",
        ),
        p(
            DateKind::ValidUntil,
            r"g(?:ü|ue)ltig\s+bis(?:\s+(?:zum|am))?|g(?:ü|ue)ltigkeit\s+bis|g(?:ü|ue)ltigkeitsdatum|ablaufdatum|l(?:ä|ae)uft\s+bis(?:\s+(?:zum|am))?|laufzeit\s+bis(?:\s+(?:zum|am))?|vertragsende|vertragsablauf|befristet\s+bis(?:\s+(?:zum|am))?|endet\s+am|valid\s+(?:until|through|thru|till)|expires?(?:\s+on)?|expiry(?:\s+date)?|expiration(?:\s+date)?|end\s+date",
        ),
        p(
            DateKind::WarrantyUntil,
            r"garantie\s+(?:bis|g(?:ü|ue)ltig\s+bis|endet\s+am|l(?:ä|ae)uft\s+bis)|garantiezeit\s+bis|garantieende|gew(?:ä|ae)hrleistung\s+bis|warranty\s+(?:until|expires(?:\s+on)?|valid\s+until|ends(?:\s+on)?|end\s+date|expiration)|garantie|warranty",
        ),
    ]
});

/// Date parser with the settings of ingestion.
pub struct DateParse {
    parser: RegexDateParser,
    order: String,
}

impl DateParse {
    /// `languages` (dateparser abbreviations; de and en are always added), `date_order` (DMY …).
    pub fn new(mut languages: Vec<String>, timezone: &str, date_order: &str) -> Self {
        for l in ["de", "en"] {
            if !languages.iter().any(|x| x == l) {
                languages.push(l.into());
            }
        }
        let order = if date_order.trim().is_empty() { "DMY".to_string() } else { date_order.trim().to_uppercase() };
        let cfg = DateParserConfig {
            languages,
            timezone_str: if timezone.trim().is_empty() { "UTC".into() } else { timezone.trim().into() },
            ignore_dates: Default::default(),
            reference_time: Utc::now().fixed_offset(),
            filename_date_order: None,
            content_date_order: order.clone(),
        };
        DateParse { parser: RegexDateParser::new(cfg), order }
    }

    /// From settings (`PAPERLESS_DATE_ORDER`, `PAPERLESS_DATE_PARSER_LANGUAGES`,
    /// `PAPERLESS_OCR_LANGUAGE`, `PAPERLESS_TIME_ZONE`) via `get` (e.g. `pl_core::config::var`).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let order = get("PAPERLESS_DATE_ORDER").unwrap_or_else(|| "DMY".into());
        let ocr = get("PAPERLESS_OCR_LANGUAGE").unwrap_or_else(|| "eng".into());
        let explicit = parse_dateparser_languages(get("PAPERLESS_DATE_PARSER_LANGUAGES").as_deref()).ok();
        let langs = resolve_languages(explicit.as_deref(), &ocr);
        let tz = get("PAPERLESS_TIME_ZONE").unwrap_or_else(|| "UTC".into());
        DateParse::new(langs, &tz, &order)
    }

    fn parse(&self, s: &str) -> Option<NaiveDate> {
        // ISO 8601 (YYYY-MM-DD) independent of DATE_ORDER
        let iso = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
        let d = match iso {
            Some(d) => d,
            None => self.parser.parse_string(s, &self.order)?.date_naive(),
        };
        (1900..=2200).contains(&d.year()).then_some(d)
    }
}

fn char_floor(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte end after at most `n` characters from `start`.
fn advance(s: &str, start: usize, n: usize) -> usize {
    s[start..].char_indices().nth(n).map(|(i, _)| start + i).unwrap_or(s.len())
}

fn flat(s: &str, max: usize) -> String {
    let f = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if f.chars().count() <= max { f } else { f.chars().take(max).collect() }
}

/// All detected dates in `content`; `created` (document date) lowers the confidence of earlier
/// dates. Per (kind, date) the most confident result, sorted by date.
pub fn extract(content: &str, created: Option<NaiveDate>, dp: &DateParse) -> Vec<Found> {
    let end = advance(content, 0, MAX_CHARS);
    let text = &content[..end];
    let paged = text.contains('\x0c');
    let mut best: BTreeMap<(DateKind, NaiveDate), Found> = BTreeMap::new();
    for pat in PATTERNS.iter() {
        for m in pat.re.find_iter(text) {
            let win_end = advance(text, m.end(), WINDOW + 16);
            let mut window = &text[m.end()..win_end];
            // no blank line in between
            if let Some(b) = BLANK_LINE.find(window) {
                window = &window[..b.start()];
            }
            let Some(ds) = date_regex_matches(window).into_iter().next() else {
                continue;
            };
            let off = ds.as_ptr() as usize - window.as_ptr() as usize;
            let dist = window[..off].chars().count();
            if dist > WINDOW {
                continue;
            }
            let gap = &window[..off];
            if pat.kind != DateKind::ValidFrom && START_WORDS.is_match(gap) {
                continue;
            }
            let Some(date) = dp.parse(ds) else { continue };
            let mut conf = 0.95 - dist as f32 / 100.0;
            if GENERIC.is_match(m.as_str().split_whitespace().collect::<Vec<_>>().join(" ").as_str()) {
                conf -= 0.15;
            }
            if created.is_some_and(|c| date < c) {
                conf -= 0.2;
            }
            let conf = (conf.max(0.5) * 100.0).round() / 100.0;
            let phrase_end = char_floor(text, m.end() + off + ds.len());
            let f = Found {
                kind: pat.kind,
                date,
                confidence: conf,
                phrase: flat(&text[m.start()..phrase_end], 120),
                page: paged.then(|| text[..m.start()].matches('\x0c').count() as i32 + 1),
            };
            match best.get(&(f.kind, f.date)) {
                Some(old) if old.confidence >= f.confidence => {}
                _ => {
                    best.insert((f.kind, f.date), f);
                }
            }
        }
    }
    let mut out: Vec<Found> = best.into_values().collect();
    if out.len() > MAX_PER_DOCUMENT {
        out.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(MAX_PER_DOCUMENT);
    }
    out.sort_by_key(|f| (f.date, f.kind));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dp() -> DateParse {
        DateParse::new(vec!["de".into(), "en".into()], "Europe/Zurich", "DMY")
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn kinds(v: &[Found]) -> Vec<(DateKind, NaiveDate)> {
        v.iter().map(|f| (f.kind, f.date)).collect()
    }

    #[test]
    fn german_invoice() {
        let c = "Swisscom (Schweiz) AG\nRechnung vom 02.10.2026\nTotal CHF 89.90\nZahlbar bis 30.11.2026 auf das Konto …";
        let r = extract(c, Some(d(2026, 10, 2)), &dp());
        assert_eq!(kinds(&r), vec![(DateKind::Due, d(2026, 11, 30))], "{r:?}");
        assert!(r[0].confidence >= 0.9, "{r:?}");
        assert_eq!(r[0].phrase, "Zahlbar bis 30.11.2026");
        assert_eq!(r[0].page, None);
    }

    #[test]
    fn german_phrases() {
        let cases = [
            ("Der Betrag ist fällig am 15.01.2027.", DateKind::Due, d(2027, 1, 15)),
            ("Fälligkeitsdatum: 1. März 2027", DateKind::Due, d(2027, 3, 1)),
            ("Zahlungsziel 05.12.2026", DateKind::Due, d(2026, 12, 5)),
            ("Bitte überweisen Sie bis zum 20.12.2026", DateKind::Due, d(2026, 12, 20)),
            ("Ihr Ausweis ist gültig bis 31.08.2031", DateKind::ValidUntil, d(2031, 8, 31)),
            ("Gueltig bis: 2028-06-30", DateKind::ValidUntil, d(2028, 6, 30)),
            ("Laufzeit bis 31.12.2027", DateKind::ValidUntil, d(2027, 12, 31)),
            ("Kündigung bis spätestens 30.09.2027 möglich", DateKind::CancelBy, d(2027, 9, 30)),
            ("Kündbar zum 31.03.2027", DateKind::CancelBy, d(2027, 3, 31)),
            ("Kündigungstermin: 31.12.2026", DateKind::CancelBy, d(2026, 12, 31)),
            ("Garantie bis 14.02.2028", DateKind::WarrantyUntil, d(2028, 2, 14)),
            ("Gewährleistung bis 01.07.2028", DateKind::WarrantyUntil, d(2028, 7, 1)),
            ("Versicherungsbeginn 01.01.2026", DateKind::ValidFrom, d(2026, 1, 1)),
            ("Gültig ab 01.04.2026", DateKind::ValidFrom, d(2026, 4, 1)),
        ];
        for (text, kind, date) in cases {
            let r = extract(text, None, &dp());
            assert!(r.iter().any(|f| f.kind == kind && f.date == date), "{text}: {r:?}");
        }
    }

    #[test]
    fn english_phrases() {
        let cases = [
            ("Payment due: 12/01/2026", DateKind::Due, d(2026, 1, 12)),
            ("Due date 15 January 2027", DateKind::Due, d(2027, 1, 15)),
            ("Payable by 30.11.2026", DateKind::Due, d(2026, 11, 30)),
            ("This card is valid until 31.08.2030", DateKind::ValidUntil, d(2030, 8, 31)),
            ("Expiry date: 2029-05-31", DateKind::ValidUntil, d(2029, 5, 31)),
            ("Cancel by 01.03.2027 to avoid renewal", DateKind::CancelBy, d(2027, 3, 1)),
            ("Warranty until 20.06.2028", DateKind::WarrantyUntil, d(2028, 6, 20)),
            ("Valid from 01.02.2026", DateKind::ValidFrom, d(2026, 2, 1)),
        ];
        for (text, kind, date) in cases {
            let r = extract(text, None, &dp());
            assert!(r.iter().any(|f| f.kind == kind && f.date == date), "{text}: {r:?}");
        }
    }

    #[test]
    fn negatives_and_rules() {
        // no date after the phrase
        assert!(extract("Kündigungsfrist drei Monate. Garantie 2 Jahre.", None, &dp()).is_empty());
        // "ab Kaufdatum" (German: "from purchase date") is a start, not a warranty end
        let r = extract("Garantie 24 Monate ab Kaufdatum 12.03.2025", None, &dp());
        assert!(r.is_empty(), "{r:?}");
        // too far away
        let far = format!("fällig {} 30.11.2026", "x".repeat(60));
        assert!(extract(&far, None, &dp()).is_empty());
        // blank line separates
        assert!(extract("Zahlbar bis\n\n30.11.2026", None, &dp()).is_empty());
        // date without phrase does not count
        assert!(extract("Rechnung vom 30.11.2026", None, &dp()).is_empty());
    }

    #[test]
    fn confidence_page_and_dedupe() {
        let c = "Seite 1\x0cSeite 2: Garantie 14.02.2028\nGarantie bis 14.02.2028";
        let r = extract(c, Some(d(2026, 1, 1)), &dp());
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].page, Some(2));
        assert!(r[0].confidence > 0.9, "spezifische Phrase gewinnt: {r:?}");
        // generic phrase alone: lower confidence
        let g = extract("Garantie: 14.02.2028", None, &dp());
        assert!(g[0].confidence < 0.85 && g[0].confidence >= 0.5, "{g:?}");
        // before the document date: lower
        let early = extract("fällig am 01.01.2020", Some(d(2026, 1, 1)), &dp());
        assert!(early[0].confidence < 0.8, "{early:?}");
        // two-digit year
        let y2 = extract("zahlbar bis 30.11.26", None, &dp());
        assert_eq!(y2[0].date, d(2026, 11, 30));
    }

    #[test]
    fn lookup_config() {
        let p = DateParse::from_lookup(|k| match k {
            "PAPERLESS_DATE_ORDER" => Some("MDY".into()),
            "PAPERLESS_OCR_LANGUAGE" => Some("deu+eng".into()),
            _ => None,
        });
        let r = extract("Payment due 12/01/2026", None, &p);
        assert_eq!(r[0].date, d(2026, 12, 1));
    }
}
