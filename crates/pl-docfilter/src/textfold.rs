//! Result-identical, faster form of Django's `UPPER(x::text) LIKE UPPER(p)` for ASCII search texts
//! (`title_content`, `content__icontains` etc., `?search=`).
//!
//! Background (docs/contract/pgvalidate.md section 9): with 100,000 documents,
//! `title_content` spends ~90 % of its time in the `UPPER()` of the database collation (ICU/libc) over ~2.4 kB
//! of content per row. `UPPER(x COLLATE "C")` only changes ASCII letters and is many times
//! cheaper.
//!
//! **Equality of the results.** For a search text `v` of pure ASCII, `UPPER(v)` in every
//! collation without Turkish/Lithuanian special rules is the ASCII uppercase image. A match of `UPPER(v)`
//! in `UPPER(x)` consists only of ASCII characters. Every character `c` of `x` is mapped by `UPPER`
//! independently (context-free, see checks below):
//! * ASCII → ASCII uppercase letter (like `COLLATE "C"`),
//! * the few non-ASCII characters whose uppercase image contains ASCII (ICU: `ß`→`SS`, `ı`→`I`,
//!   `ſ`→`S`, `ﬁ`→`FI` …, libc: only `ı`, `ſ`) → exactly this uppercase image,
//! * all other non-ASCII characters → a text without ASCII (under `"C"`: the character itself,
//!   likewise without ASCII).
//!
//! If in `UPPER(x COLLATE "C")` the special characters are replaced by their uppercase image of the
//! database collation, both texts agree in all ASCII sections and their boundaries –
//! `LIKE '%P%'`, `'P%'`, `'%P'` and `= 'P'` (P = ASCII) thus yield the same. A special character
//! whose uppercase image contains no letter of P cannot contribute to a match and is
//! not replaced (fewer `replace()` passes).
//!
//! The table of special characters is read **from the database itself** (once per process and
//! database): `upper(chr(i))` for all characters of planes 0–1 (planes 2–16 have no
//! upper/lower mapping; checked: 0 characters). The old form is switched back on if
//! * the database is not UTF-8,
//! * `upper` of an ASCII character is not the ASCII uppercase image (Turkish/Azerbaijani `i`→`İ`),
//! * `upper(E'i̇')` is not `E'İ'` (Lithuanian context rule),
//! * `upper(... COLLATE "C")` changes non-ASCII,
//! * `title`/`content` have a collation of their own on the column,
//! * the calibration fails.
//!
//! Search texts with non-ASCII characters keep the old form (`UPPER(x::text) LIKE UPPER(p)`),
//! as do search texts with more than [`MAX_REPLACES`] relevant special characters (each `replace()` is
//! another pass; e.g. `strasse` → `ß ſ ﬅ ﬆ ẗ ẚ`), because the folding is not faster there.

use sea_query::{Expr, SimpleExpr};

use crate::sql::{TextLookup, like_escape, text_lookup};

/// Maximum number of `replace()` passes up to which the folded form is faster than
/// the collation's `UPPER()` (100k documents, ICU `en-US`: 2 replacements −30 %, 6 replacements +10 %).
pub const MAX_REPLACES: usize = 4;

/// Mapping `upper()` of the database for non-ASCII characters whose uppercase image contains ASCII.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpperFold {
    specials: Vec<(char, String)>,
}

impl UpperFold {
    /// From the calibration rows `(code point, upper(chr(i)))` of the database. `None` if ASCII
    /// is not mapped as expected or a row is invalid.
    pub fn from_db_rows(rows: &[(i32, String)]) -> Option<UpperFold> {
        let mut seen_ascii = 0usize;
        let mut specials = Vec::new();
        for (cp, up) in rows {
            let c = char::from_u32(u32::try_from(*cp).ok()?)?;
            if c.is_ascii() {
                if up.len() != 1 || up.as_bytes()[0] != c.to_ascii_uppercase() as u8 {
                    return None;
                }
                seen_ascii += 1;
            } else if up.chars().any(|u| u.is_ascii()) {
                specials.push((c, up.clone()));
            }
        }
        // 1..=127 must be fully checked
        (seen_ascii == 127).then_some(UpperFold { specials })
    }

    /// Fixed table (Unicode standard, full mapping like ICU `en-US`) – for tests.
    pub fn icu_default() -> UpperFold {
        let specials = [
            ('\u{00DF}', "SS"),
            ('\u{0131}', "I"),
            ('\u{0149}', "\u{02BC}N"),
            ('\u{017F}', "S"),
            ('\u{01F0}', "J\u{030C}"),
            ('\u{1E96}', "H\u{0331}"),
            ('\u{1E97}', "T\u{0308}"),
            ('\u{1E98}', "W\u{030A}"),
            ('\u{1E99}', "Y\u{030A}"),
            ('\u{1E9A}', "A\u{02BE}"),
            ('\u{FB00}', "FF"),
            ('\u{FB01}', "FI"),
            ('\u{FB02}', "FL"),
            ('\u{FB03}', "FFI"),
            ('\u{FB04}', "FFL"),
            ('\u{FB05}', "ST"),
            ('\u{FB06}', "ST"),
        ];
        UpperFold { specials: specials.iter().map(|(c, u)| (*c, (*u).to_string())).collect() }
    }

    pub fn specials(&self) -> &[(char, String)] {
        &self.specials
    }

    /// Like [`text_lookup`], for ASCII search texts in the fast form (otherwise unchanged).
    pub fn lookup(&self, lhs: SimpleExpr, lookup: TextLookup, value: &str) -> SimpleExpr {
        self.lookup_limited(lhs, lookup, value, MAX_REPLACES)
    }

    /// Like [`Self::lookup`] with its own maximum number of replacements (tests: `usize::MAX`).
    pub fn lookup_limited(&self, lhs: SimpleExpr, lookup: TextLookup, value: &str, max_replaces: usize) -> SimpleExpr {
        if !value.is_ascii() {
            return text_lookup(lhs, lookup, value);
        }
        let (op, pattern) = match lookup {
            TextLookup::IContains => ("LIKE", format!("%{}%", like_escape(value))),
            TextLookup::IStartsWith => ("LIKE", format!("{}%", like_escape(value))),
            TextLookup::IEndsWith => ("LIKE", format!("%{}", like_escape(value))),
            TextLookup::IExact => ("=", value.to_string()),
        };
        let upper = pattern.to_ascii_uppercase();
        let relevant: Vec<&(char, String)> =
            self.specials.iter().filter(|(_, up)| up.chars().any(|u| u.is_ascii() && upper.contains(u))).collect();
        // Each replace() is another pass over the text; from MAX_REPLACES on, the
        // collation UPPER() form is equally fast or faster (measured, pgvalidate.md 9).
        if relevant.len() > max_replaces {
            return text_lookup(lhs, lookup, value);
        }
        let mut folded = Expr::cust_with_exprs(r#"UPPER(($1)::text COLLATE "C")"#, [lhs]);
        for (c, up) in relevant {
            folded = Expr::cust_with_exprs("replace($1, $2, $3)", [folded, Expr::val(c.to_string()).into(), Expr::val(up.clone()).into()]);
        }
        Expr::cust_with_exprs(format!("$1 {op} $2"), [folded, Expr::val(upper).into()])
    }
}

/// Text lookup with optional folding (`None` = Django form).
pub fn folded_lookup(fold: Option<&UpperFold>, lhs: SimpleExpr, lookup: TextLookup, value: &str) -> SimpleExpr {
    match fold {
        Some(f) => f.lookup(lhs, lookup, value),
        None => text_lookup(lhs, lookup, value),
    }
}

/// Calibration query 1: prerequisites (UTF-8, Lithuanian context rule, `"C"` ASCII only,
/// column collation default).
pub const CALIBRATE_CHECKS_SQL: &str = r#"SELECT
  pg_encoding_to_char(d.encoding) = 'UTF8'
  AND upper(E'i̇') = E'İ'
  AND upper(E'äbß' COLLATE "C") = E'äBß'
  AND COALESCE((SELECT bool_and(a.attcollation IN (0, 100)) FROM pg_attribute a
       WHERE a.attrelid = 'documents_document'::regclass AND a.attname IN ('title', 'content')), false)
FROM pg_database d WHERE d.datname = current_database()"#;

/// Calibration query 2: ASCII 1–127 and all non-ASCII characters of planes 0–1 whose
/// uppercase image contains ASCII.
pub const CALIBRATE_TABLE_SQL: &str = r#"SELECT i, upper(chr(i)) FROM generate_series(1, 131071) AS i
WHERE (i < 55296 OR i > 57343) AND (i < 128 OR upper(chr(i)) ~ '[\x01-\x7f]')
ORDER BY i"#;

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{PostgresQueryBuilder, Query};

    fn sql(e: SimpleExpr) -> String {
        Query::select().expr(e).to_string(PostgresQueryBuilder)
    }

    /// The fixed table corresponds to Rust's Unicode data (full mapping).
    #[test]
    fn icu_table_matches_unicode() {
        let mut expect = Vec::new();
        for cp in 0x80u32..=0x10FFFF {
            let Some(c) = char::from_u32(cp) else { continue };
            let up: String = c.to_uppercase().collect();
            if up.chars().any(|u| u.is_ascii()) {
                expect.push((c, up));
            }
        }
        assert_eq!(UpperFold::icu_default().specials, expect);
    }

    #[test]
    fn rows_validate_ascii() {
        let mut rows: Vec<(i32, String)> = (1..128u8).map(|b| (b as i32, (b as char).to_ascii_uppercase().to_string())).collect();
        rows.push((0xDF, "SS".into()));
        let f = UpperFold::from_db_rows(&rows).unwrap();
        assert_eq!(f.specials, vec![('\u{DF}', "SS".to_string())]);
        // Turkish: upper('i') = 'İ'
        rows[(b'i' - 1) as usize].1 = "\u{130}".into();
        assert!(UpperFold::from_db_rows(&rows).is_none());
        assert!(UpperFold::from_db_rows(&rows[..10]).is_none());
    }

    #[test]
    fn fast_form() {
        let f = UpperFold::icu_default();
        let col = || Expr::col(sea_query::Alias::new("t")).into();
        // RECHNUNG: relevant ŉ (N) and ẖ (H)
        assert_eq!(
            sql(f.lookup(col(), TextLookup::IContains, "rechnung")),
            r#"SELECT replace(replace(UPPER(("t")::text COLLATE "C"), 'ŉ', 'ʼN'), 'ẖ', 'H̱') LIKE '%RECHNUNG%'"#
        );
        // without relevant special characters
        assert_eq!(sql(f.lookup(col(), TextLookup::IExact, "42 x")), r#"SELECT UPPER(("t")::text COLLATE "C") = '42 X'"#);
        // Escape like Django
        assert_eq!(sql(f.lookup(col(), TextLookup::IStartsWith, "5%_")), r#"SELECT UPPER(("t")::text COLLATE "C") LIKE E'5\\%\\_%'"#);
        // too many relevant special characters (S, T, F, I …: ß ſ ﬅ ﬆ ẗ …) → Django form
        assert_eq!(sql(f.lookup(col(), TextLookup::IContains, "strasse")), sql(text_lookup(col(), TextLookup::IContains, "strasse")));
        // non-ASCII → Django form
        assert_eq!(sql(f.lookup(col(), TextLookup::IContains, "für")), sql(text_lookup(col(), TextLookup::IContains, "für")));
    }

    /// Model of equality: ICU uppercase image (Rust `to_uppercase`, full mapping) against the
    /// folding (ASCII uppercase image + replacement of the relevant special characters) for many texts.
    #[test]
    fn equivalence_model() {
        let f = UpperFold::icu_default();
        let fold = |x: &str, pat: &str| -> String {
            let mut s = x.to_ascii_uppercase();
            for (c, up) in f.specials() {
                if up.chars().any(|u| u.is_ascii() && pat.contains(u)) {
                    s = s.replace(*c, up);
                }
            }
            s
        };
        let texts = [
            "Straße",
            "STRASSE",
            "straſse",
            "ﬁle",
            "Rechnung",
            "rechnungß",
            "aı",
            "ǰo",
            "ẖa",
            "Ärger",
            "ärger",
            "xﬀy",
            "ﬅop",
            "ŉo",
            "ʼn",
            "a\u{0307}b",
            "ΐ",
            "ﬃx",
            "Fİle",
        ];
        let pats =
            ["STRASSE", "SS", "FILE", "FI", "RECHNUNG", "AI", "JO", "HA", "RGER", "FFY", "FY", "STOP", "ʼN", "N", "IX", "FFI", "B", "I"];
        for t in texts {
            let icu: String = t.chars().flat_map(char::to_uppercase).collect();
            for p in pats {
                if !p.is_ascii() {
                    continue;
                }
                assert_eq!(icu.contains(p), fold(t, p).contains(p), "{t:?} / {p:?}");
                assert_eq!(icu.starts_with(p), fold(t, p).starts_with(p), "{t:?} / {p:?}");
                assert_eq!(icu.ends_with(p), fold(t, p).ends_with(p), "{t:?} / {p:?}");
                assert_eq!(icu == p, fold(t, p) == p, "{t:?} / {p:?}");
            }
        }
    }
}
