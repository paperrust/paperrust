//! Selection of the OCR engine (`PAPERRUST_OCR_ENGINE`) and language mapping for operating-system
//! engines.
//!
//! The default is and remains Tesseract ([`EngineChoice::Tesseract`]); without a setting nothing
//! changes. The other engines only swap the recognizer: words + boxes + page text go through
//! the same path (invisible text layer in [`crate::pdfwrite`], sidecar text, PDF/A stamp).
//!
//! | Value | Behavior |
//! |---|---|
//! | `tesseract` (default) | Tesseract (library or program, see [`crate::engine::TesseractCli`]). |
//! | `windows` | `Windows.Media.Ocr` ([`crate::winocr`]). If Windows OCR is missing entirely (other system, Windows N without media features …), warning and Tesseract. Missing language packs report themselves per document with a clear error message. |
//! | `macos` | Reserved for Apple Vision ([`crate::macocr`], not implemented yet) → warning and Tesseract. |
//! | `auto` | Windows OCR if present **and** all languages from `PAPERLESS_OCR_LANGUAGE` are covered; otherwise Tesseract; without Tesseract, Windows OCR (if present). |

use crate::engine::{OcrEngine, TesseractCli};
use crate::{OcrError, Result};

/// Value of `PAPERRUST_OCR_ENGINE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineChoice {
    #[default]
    Tesseract,
    Windows,
    Macos,
    Auto,
}

impl EngineChoice {
    pub fn parse(v: &str) -> Result<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "tesseract" => Ok(Self::Tesseract),
            "windows" | "windows-ocr" | "winrt" => Ok(Self::Windows),
            "macos" | "vision" => Ok(Self::Macos),
            "auto" => Ok(Self::Auto),
            other => {
                Err(OcrError::Config(format!("PAPERRUST_OCR_ENGINE: invalid value {other:?} (allowed: tesseract, windows, macos, auto)")))
            }
        }
    }

    /// `PAPERRUST_OCR_ENGINE`, otherwise `PAPERLESS_OCR_ENGINE`; invalid → warning + Tesseract.
    pub fn from_env() -> Self {
        let v = pl_envlive::var("PAPERRUST_OCR_ENGINE").or_else(|| pl_envlive::var("PAPERLESS_OCR_ENGINE")).unwrap_or_default();
        Self::parse(&v).unwrap_or_else(|e| {
            tracing::warn!("{e}; using tesseract");
            Self::Tesseract
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tesseract => "tesseract",
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Auto => "auto",
        }
    }
}

/// Which engine was chosen (for log/status display).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    Tesseract,
    Windows,
    None,
}

/// Pure decision logic (testable without engines).
///
/// `windows_langs_ok`: `None` = Windows OCR not present, `Some(ok)` = present, `ok` = all
/// requested languages covered.
pub fn decide(choice: EngineChoice, tesseract_available: bool, windows_langs_ok: Option<bool>) -> Selected {
    let tess = if tesseract_available { Selected::Tesseract } else { Selected::None };
    match (choice, windows_langs_ok) {
        (EngineChoice::Tesseract | EngineChoice::Macos, _) => tess,
        (EngineChoice::Windows, Some(_)) => Selected::Windows,
        (EngineChoice::Windows, None) => tess,
        (EngineChoice::Auto, Some(true)) => Selected::Windows,
        (EngineChoice::Auto, Some(false)) if !tesseract_available => Selected::Windows,
        (EngineChoice::Auto, _) => tess,
    }
}

/// Engine for page OCR per `PAPERRUST_OCR_ENGINE` (entry point for the caller, e.g.
/// `pl-consumer`). `tesseract`: the previously used, already checked Tesseract engine (or
/// `None`); `languages`: `PAPERLESS_OCR_LANGUAGE` (for `auto`).
///
/// Without a setting exactly the previous behavior: `tesseract` as the engine.
pub fn engine_from_env(tesseract: Option<TesseractCli>, languages: &str) -> Option<Box<dyn OcrEngine>> {
    select_engine(EngineChoice::from_env(), tesseract, languages)
}

/// Like [`engine_from_env`] with an explicit choice.
pub fn select_engine(choice: EngineChoice, tesseract: Option<TesseractCli>, languages: &str) -> Option<Box<dyn OcrEngine>> {
    if choice == EngineChoice::Tesseract {
        // Default path: touch nothing else (no WinRT initialization).
        return tesseract.map(|t| Box::new(t) as Box<dyn OcrEngine>);
    }
    if choice == EngineChoice::Macos {
        once_warn("macos", &crate::macocr::unavailable_reason());
        return tesseract.map(|t| Box::new(t) as Box<dyn OcrEngine>);
    }
    let win = windows_engine(tesseract.clone());
    let langs_ok = win.as_ref().map(|w| w.check_languages(languages).is_ok());
    if choice == EngineChoice::Windows && langs_ok.is_none() {
        once_warn(
            "windows-missing",
            "PAPERRUST_OCR_ENGINE=windows, but Windows OCR (Windows.Media.Ocr) is not available here; using tesseract",
        );
    }
    match decide(choice, tesseract.is_some(), langs_ok) {
        Selected::Windows => win,
        Selected::Tesseract => tesseract.map(|t| Box::new(t) as Box<dyn OcrEngine>),
        Selected::None => None,
    }
}

#[cfg(windows)]
fn windows_engine(osd: Option<TesseractCli>) -> Option<Box<dyn OcrEngine>> {
    match crate::winocr::WindowsOcr::new(osd) {
        Ok(w) => Some(Box::new(w)),
        Err(e) => {
            tracing::debug!("Windows OCR not available: {e}");
            None
        }
    }
}

#[cfg(not(windows))]
fn windows_engine(_osd: Option<TesseractCli>) -> Option<Box<dyn OcrEngine>> {
    None
}

fn once_warn(key: &'static str, msg: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut g = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if g.insert(key) {
        tracing::warn!("{msg}");
    }
}

// ---------------------------------------------------------------------------------------------
// Languages: Tesseract codes ↔ BCP-47 tags of the operating-system engines
// ---------------------------------------------------------------------------------------------

/// Script of a language (one model recognizes other languages of the same script reasonably well).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Latin,
    Cyrillic,
    Greek,
    Arabic,
    Hans,
    Hant,
    Japanese,
    Korean,
}

/// Tesseract code → (preferred BCP-47 tag, comparison key, script). Only languages for which
/// Windows OCR offers language packs (as of Windows 10/11).
const LANGS: &[(&str, &str, &str, Script)] = &[
    ("eng", "en-US", "en", Script::Latin),
    ("deu", "de-DE", "de", Script::Latin),
    ("fra", "fr-FR", "fr", Script::Latin),
    ("ita", "it-IT", "it", Script::Latin),
    ("spa", "es-ES", "es", Script::Latin),
    ("por", "pt-PT", "pt", Script::Latin),
    ("nld", "nl-NL", "nl", Script::Latin),
    ("dan", "da-DK", "da", Script::Latin),
    ("swe", "sv-SE", "sv", Script::Latin),
    ("nor", "nb-NO", "nb", Script::Latin),
    ("fin", "fi-FI", "fi", Script::Latin),
    ("pol", "pl-PL", "pl", Script::Latin),
    ("ces", "cs-CZ", "cs", Script::Latin),
    ("slk", "sk-SK", "sk", Script::Latin),
    ("slv", "sl-SI", "sl", Script::Latin),
    ("hrv", "hr-HR", "hr", Script::Latin),
    ("bos", "bs-Latn-BA", "bs", Script::Latin),
    ("srp_latn", "sr-Latn-RS", "sr-latn", Script::Latin),
    ("hun", "hu-HU", "hu", Script::Latin),
    ("ron", "ro-RO", "ro", Script::Latin),
    ("tur", "tr-TR", "tr", Script::Latin),
    ("srp", "sr-Cyrl-RS", "sr-cyrl", Script::Cyrillic),
    ("rus", "ru-RU", "ru", Script::Cyrillic),
    ("ell", "el-GR", "el", Script::Greek),
    ("ara", "ar-SA", "ar", Script::Arabic),
    ("chi_sim", "zh-Hans-CN", "zh-hans", Script::Hans),
    ("chi_tra", "zh-Hant-TW", "zh-hant", Script::Hant),
    ("jpn", "ja-JP", "ja", Script::Japanese),
    ("kor", "ko-KR", "ko", Script::Korean),
];

/// Preferred BCP-47 tag for a Tesseract code (`deu` → `de-DE`).
pub fn bcp47_for(tess: &str) -> Option<&'static str> {
    LANGS.iter().find(|l| l.0 == tess).map(|l| l.1)
}

/// Script of a Tesseract code.
pub fn script_of(tess: &str) -> Option<Script> {
    LANGS.iter().find(|l| l.0 == tess).map(|l| l.3)
}

/// Comparison key of a BCP-47 tag: main language, with script for `zh`/`sr`
/// (`de-CH` → `de`, `zh-CN` → `zh-hans`, `sr-Latn-RS` → `sr-latn`).
pub fn lang_key(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let mut parts = lower.split(['-', '_']);
    let primary = parts.next().unwrap_or("").to_string();
    let rest: Vec<&str> = parts.collect();
    match primary.as_str() {
        "zh" => {
            if rest.iter().any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo")) {
                "zh-hant".into()
            } else {
                "zh-hans".into()
            }
        }
        "sr" => {
            if rest.contains(&"latn") {
                "sr-latn".into()
            } else {
                "sr-cyrl".into()
            }
        }
        "no" | "nn" => "nb".into(),
        _ => primary,
    }
}

/// Tesseract code for an installed BCP-47 tag (`de-DE` → `deu`).
pub fn tess_for_tag(tag: &str) -> Option<&'static str> {
    let k = lang_key(tag);
    LANGS.iter().find(|l| l.2 == k).map(|l| l.0)
}

/// Result of the language mapping for an engine with exactly one language model per recognition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangPlan {
    /// Installed tag used for recognition (e.g. `de-DE`).
    pub tag: String,
    /// Corresponding Tesseract code (first requested language with a language pack).
    pub primary: String,
    /// Requested languages without their own pack that are also recognized via the model of the same script
    /// (e.g. `eng` via `de-DE`); special characters of these languages may be worse.
    pub substituted: Vec<String>,
}

/// Maps `requested` (`deu+eng`) to the installed languages (`installed`: BCP-47 tags).
///
/// Windows OCR recognizes with exactly one language. Rule: model = first requested language with an
/// installed pack. Further requested languages without a pack are allowed if they have the same
/// script (list in [`LangPlan::substituted`]); otherwise an error. Unknown codes and "not a
/// single pack" are errors with an installation hint.
pub fn plan_languages(requested: &str, installed: &[String]) -> std::result::Result<LangPlan, String> {
    let codes: Vec<&str> = requested.split('+').map(str::trim).filter(|c| !c.is_empty()).collect();
    if codes.is_empty() {
        return Err("no OCR language requested".into());
    }
    let unknown: Vec<&str> = codes.iter().copied().filter(|c| bcp47_for(c).is_none()).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "Windows OCR does not support the following requested languages: {} (use PAPERRUST_OCR_ENGINE=tesseract)",
            unknown.join(", ")
        ));
    }
    let installed_tag = |code: &str| -> Option<&String> {
        let key = LANGS.iter().find(|l| l.0 == code).map(|l| l.2)?;
        installed.iter().find(|t| lang_key(t) == key)
    };
    let Some((primary, tag)) = codes.iter().find_map(|c| installed_tag(c).map(|t| (*c, t.clone()))) else {
        return Err(format!(
            "Windows OCR has no language pack for the requested languages: {}. Installed: {}. {}",
            codes.iter().map(|c| format!("{c} ({})", bcp47_for(c).unwrap_or("?"))).collect::<Vec<_>>().join(", "),
            if installed.is_empty() { "none".to_string() } else { installed.join(", ") },
            install_hint(codes[0])
        ));
    };
    let pscript = script_of(primary);
    let mut substituted = Vec::new();
    let mut missing = Vec::new();
    for c in &codes {
        if *c == primary || installed_tag(c).is_some() {
            continue;
        }
        if script_of(c) == pscript {
            substituted.push((*c).to_string());
        } else {
            missing.push(*c);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "Windows OCR has no language pack for: {} (different script than {tag}). {}",
            missing.iter().map(|c| format!("{c} ({})", bcp47_for(c).unwrap_or("?"))).collect::<Vec<_>>().join(", "),
            install_hint(missing[0])
        ));
    }
    Ok(LangPlan { tag, primary: primary.to_string(), substituted })
}

/// Installation hint for the Windows language pack of a Tesseract code.
pub fn install_hint(tess: &str) -> String {
    let tag = bcp47_for(tess).unwrap_or("xx-XX");
    format!(
        "Install the language in Windows Settings > Time & language > Language & region (optional feature \"Optical character recognition\"), \
         or as administrator: Add-WindowsCapability -Online -Name \"Language.OCR~~~{tag}~0.0.1.0\""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_choice() {
        assert_eq!(EngineChoice::parse("").unwrap(), EngineChoice::Tesseract);
        assert_eq!(EngineChoice::parse("Tesseract").unwrap(), EngineChoice::Tesseract);
        assert_eq!(EngineChoice::parse(" windows ").unwrap(), EngineChoice::Windows);
        assert_eq!(EngineChoice::parse("AUTO").unwrap(), EngineChoice::Auto);
        assert_eq!(EngineChoice::parse("vision").unwrap(), EngineChoice::Macos);
        assert!(matches!(EngineChoice::parse("easyocr"), Err(OcrError::Config(_))));
        assert_eq!(EngineChoice::default(), EngineChoice::Tesseract);
    }

    #[test]
    fn decide_matrix() {
        use EngineChoice::*;
        use Selected as S;
        // Default: never anything other than Tesseract.
        for w in [None, Some(true), Some(false)] {
            assert_eq!(decide(Tesseract, true, w), S::Tesseract);
            assert_eq!(decide(Tesseract, false, w), S::None);
            assert_eq!(decide(Macos, true, w), S::Tesseract);
        }
        assert_eq!(decide(Windows, true, Some(true)), S::Windows);
        assert_eq!(decide(Windows, true, Some(false)), S::Windows); // language errors visible per document
        assert_eq!(decide(Windows, true, None), S::Tesseract);
        assert_eq!(decide(Windows, false, None), S::None);
        assert_eq!(decide(Auto, true, Some(true)), S::Windows);
        assert_eq!(decide(Auto, true, Some(false)), S::Tesseract);
        assert_eq!(decide(Auto, false, Some(false)), S::Windows);
        assert_eq!(decide(Auto, true, None), S::Tesseract);
        assert_eq!(decide(Auto, false, None), S::None);
    }

    #[test]
    fn tesseract_choice_passes_engine_through() {
        let t = TesseractCli::new("does-not-matter");
        let e = select_engine(EngineChoice::Tesseract, Some(t), "deu+eng").unwrap();
        assert!(e.name().starts_with("tesseract"), "{}", e.name());
        assert!(select_engine(EngineChoice::Tesseract, None, "eng").is_none());
        assert!(select_engine(EngineChoice::Macos, None, "eng").is_none());
    }

    #[test]
    fn keys_and_mapping() {
        assert_eq!(lang_key("de-DE"), "de");
        assert_eq!(lang_key("de-CH"), "de");
        assert_eq!(lang_key("zh-CN"), "zh-hans");
        assert_eq!(lang_key("zh-Hant-TW"), "zh-hant");
        assert_eq!(lang_key("zh-HK"), "zh-hant");
        assert_eq!(lang_key("sr-Latn-RS"), "sr-latn");
        assert_eq!(lang_key("sr-Cyrl-RS"), "sr-cyrl");
        assert_eq!(lang_key("nb-NO"), "nb");
        assert_eq!(tess_for_tag("en-GB"), Some("eng"));
        assert_eq!(tess_for_tag("de-AT"), Some("deu"));
        assert_eq!(tess_for_tag("zh-TW"), Some("chi_tra"));
        assert_eq!(tess_for_tag("xx-YY"), None);
        assert_eq!(bcp47_for("deu"), Some("de-DE"));
        assert_eq!(bcp47_for("osd"), None);
        // Every key is unique and matches the preferred tag.
        for l in LANGS {
            assert_eq!(lang_key(l.1), l.2, "{}", l.0);
            assert_eq!(tess_for_tag(l.1), Some(l.0));
        }
    }

    #[test]
    fn plan_exact_and_substitute() {
        let inst = v(&["de-DE", "fr-FR"]);
        let p = plan_languages("deu", &inst).unwrap();
        assert_eq!(p, LangPlan { tag: "de-DE".into(), primary: "deu".into(), substituted: vec![] });
        // eng missing, same script → via de-DE
        let p = plan_languages("deu+eng", &inst).unwrap();
        assert_eq!(p.tag, "de-DE");
        assert_eq!(p.substituted, vec!["eng".to_string()]);
        // first requested language with a pack wins
        let p = plan_languages("eng+fra+deu", &inst).unwrap();
        assert_eq!((p.tag.as_str(), p.primary.as_str()), ("fr-FR", "fra"));
        assert_eq!(p.substituted, vec!["eng".to_string()]);
        // region does not matter
        let p = plan_languages("eng", &v(&["en-GB"])).unwrap();
        assert_eq!(p.tag, "en-GB");
    }

    #[test]
    fn plan_errors_are_clear() {
        let inst = v(&["de-DE"]);
        let e = plan_languages("eng", &v(&[])).unwrap_err();
        assert!(e.contains("no language pack") && e.contains("eng (en-US)") && e.contains("Language.OCR~~~en-US"), "{e}");
        let e = plan_languages("rus", &inst).unwrap_err();
        assert!(e.contains("rus (ru-RU)") && e.contains("Installed: de-DE"), "{e}");
        let e = plan_languages("deu+rus", &inst).unwrap_err();
        assert!(e.contains("different script") && e.contains("rus"), "{e}");
        let e = plan_languages("deu+frk", &inst).unwrap_err();
        assert!(e.contains("does not support") && e.contains("frk"), "{e}");
        assert!(plan_languages("", &inst).is_err());
    }
}
