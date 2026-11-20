//! Strong name normalization (PaperRust extension, no upstream counterpart).
//!
//! Goal: "Swisscom (Schweiz) AG", "SWISSCOM AG", "Swisscom" → same core `swisscom`;
//! "Müller & Söhne GmbH" ≈ "Mueller und Soehne"; "Crédit Agricole S.A." ≈ "Credit Agricole".
//!
//! Steps ([`normalize`]):
//! 1. Unicode NFKD, remove combining characters (accents), resolve ligatures (ﬁ → fi).
//! 2. Lowercase; `ß` → `ss`, `æ` → `ae`, `ø` → `o`, `œ` → `oe`, `ł` → `l`, `đ` → `d`.
//! 3. German umlaut transliteration (`ä` → `ae` …) as a **second variant** (`folded_de`), so that
//!    "Müller" matches both "Muller" and "Mueller".
//! 4. Punctuation → space; `&`/`+` → "und" token (removed as a filler word);
//!    dots in abbreviations are contracted ("S.A." → "sa", "e.V." → "ev").
//! 5. Tokens: remove filler words (DE/FR/IT/EN); for [`NameKind::Organisation`] additionally
//!    legal forms (GmbH, AG, SA, Sàrl, Ltd, Inc, …) and country/region suffixes
//!    ("Schweiz", "Suisse", "Svizzera", "Switzerland", "Deutschland", …) – but never so that
//!    the core becomes empty (a tag "AG" stays "ag").
//! 6. `key` = core tokens in original order, `sorted_key` = sorted (token-set comparison).

use unicode_normalization::UnicodeNormalization;

/// What a name stands for – controls which word lists apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    /// Correspondents: legal forms and country suffixes are removed.
    Organisation,
    /// Tags, document types, custom fields: filler words only.
    Label,
    /// Storage paths: `/` separates levels, otherwise like `Label`.
    Path,
}

/// Legal forms (after contracting the dots, without spaces), multilingual.
pub const LEGAL_FORMS: &[&str] = &[
    // DE/AT/CH
    "gmbh", "ag", "kg", "ohg", "gbr", "ug", "eg", "ev", "mbh", "kgaa", "se", "co", "cokg", "gmbhcokg",
    "haftungsbeschrankt", "stiftung", "genossenschaft", "verein", "einzelfirma", "einzelunternehmen",
    "aktiengesellschaft", "gesellschaft", "holding",
    // FR/CH/BE/LU
    "sa", "sarl", "sas", "sasu", "eurl", "snc", "sca", "scs", "sci", "sprl", "srl", "cie",
    "societeanonyme", "societe", "association", "fondation", "cooperative",
    // IT/CH
    "spa", "sagl", "snc", "sas", "srls", "societa", "fondazione", "cooperativa",
    // EN
    "ltd", "limited", "inc", "incorporated", "corp", "corporation", "company", "plc", "llc", "llp",
    "lp", "pty", "pvt", "group", "grp",
    // NL/Nordics/andere
    "bv", "nv", "vof", "oy", "oyj", "ab", "as", "asa", "aps", "sl", "sau", "lda", "kft", "spzoo", "sro",
];

/// Country/region suffixes that often appear in organization names.
pub const REGION_QUALIFIERS: &[&str] = &[
    "schweiz", "suisse", "svizzera", "svizra", "switzerland", "swiss", "ch",
    "deutschland", "germany", "allemagne", "germania", "de",
    "osterreich", "oesterreich", "austria", "autriche", "at",
    "france", "frankreich", "francia", "fr",
    "italia", "italien", "italy", "italie", "it",
    "liechtenstein", "li", "europe", "europa", "international", "intl", "worldwide", "global",
];

/// Filler words (articles, conjunctions, prepositions), DE/FR/IT/EN.
pub const FILLER_WORDS: &[&str] = &[
    // DE
    "der", "die", "das", "des", "dem", "den", "ein", "eine", "einer", "und", "u", "fur", "fuer", "von",
    "vom", "zu", "zur", "zum", "im", "in", "am", "an", "auf", "mit",
    // FR
    "le", "la", "les", "l", "un", "une", "et", "de", "du", "d", "pour", "au", "aux", "en", "sur",
    // IT
    "il", "lo", "gli", "i", "uno", "una", "e", "ed", "di", "del", "della", "dello", "dei", "degli",
    "delle", "per", "al", "alla", "nel", "nella",
    // EN
    "the", "a", "an", "and", "of", "for", "to", "on", "at", "by",
];

/// Result of normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    /// All tokens after steps 1–4 (without removing words).
    pub tokens_all: Vec<String>,
    /// Core tokens (filler words/legal forms/countries removed, never empty if `tokens_all` is not empty).
    pub tokens: Vec<String>,
    /// Core as a string (tokens with spaces, original order).
    pub key: String,
    /// Core, tokens sorted and deduplicated.
    pub sorted_key: String,
    /// Core without spaces ("swiss com" == "swisscom").
    pub compact: String,
    /// Removed legal forms (for explanations).
    pub removed_legal: Vec<String>,
    /// Removed country suffixes.
    pub removed_region: Vec<String>,
    /// Numbers in the name (for number incompatibility "Steuern 2024" ≠ "Steuern 2025").
    pub numbers: Vec<String>,
}

fn fold_char(c: char, german: bool, out: &mut String) {
    match c {
        'ß' => out.push_str("ss"),
        'æ' | 'Æ' => out.push_str("ae"),
        'œ' | 'Œ' => out.push_str("oe"),
        'ø' | 'Ø' => out.push('o'),
        'ł' | 'Ł' => out.push('l'),
        'đ' | 'Đ' => out.push('d'),
        'þ' | 'Þ' => out.push_str("th"),
        'ä' | 'Ä' if german => out.push_str("ae"),
        'ö' | 'Ö' if german => out.push_str("oe"),
        'ü' | 'Ü' if german => out.push_str("ue"),
        _ => {
            for d in c.to_string().nfkd() {
                if !unicode_normalization::char::is_combining_mark(d) {
                    for l in d.to_lowercase() {
                        out.push(l);
                    }
                }
            }
        }
    }
}

/// Remove accents + lowercase. `german` = umlauts as ae/oe/ue.
pub fn fold(s: &str, german: bool) -> String {
    let mut out = String::with_capacity(s.len());
    // NFC first, so that "u + ¨" (decomposed) is treated like "ü"
    for c in s.nfc() {
        fold_char(c, german, &mut out);
    }
    out
}

/// Splits a folded string into tokens. Dots between single letters are
/// contracted ("s.a." → "sa", "e.v." → "ev"), `&`/`+` become "und".
fn tokenize(folded: &str, kind: NameKind) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    // raw segments at whitespace and separators
    let mut raw = String::new();
    for c in folded.chars() {
        let sep = c.is_whitespace()
            || matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | '´'
                | '/' | '\\' | '|' | '-' | '–' | '—' | '_' | '!' | '?' | '*' | '#' | '~' | '<' | '>'
                | '«' | '»' | '„' | '“' | '”' | '‘' | '’' | '=' | '@' | '%')
            || (kind == NameKind::Path && c == '/');
        if c == '&' || c == '+' {
            if !raw.is_empty() {
                tokens.push(std::mem::take(&mut raw));
            }
            tokens.push("und".into());
            continue;
        }
        if sep {
            if !raw.is_empty() {
                tokens.push(std::mem::take(&mut raw));
            }
        } else {
            raw.push(c);
        }
    }
    if !raw.is_empty() {
        tokens.push(raw);
    }
    // Punkte: "s.a." → "sa"; "www.example.com" → "www", "example", "com"; "1.5" bleibt "1.5"→"15"
    let mut out = Vec::new();
    for t in tokens {
        if t.contains('.') {
            let parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
            let all_short = parts.iter().all(|p| p.chars().count() <= 2);
            let all_digits = parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()));
            if all_short || all_digits {
                let joined: String = parts.concat();
                if !joined.is_empty() {
                    out.push(joined);
                }
            } else {
                out.extend(parts.into_iter().map(str::to_string));
            }
        } else {
            out.push(t);
        }
    }
    out.into_iter()
        .map(|t| t.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|t| !t.is_empty())
        .collect()
}

fn is_number(t: &str) -> bool {
    t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit())
}

/// Detects multi-part legal forms ("gmbh co kg", "s a", "sp z o o", "e v").
fn merge_multi_token_legal(tokens: &[String]) -> Vec<String> {
    const SEQS: &[(&[&str], &str)] = &[
        (&["gmbh", "und", "co", "kg"], "gmbhcokg"),
        (&["gmbh", "co", "kg"], "gmbhcokg"),
        (&["und", "co", "kg"], "cokg"),
        (&["co", "kg"], "cokg"),
        (&["und", "cie"], "cie"),
        (&["und", "co"], "co"),
        (&["sp", "z", "oo"], "spzoo"),
        (&["s", "a"], "sa"),
        (&["s", "r", "l"], "srl"),
        (&["e", "v"], "ev"),
        (&["s", "p", "a"], "spa"),
    ];
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < tokens.len() {
        for (seq, merged) in SEQS {
            if i + seq.len() <= tokens.len() && tokens[i..i + seq.len()].iter().zip(seq.iter()).all(|(a, b)| a == b) {
                out.push((*merged).to_string());
                i += seq.len();
                continue 'outer;
            }
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out
}

/// Normalizes a name. `german` = umlauts as ae/oe/ue (second variant).
pub fn normalize_with(name: &str, kind: NameKind, german: bool) -> Normalized {
    let folded = fold(name, german);
    let tokens_all = merge_multi_token_legal(&tokenize(&folded, kind));
    let numbers: Vec<String> = tokens_all.iter().filter(|t| is_number(t)).cloned().collect();

    let mut removed_legal = Vec::new();
    let mut removed_region = Vec::new();
    let mut core: Vec<String> = Vec::new();
    let n = tokens_all.len();
    for (idx, t) in tokens_all.iter().enumerate() {
        let is_filler = FILLER_WORDS.contains(&t.as_str());
        let org = kind == NameKind::Organisation;
        // Legal forms only from the 2nd token on (a name "AG Müller" does not keep "ag" – but
        // "SA" as the only word is kept, see below).
        let is_legal = org && LEGAL_FORMS.contains(&t.as_str()) && (idx > 0 || n > 1);
        let is_region = org && idx > 0 && REGION_QUALIFIERS.contains(&t.as_str());
        if is_legal {
            removed_legal.push(t.clone());
        } else if is_region {
            removed_region.push(t.clone());
        } else if is_filler && n > 1 {
            // filler word
        } else {
            core.push(t.clone());
        }
    }
    if core.is_empty() {
        // Never empty: then the tokens without filler words, otherwise everything.
        core = tokens_all.iter().filter(|t| !FILLER_WORDS.contains(&t.as_str())).cloned().collect();
        if core.is_empty() {
            core = tokens_all.clone();
        }
        removed_legal.retain(|t| !core.contains(t));
        removed_region.retain(|t| !core.contains(t));
    }
    let key = core.join(" ");
    let mut sorted = core.clone();
    sorted.sort();
    sorted.dedup();
    let sorted_key = sorted.join(" ");
    let compact = core.concat();
    Normalized { tokens_all, tokens: core, key, sorted_key, compact, removed_legal, removed_region, numbers }
}

/// Normalizes with accent folding (ä → a).
pub fn normalize(name: &str, kind: NameKind) -> Normalized {
    normalize_with(name, kind, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str, k: NameKind) -> String {
        normalize(s, k).key
    }

    #[test]
    fn legal_forms_multilingual() {
        let o = NameKind::Organisation;
        assert_eq!(key("Swisscom (Schweiz) AG", o), "swisscom");
        assert_eq!(key("SWISSCOM AG", o), "swisscom");
        assert_eq!(key("Crédit Agricole S.A.", o), "credit agricole");
        assert_eq!(key("Crédit Agricole SA", o), "credit agricole");
        assert_eq!(key("Boulangerie Dupont Sàrl", o), "boulangerie dupont");
        assert_eq!(key("Ferrari S.p.A.", o), "ferrari");
        assert_eq!(key("Rossi Sagl", o), "rossi");
        assert_eq!(key("Acme Ltd.", o), "acme");
        assert_eq!(key("Acme, Inc.", o), "acme");
        assert_eq!(key("Müller GmbH & Co. KG", o), "muller");
        assert_eq!(key("Sportverein Musterhausen e.V.", o), "sportverein musterhausen");
        assert_eq!(key("Die Post AG", o), "post");
        assert_eq!(key("La Poste Suisse SA", o), "poste");
    }

    #[test]
    fn never_empty() {
        assert_eq!(key("AG", NameKind::Organisation), "ag");
        assert_eq!(key("SA", NameKind::Organisation), "sa");
        assert_eq!(key("The", NameKind::Label), "the");
        assert_eq!(key("", NameKind::Label), "");
        assert_eq!(key("...", NameKind::Label), "");
    }

    #[test]
    fn umlauts_and_accents() {
        assert_eq!(fold("Müller", false), "muller");
        assert_eq!(fold("Müller", true), "mueller");
        assert_eq!(fold("Straße", false), "strasse");
        assert_eq!(fold("Ærø", false), "aero");
        assert_eq!(fold("ﬁnance", false), "finance");
        // decomposed ü (u + U+0308)
        assert_eq!(fold("Mu\u{308}ller", true), "mueller");
        assert_eq!(fold("École", false), "ecole");
    }

    #[test]
    fn labels_keep_legal_words() {
        // For tags, "AG" is not a legal-form suffix.
        assert_eq!(key("Steuern AG", NameKind::Label), "steuern ag");
        assert_eq!(key("Rechnung für die Steuer", NameKind::Label), "rechnung steuer");
        assert_eq!(key("Finanzen/Rechnungen", NameKind::Path), "finanzen rechnungen");
    }

    #[test]
    fn numbers_and_sorted() {
        let n = normalize("Steuern 2024", NameKind::Label);
        assert_eq!(n.numbers, vec!["2024"]);
        assert_eq!(normalize("Bank Kantonal", NameKind::Label).sorted_key, "bank kantonal");
        assert_eq!(normalize("Kantonal Bank", NameKind::Label).sorted_key, "bank kantonal");
        assert_eq!(normalize("Swiss Com", NameKind::Label).compact, "swisscom");
    }
}
