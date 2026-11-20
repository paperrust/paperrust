//! PaperRust's own translations of the account pages and mails (no texts from paperless-ngx
//! or django-allauth).
//!
//! Catalog: `crates/pl-accounts/i18n/<locale>.json`, same languages, file names, key form
//! (`area.name`) and placeholders (`{$NAME}`) as the UI in `frontend-light/i18n/`. Kept separate
//! from the UI so that its bundle does not carry login texts along; completeness is checked by
//! the test `catalog_complete` (like `frontend-light/tools/i18n-check.mjs` for the UI).
//!
//! Language: the Django language code of the request (`de-de`, `pt-br`, …, from `pl_core::locale`) is
//! mapped to a catalog via the primary language (`de-*` → `de-DE`, `pt-*` → `pt-BR`); everything
//! else gets English. If a key is missing in a language: English, then the key.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Languages with their own catalog (like `SUPPORTED_LOCALES` of the UI), English first.
pub const LOCALES: &[&str] = &["en-US", "de-DE", "fr-FR", "it-IT", "es-ES", "nl-NL", "pt-BR", "pl-PL", "cs-CZ", "sv-SE"];

const SOURCES: &[(&str, &str)] = &[
    ("en-US", include_str!("../i18n/en-US.json")),
    ("de-DE", include_str!("../i18n/de-DE.json")),
    ("fr-FR", include_str!("../i18n/fr-FR.json")),
    ("it-IT", include_str!("../i18n/it-IT.json")),
    ("es-ES", include_str!("../i18n/es-ES.json")),
    ("nl-NL", include_str!("../i18n/nl-NL.json")),
    ("pt-BR", include_str!("../i18n/pt-BR.json")),
    ("pl-PL", include_str!("../i18n/pl-PL.json")),
    ("cs-CZ", include_str!("../i18n/cs-CZ.json")),
    ("sv-SE", include_str!("../i18n/sv-SE.json")),
];

type Catalog = HashMap<String, String>;

static CATALOGS: LazyLock<HashMap<&'static str, Catalog>> = LazyLock::new(|| {
    let mut out = HashMap::new();
    for (locale, src) in SOURCES {
        match serde_json::from_str::<Catalog>(src) {
            Ok(c) => {
                out.insert(*locale, c);
            }
            Err(e) => tracing::error!(target: "pl_accounts", "i18n catalog {locale}: {e}"),
        }
    }
    out
});

/// Catalog language for a Django language code (`de-de`, `pt-pt`, `en-gb`, …).
pub fn locale_for(lang: &str) -> &'static str {
    let primary = lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    LOCALES.iter().copied().find(|l| l.split('-').next().is_some_and(|p| p.eq_ignore_ascii_case(&primary))).unwrap_or("en-US")
}

/// Value for `<html lang>` (`de`, `pt-BR`, `en`).
pub fn html_lang(lang: &str) -> &'static str {
    match locale_for(lang) {
        "pt-BR" => "pt-BR",
        l => l.split('-').next().unwrap_or("en"),
    }
}

/// Text for `key` in the request's language (fallback English, then the key itself).
pub fn t<'a>(lang: &str, key: &'a str) -> &'a str {
    let get = |loc: &str| CATALOGS.get(loc).and_then(|c| c.get(key)).map(String::as_str);
    get(locale_for(lang)).or_else(|| get("en-US")).unwrap_or(key)
}

/// Fills in `{$NAME}` placeholders (values unchanged, no escaping).
pub fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{${k}}}"), v);
    }
    out
}

/// `t` + `fill`.
pub fn tf(lang: &str, key: &str, args: &[(&str, &str)]) -> String {
    fill(t(lang, key), args)
}

/// Shorthand `tr!(lang, "auth.x")` for `i18n::t`.
#[macro_export]
macro_rules! tr {
    ($lang:expr, $key:literal) => {
        $crate::i18n::t($lang, $key)
    };
}

#[cfg(test)]
pub(crate) fn catalog(locale: &str) -> Option<&'static Catalog> {
    CATALOGS.get(locale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Placeholders of a text (`{$NAME}`), sorted.
    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(p) = rest.find("{$") {
            let tail = &rest[p + 2..];
            match tail.find('}') {
                Some(e) => {
                    out.insert(tail[..e].to_string());
                    rest = &tail[e + 1..];
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn catalog_complete() {
        let en = catalog("en-US").expect("en-US parses");
        assert!(!en.is_empty());
        let key_ok = |k: &str| k.starts_with("auth.") && k.split('.').count() >= 2 && k.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        for loc in LOCALES {
            let c = catalog(loc).unwrap_or_else(|| panic!("{loc} parses"));
            let a: BTreeSet<&String> = en.keys().collect();
            let b: BTreeSet<&String> = c.keys().collect();
            assert_eq!(a.difference(&b).collect::<Vec<_>>(), Vec::<&&String>::new(), "{loc}: missing keys");
            assert_eq!(b.difference(&a).collect::<Vec<_>>(), Vec::<&&String>::new(), "{loc}: extra keys");
            for (k, v) in c {
                assert!(key_ok(k), "{loc}: bad key {k}");
                assert!(!v.trim().is_empty(), "{loc}: empty {k}");
                assert_eq!(placeholders(v), placeholders(&en[k]), "{loc}: placeholders of {k}");
                assert!(!v.contains('<') && !v.contains('>'), "{loc}: no HTML in {k}");
            }
        }
        // All catalogs are also a supported language in the UI.
        assert_eq!(LOCALES.len(), SOURCES.len());
    }

    /// No text mentions paperless-ngx/allauth – except the license notice in the footer.
    #[test]
    fn catalog_has_no_upstream_names() {
        for loc in LOCALES {
            for (k, v) in catalog(loc).into_iter().flatten() {
                let low = v.to_lowercase();
                if k == "auth.footer.license" {
                    assert!(v.contains("paperless-ngx") && v.contains("GPL-3.0"), "{loc}: license notice changed");
                    continue;
                }
                assert!(!low.contains("paperless") && !low.contains("allauth") && !low.contains("ngx"), "{loc}: {k} = {v}");
            }
        }
    }

    #[test]
    fn language_mapping() {
        assert_eq!(locale_for("de-de"), "de-DE");
        assert_eq!(locale_for("pt-pt"), "pt-BR");
        assert_eq!(locale_for("en-gb"), "en-US");
        assert_eq!(locale_for("ja-jp"), "en-US");
        assert_eq!(locale_for(""), "en-US");
        assert_eq!(html_lang("pt-br"), "pt-BR");
        assert_eq!(html_lang("sv-se"), "sv");
        assert_eq!(html_lang("ja-jp"), "en");
    }

    #[test]
    fn lookup_and_fallback() {
        assert_eq!(t("de-de", "auth.field.username"), "Benutzername");
        assert_eq!(t("en-us", "auth.field.username"), "Username");
        assert_eq!(t("ja-jp", "auth.field.username"), "Username");
        assert_eq!(t("de-de", "auth.does_not_exist"), "auth.does_not_exist");
        assert_eq!(tf("en-us", "auth.msg.signed_in", &[("NAME", "alice")]), "Signed in as alice.");
        assert_eq!(fill("{$A}/{$B}/{$A}", &[("A", "1"), ("B", "2")]), "1/2/1");
    }
}
