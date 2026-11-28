//! Loop guard for AI transcriptions (docs/OCR-NEXT.md 2.7 "Schleifenschutz").
//!
//! In the measurements (results/ocr-eval/2026-11-24) almost every image model got stuck at least once:
//! the same line hundreds of times (`103,59 €`, an invoice number), the same sentence or word group over
//! and over until the token limit, or a character pattern inside one word (`4.0.0.0.0…`). The guard
//! finds such repetitions, cuts the text before the first repetition and reports the page/crop as
//! uncertain – the caller then keeps the classic OCR text.
//!
//! Only *consecutive* repetitions count: legitimate documents repeat boilerplate (headers on every page,
//! the same table row structure), but not the same line five times in a row.

/// Result of [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guarded {
    /// Text, cut before the first repetition if a loop was found.
    pub text: String,
    pub looped: bool,
}

/// Identical consecutive non-empty lines from which on it is a loop.
const LINE_RUN: usize = 5;
/// For very short lines (< 4 visible characters, e.g. "1" or "€") a longer run is needed.
const SHORT_LINE_RUN: usize = 8;
/// Longest repeated word group that is searched for (in words).
const MAX_PERIOD_WORDS: usize = 60;
/// Longest repeated character pattern inside a line (in characters).
const MAX_PERIOD_CHARS: usize = 8;

fn has_alnum(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

/// Repetitions needed for a word group of `p` words: short groups need more repetitions
/// (`p = 1, 2` → 12, `3` → 8, `4` → 6, `5` → 5, `≥ 6` → 4).
fn needed_reps(p: usize) -> usize {
    24usize.div_ceil(p).clamp(4, 12)
}

/// Byte offset of the second occurrence in a run of identical consecutive lines (blank lines between
/// them are ignored).
fn line_loop(text: &str) -> Option<usize> {
    let mut prev: Option<&str> = None;
    let mut run = 0usize;
    let mut second: Option<usize> = None;
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if prev == Some(line) {
            run += 1;
            if run == 2 {
                second = Some(start);
            }
        } else {
            prev = Some(line);
            run = 1;
            second = None;
        }
        let need = if line.chars().filter(|c| !c.is_whitespace()).count() < 4 { SHORT_LINE_RUN } else { LINE_RUN };
        if run >= need && has_alnum(line) {
            return second;
        }
    }
    None
}

/// Byte offset where the second copy of a repeated word group starts (earliest over all group sizes).
fn word_loop(text: &str) -> Option<usize> {
    let mut toks: Vec<(usize, &str)> = Vec::new();
    let mut i = 0usize;
    for part in text.split_inclusive(char::is_whitespace) {
        let w = part.trim_end();
        if !w.is_empty() {
            toks.push((i, w));
        }
        i += part.len();
    }
    let n = toks.len();
    let mut best: Option<usize> = None;
    for p in 1..=MAX_PERIOD_WORDS.min(n / 2) {
        let need = needed_reps(p);
        let mut j = 0usize;
        while j + p < n {
            if toks[j].1 != toks[j + p].1 {
                j += 1;
                continue;
            }
            let start = j;
            while j + p < n && toks[j].1 == toks[j + p].1 {
                j += 1;
            }
            // run of `j - start` matching positions → the group repeats (run / p) + 1 times
            let reps = (j - start) / p + 1;
            if reps >= need && toks[start..start + p].iter().any(|t| has_alnum(t.1)) {
                let cut = toks[start + p].0;
                best = Some(best.map_or(cut, |b| b.min(cut)));
                break;
            }
        }
    }
    best
}

/// Byte offset inside a line where a repeated character pattern (`4.0.0.0…`, `ababab…`) continues.
fn char_loop(text: &str) -> Option<usize> {
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let base = offset;
        offset += raw.len();
        let chars: Vec<(usize, char)> = raw.char_indices().collect();
        if chars.len() < 40 {
            continue;
        }
        for p in 1..=MAX_PERIOD_CHARS {
            let need = (p * 10).max(30);
            let mut j = 0usize;
            while j + p < chars.len() {
                if chars[j].1 != chars[j + p].1 {
                    j += 1;
                    continue;
                }
                let start = j;
                while j + p < chars.len() && chars[j].1 == chars[j + p].1 {
                    j += 1;
                }
                if j - start >= need && chars[start..start + p].iter().any(|c| c.1.is_alphanumeric()) {
                    return Some(base + chars[start + p].0);
                }
            }
        }
    }
    None
}

/// Checks a model answer for repetition loops.
pub fn check(text: &str) -> Guarded {
    let cut = [line_loop(text), word_loop(text), char_loop(text)].into_iter().flatten().min();
    match cut {
        Some(c) => Guarded { text: text[..c].trim_end().to_string(), looped: true },
        None => Guarded { text: text.to_string(), looped: false },
    }
}

/// Verdict for one page or crop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Loop,
    TokenLimit,
    TooLong,
    Empty,
    /// The answer is markup (LaTeX, Markdown headings, HTML/figure blocks), not a transcription.
    Markup,
}

/// Markup that an OCR model emits instead of plain text (Jetson run 2026-11-27: a crop "raus." came back
/// as `$\text{Hakka.}$`, other crops as `<figure> ### Figure …`). Only checked for crops: whole pages may
/// legitimately contain `$` or `#`.
fn looks_like_markup(text: &str) -> bool {
    let t = text.trim();
    let latex =
        t.contains("\\text{") || t.contains("\\frac") || t.contains("\\mathrm") || (t.starts_with('$') && t.ends_with('$') && t.len() > 2);
    let html = t.contains("<figure") || t.contains("</") || t.contains("<img") || t.contains("<table");
    let markdown = t.starts_with("#") || t.contains("```") || t.contains("\n#");
    latex || html || markdown
}

/// Full check of an answer: loop, token limit (`hit_limit`), plausible length compared with the classic
/// OCR (`ocr_chars`, visible characters; crops are single lines and may only be a little longer than
/// the OCR line). Crop answers are joined into one line.
pub fn assess(raw: &str, hit_limit: bool, ocr_chars: Option<usize>, crop: bool) -> (Verdict, String) {
    let g = check(raw.trim());
    if crop && looks_like_markup(&g.text) {
        return (Verdict::Markup, g.text);
    }
    let text = if crop { g.text.split_whitespace().collect::<Vec<_>>().join(" ") } else { g.text };
    if g.looped {
        return (Verdict::Loop, text);
    }
    if hit_limit {
        return (Verdict::TokenLimit, text);
    }
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    if chars == 0 {
        return (Verdict::Empty, text);
    }
    if let Some(o) = ocr_chars {
        let too_long = if crop { chars > 3 * o + 40 } else { o > 0 && chars > (3 * o).max(o + 3000) };
        if too_long {
            return (Verdict::TooLong, text);
        }
    }
    (Verdict::Accepted, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Head of a synthetic invoice page (corpus `dense_03`, synthetic data only).
    const INVOICE_HEAD: &str = "Kundenservice: 0800 555 1519 · Mo-Fr 8-18 Uhr\n\nFjordline Logistics Ltd.\n\n\
Clara Müllenbeck\nBahnhofstraße 27a\n28195 Bremen\n\nRechnungsnummer: RE-2024-33294\nRechnungsdatum: 18.11.2021\n\
Pos. Bezeichnung Menge Einzelpreis Gesamt\n1 Wartung Heizungsanlage 1 103,59 € 103,59 €\n";

    #[test]
    fn repeated_line_until_limit() {
        // paddlevl on dense_03: the same amount line ~250 times until the token limit
        let mut t = INVOICE_HEAD.to_string();
        t.push_str("Summe netto\n");
        for _ in 0..250 {
            t.push_str("103,59 €\n");
        }
        t.push_str("103");
        let g = check(&t);
        assert!(g.looped);
        assert!(g.text.ends_with("Summe netto\n103,59 €"), "{}", &g.text[g.text.len().saturating_sub(40)..]);
    }

    #[test]
    fn repeated_number_line() {
        // qwen3.5-2B (CPU) on dense_01: the invoice number line repeated ~180 times
        let mut t = "Kranich Versicherung VVaG\nKastanienring 109b\n97070 Würzburg\n".to_string();
        for _ in 0..180 {
            t.push_str("RE-2022-10935\n");
        }
        let g = check(&t);
        assert!(g.looped);
        assert_eq!(g.text, "Kranich Versicherung VVaG\nKastanienring 109b\n97070 Würzburg\nRE-2022-10935");
    }

    #[test]
    fn repeated_sentence_in_one_paragraph() {
        // nanonets on skew_03: one sentence repeated inside a paragraph without line breaks
        let s = "Änderungen der Bankverbindung teilen Sie uns bitte spätestens bis zum 22.11.2026 mit. ";
        let t = format!("Die Versicherung gilt ab dem 1. März. {}", s.repeat(30));
        let g = check(&t);
        assert!(g.looped);
        assert_eq!(g.text, format!("Die Versicherung gilt ab dem 1. März. {}", s.trim_end()));
    }

    #[test]
    fn repeated_short_word_group() {
        // paddlevl (NAS CPU) on skew_03: "1.978,24 € von" over and over
        let t = format!("Der Selbstbehalt beträgt {}", "1.978,24 € von ".repeat(120));
        let g = check(&t);
        assert!(g.looped);
        assert_eq!(g.text, "Der Selbstbehalt beträgt 1.978,24 € von");
    }

    #[test]
    fn character_pattern_inside_a_word() {
        // granite-style loop inside one token (synthetic: "Version 4.0.0.0.0…")
        let t = format!("Projekt Industrie\nVersion 4{}\nEnde", ".0".repeat(200));
        let g = check(&t);
        assert!(g.looped);
        assert!(g.text.starts_with("Projekt Industrie\nVersion 4.0"));
        assert!(g.text.len() < 40);
    }

    #[test]
    fn legitimate_repetition_is_kept() {
        // table rows with the same structure, a markdown separator, a boilerplate line on every page,
        // a short Ja/Nein form, a few equal amounts
        let t = "| Pos | Text | Betrag |\n|---|---|---|\n| 1 | Miete | 800,00 € |\n| 2 | Nebenkosten | 120,00 € |\n\
==================================================\n\
Seite 1 von 3\nAllgemeine Geschäftsbedingungen\n\x0cSeite 2 von 3\nAllgemeine Geschäftsbedingungen\n\
Haustiere: Ja Nein Ja Nein Ja Nein\n0,00 €\n0,00 €\n0,00 €\n0,00 €\nIBAN DE58 9123 5426 3684 4696 35\n\
--------------------------------------------------------------------------------\n";
        let g = check(t);
        assert!(!g.looped, "{}", g.text);
        assert_eq!(g.text, t);
        // a header repeated between differing sections is not a loop
        let t: String = (1..=6)
            .map(|i| {
                format!(
                    "Minutes of the Annual General Meeting
{i}. Scope
Item {i}: the parties agree on clause {i}.
"
                )
            })
            .collect();
        assert!(!check(&t).looped);
    }

    #[test]
    fn assess_rules() {
        assert_eq!(assess("Bitte anrufen!", false, Some(13), true), (Verdict::Accepted, "Bitte anrufen!".into()));
        // crop: joined into one line
        assert_eq!(assess("Bitte\nanrufen!", false, Some(13), true).1, "Bitte anrufen!");
        // crop answer much longer than the OCR line → explanation/hallucination
        assert_eq!(
            assess("Auf dem Bild steht vermutlich eine Telefonnummer, aber sie ist kaum lesbar.", false, Some(5), true).0,
            Verdict::TooLong
        );
        assert_eq!(assess("   ", false, Some(5), false).0, Verdict::Empty);
        assert_eq!(assess("Text", true, None, false).0, Verdict::TokenLimit);
        // handwriting page: little OCR text, normal AI text → accepted
        let words = |n: usize| (0..n).map(|i| format!("Wort{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(assess(&words(40), false, Some(12), false).0, Verdict::Accepted);
        // page: far longer than the classic text (OCR 1000 characters, AI ~7000) → too long
        assert_eq!(assess(&words(1000), false, Some(1000), false).0, Verdict::TooLong);
        let looped = format!("Kopf\n{}", "RE-2022-10935\n".repeat(20));
        assert_eq!(assess(&looped, true, None, false).0, Verdict::Loop);
    }

    /// Regression (Jetson run 2026-11-27): markup answers for line crops were accepted.
    #[test]
    fn crop_markup_is_rejected() {
        assert_eq!(assess("$\\text{Hakka.}$", false, Some(5), true).0, Verdict::Markup);
        assert_eq!(assess("<figure> ### Figure – Handwritten", false, Some(20), true).0, Verdict::Markup);
        assert_eq!(assess("### Notizen", false, Some(8), true).0, Verdict::Markup);
        // plain text with symbols stays accepted
        assert_eq!(assess("Summe: 12,50 $ (USD) #3", false, Some(20), true).0, Verdict::Accepted);
        assert_eq!(assess("Zählerstand Wasser: 464,5 m³", false, Some(25), true).0, Verdict::Accepted);
        // whole pages are not checked for markup
        assert_eq!(assess("# Überschrift\nText", false, Some(15), false).0, Verdict::Accepted);
    }
}
