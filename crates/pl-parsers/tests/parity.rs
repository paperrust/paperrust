//! Parity tests against `tests/parity/pl-parsers/reference.json` (generated with
//! `tests/parity/pl-parsers/gen_reference.py` from upstream v3.4.2 + libmagic 5.48).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pl_parsers::mail::MailDocumentParser;
use pl_parsers::{DocumentParser, ParseContext, ParserRegistry, RegistryConfig, mime, pdf, text};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-parsers")
}

fn reference() -> Value {
    let p = dir().join("reference.json");
    let s = std::fs::read_to_string(&p).expect("reference.json (gen_reference.py ausführen)");
    serde_json::from_str(&s).expect("json")
}

fn ctx() -> ParseContext {
    ParseContext { time_zone: chrono_tz::Europe::Zurich, ..ParseContext::default() }
}

/// Known, documented deviations of the MIME detection (contract: Known differences).
const MIME_KNOWN_DIFF: &[&str] = &[
    // libmagic detects random bytes as a TGA image; we: application/octet-stream
    "binary_garbage.bin",
    // libmagic: application/octet-stream ("Microsoft OOXML", word/ too late); PaperRust detects DOCX
    // across all entries (Known difference 13, test log 2026-11-16 C-008)
    "office_late_word.docx",
    // libmagic: application/octet-stream (binary junk before %PDF-); PaperRust: PDF (Known difference
    // 14, C-073)
    "pdf_leading_junk.pdf",
];

/// Probes that PaperRust deliberately classifies differently from libmagic (contract pl-parsers:
/// Known differences; test log 2026-11-16 C-008): OOXML packages with `[Content_Types].xml` whose
/// `word/` part libmagic overlooks because of the entry order count as DOCX.
const MIME_PROBE_KNOWN_DIFF: &[(&str, &str)] = &[
    ("zip_ooxml_second_word", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("zip_ooxml_big_first", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
];

#[test]
fn mime_detection_matches_libmagic() {
    let r = reference();
    let mut bad = Vec::new();
    for (name, e) in r["files"].as_object().unwrap() {
        let got = mime::detect_file(&dir().join("samples").join(name)).unwrap();
        let want = e["mime"].as_str().unwrap();
        if got != want && !MIME_KNOWN_DIFF.contains(&name.as_str()) {
            bad.push(format!("{name}: got {got}, want {want}"));
        }
    }
    for (name, want) in r["mime_probes"].as_object().unwrap() {
        let got = mime::detect_file(&dir().join("samples/mime").join(name)).unwrap();
        let want = want.as_str().unwrap();
        if let Some((_, ours)) = MIME_PROBE_KNOWN_DIFF.iter().find(|(n, _)| *n == name.as_str()) {
            if got != *ours {
                bad.push(format!("probe {name}: got {got}, want (PaperRust) {ours}"));
            }
            continue;
        }
        if got != want {
            bad.push(format!("probe {name}: got {got}, want {want}"));
        }
    }
    assert!(bad.is_empty(), "MIME-Abweichungen:\n{}", bad.join("\n"));
}

#[test]
fn parser_selection_matches_upstream() {
    let r = reference();
    let mut bad = Vec::new();
    for (tika, key) in [(false, "parser_no_tika"), (true, "parser_tika")] {
        let reg = ParserRegistry::new(RegistryConfig { tika_enabled: tika, extended_formats: false });
        for (name, e) in r["files"].as_object().unwrap() {
            let m = e["mime"].as_str().unwrap();
            let got = reg.get_parser_for_file(m, name, None, true).map(|p| p.class_name());
            let want = e[key].as_str();
            if got != want {
                bad.push(format!("{key} {name} ({m}): got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn default_extensions_match_upstream() {
    let r = reference();
    let mut bad = Vec::new();
    for (tika, key) in [(false, "no_tika"), (true, "tika")] {
        let reg = ParserRegistry::new(RegistryConfig { tika_enabled: tika, extended_formats: false });
        for (m, want) in r["default_extension"][key].as_object().unwrap() {
            // Ignore Windows registry entries of the reference: compare with the Python default table
            let py_default = r["mimetypes_default_guess_extension"][m].as_str();
            let got = reg.get_default_file_extension(m);
            let want = want.as_str().unwrap();
            let want = if py_default.is_none() && reg.get_parser_for_file(m, "", None, true).is_none() { "" } else { want };
            if got != want {
                bad.push(format!("{key} {m}: got {got:?}, want {want:?}"));
            }
        }
    }
    // Supported extensions: parser extensions + Python default table (without OS tables)
    let reg = ParserRegistry::default();
    let mut want: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in reg.all_parsers() {
        for (m, ext) in p.supported_mime_types() {
            want.insert((*ext).to_owned());
            if let Some(list) = r["mimetypes_default_all_extensions"][*m].as_array() {
                for e in list {
                    want.insert(e.as_str().unwrap().to_owned());
                }
            }
        }
    }
    assert_eq!(reg.get_supported_file_extensions(), want);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn text_reading_matches_upstream() {
    let r = reference();
    let mut bad = Vec::new();
    for (name, e) in r["files"].as_object().unwrap() {
        let Some(want) = e["text_read"].as_str() else {
            continue;
        };
        let got = text::read_file_handle_unicode_errors(&dir().join("samples").join(name), &ctx()).unwrap();
        if got != want {
            bad.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn fake_tika(html: &str, _max: usize) -> pl_parsers::Result<(String, Option<pl_parsers::limits::Truncation>)> {
    Ok((format!("TIKAHTML{}", html.chars().count()), None))
}

#[test]
fn mail_matches_upstream() {
    let r = reference();
    let parser = MailDocumentParser { html_to_text: fake_tika, ..Default::default() };
    let c = ctx();
    let mut bad = Vec::new();
    for (name, e) in r["files"].as_object().unwrap() {
        let Some(m) = e.get("mail") else { continue };
        let path = dir().join("samples").join(name);
        match parser.parse(&path, "message/rfc822", &c) {
            Ok(doc) => {
                if Some(doc.text.as_str()) != m["text"].as_str() {
                    bad.push(format!("{name} text:\n got {:?}\nwant {:?}", doc.text, m["text"]));
                }
                let d = doc.date.map(|d| d.to_rfc3339());
                if d.as_deref() != m["date"].as_str() {
                    bad.push(format!("{name} date: got {d:?}, want {:?}", m["date"]));
                }
            }
            Err(err) => {
                let got = err.to_string().replace(&path.display().to_string(), "<path>");
                if Some(got.as_str()) != m["parse_error"].as_str() {
                    bad.push(format!("{name} error: got {got:?}, want {:?}", m["parse_error"]));
                }
            }
        }
        let md = parser.extract_metadata(&path, "message/rfc822", &c);
        let want_md: Vec<pl_parsers::MetadataEntry> = serde_json::from_value(m["metadata"].clone()).unwrap();
        if md != want_md {
            bad.push(format!("{name} metadata:\n got {md:?}\nwant {want_md:?}"));
        }
        if let Some(atts) = m.get("attachments").and_then(|a| a.as_array())
            && let Ok(mail) = MailDocumentParser::parse_file_to_message(&path, &c)
        {
            let got: Vec<(String, usize, String)> =
                mail.attachments().iter().map(|a| (a.filename.clone(), a.size(), a.content_id.clone())).collect();
            let want: Vec<(String, usize, String)> = atts
                .iter()
                .map(|a| {
                    (
                        a["filename"].as_str().unwrap().to_owned(),
                        a["size"].as_u64().unwrap() as usize,
                        a["content_id"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            if got != want {
                bad.push(format!("{name} attachments: got {got:?}, want {want:?}"));
            }
            if Some(mail.html().as_str()) != m["html"].as_str() {
                bad.push(format!("{name} html: got {:?}, want {:?}", mail.html(), m["html"]));
            }
            if Some(mail.text().as_str()) != m["plain"].as_str() {
                bad.push(format!("{name} plain: got {:?}, want {:?}", mail.text(), m["plain"]));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn pdf_matches_upstream() {
    let r = reference();
    let c = ctx();
    let mut bad = Vec::new();
    for (name, e) in r["files"].as_object().unwrap() {
        if e.get("pdf_page_count").is_none() {
            continue;
        }
        let path = dir().join("samples").join(name);
        let pc = pdf::get_page_count(&path, &c);
        if pc.map(u64::from) != e["pdf_page_count"].as_u64() {
            bad.push(format!("{name} page_count: got {pc:?}, want {:?}", e["pdf_page_count"]));
        }
        let tagged = pdf::is_tagged_pdf(&path, &c);
        if Some(tagged) != e["pdf_tagged"].as_bool() {
            bad.push(format!("{name} tagged: got {tagged}"));
        }
        let md = pdf::extract_pdf_metadata(&path, &c);
        let want: Vec<pl_parsers::MetadataEntry> = serde_json::from_value(e["pdf_metadata"].clone()).unwrap();
        if md != want {
            bad.push(format!("{name} metadata:\n got {md:?}\nwant {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Own expectations (no local Tika/pdftotext): text of the office/PDF samples.
#[test]
fn own_expectations_text() {
    let c = ctx();
    let reg = ParserRegistry::default();
    let mut got = BTreeMap::new();
    for name in [
        "office_word_order.docx",
        "office_lo_order.docx",
        "office.xlsx",
        "office.pptx",
        "office.ppsx",
        "office.odt",
        "office.ods",
        "office.odp",
        "office.odg",
        "office.rtf",
        "pdf_text.pdf",
    ] {
        let path = dir().join("samples").join(name);
        let m = mime::detect_file(&path).unwrap();
        let p = reg.get_parser_for_file(&m, name, Some(&path), true).expect(name);
        let doc = p.parse(&path, &m, &c).unwrap_or_else(|e| panic!("{name}: {e}"));
        got.insert(name, (doc.text, doc.date.map(|d| d.to_rfc3339())));
    }
    let docx = "Überschrift\nErster Absatz\tmit Tab\nund Umbruch\nZelle A1\tZelle B1\nZelle A2\tZelle B2\nText & Zeichen <ok> eingefuegt";
    assert_eq!(got["office_word_order.docx"].0, docx);
    assert_eq!(got["office_lo_order.docx"].0, docx);
    assert_eq!(got["office_word_order.docx"].1.as_deref(), Some("2025-01-06T08:00:00+00:00"));
    assert_eq!(got["office.xlsx"].0, "Umsatz\n\tMonat\tBetrag\n\tJänner\t1234.5\n\tFebruar\tinline\n\tTRUE\tFormel\n\nLeer & Zwei");
    assert_eq!(got["office.pptx"].0, "Folie Eins Titel\nErster Punkt & mehr\n\nFolie Zwei Titel\nZweiter Punkt");
    assert_eq!(got["office.ppsx"].0, got["office.pptx"].0);
    assert_eq!(got["office.odt"].0, "ODF Überschrift\nErster Absatz\tmit Tab\nUmbruch\nZweiter Absatz & mehr   Ende\nListenpunkt\nT1\tT2");
    assert_eq!(got["office.odt"].1.as_deref(), Some("2025-01-06T08:00:00+01:00"));
    assert_eq!(got["office.ods"].0, "Tabelle1\n\tMonat\t12,50\n\nZweite\n\twdh\twdh");
    assert_eq!(got["office.odp"].0, "Folie eins\nFolie zwei");
    assert_eq!(got["office.odg"].0, "Grafiktext");
    assert_eq!(got["office.rtf"].0, "Erster Absatz mit Umlaut: üä\nZweiter\tAbsatz mit fett und Unicode €.\nEnde\nZeile");
    assert!(got["pdf_text.pdf"].0.contains("born-digital PDF"), "{:?}", got["pdf_text.pdf"].0);
}

#[test]
fn robustness_limits() {
    let path = dir().join("samples/office_zipbomb.docx");
    let mut c = ctx();
    c.limits.max_entry_size = 1024 * 1024;
    let reg = ParserRegistry::default();
    let m = mime::detect_file(&path).unwrap();
    let p = reg.get_parser_for_file(&m, "x", None, true).unwrap();
    assert!(matches!(p.parse(&path, &m, &c), Err(pl_parsers::ParseError::LimitExceeded(_))));
    // Default limits: 20 MiB fit, text limit applies when smaller
    c.limits = pl_parsers::Limits { max_text_chars: 1000, ..pl_parsers::Limits::default() };
    assert!(p.parse(&path, &m, &c).is_err());
    // Broken files → error, no panic
    for name in ["office_truncated.docx", "office_not_zip.docx"] {
        let path = dir().join("samples").join(name);
        let res = p.parse(&path, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", &ctx());
        assert!(res.is_err(), "{name}");
        assert!(p.extract_metadata(&path, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", &ctx()).is_empty());
    }
    for name in ["pdf_truncated.pdf", "pdf_garbage.pdf", "pdf_garbage_binary.pdf", "pdf_encrypted_password.pdf"] {
        let path = dir().join("samples").join(name);
        let _ = pdf::pdf_born_digital_text(&path, &ctx());
        let _ = pdf::extract_pdf_metadata(&path, &ctx());
    }
}

/// "Does the PDF already have text?" – own expectations (upstream uses pdftotext, not available locally).
#[test]
fn pdf_born_digital_decisions() {
    let c = ctx();
    let cases = [
        ("pdf_text.pdf", true),
        ("pdf_xmp.pdf", true),
        ("pdf_short_text.pdf", false),
        ("pdf_tagged.pdf", true),
        ("pdf_empty_page.pdf", false),
        ("pdf_multipage.pdf", false),
        ("pdf_encrypted_empty_user.pdf", true),
        ("pdf_encrypted_password.pdf", false),
        ("pdf_garbage_binary.pdf", false),
    ];
    for (name, want) in cases {
        let (text, bd) = pdf::pdf_born_digital_text(&dir().join("samples").join(name), &c);
        assert_eq!(bd, want, "{name}: {text:?}");
    }
    let (text, _) = pdf::pdf_born_digital_text(&dir().join("samples/pdf_multipage.pdf"), &c);
    assert_eq!(
        text.as_deref().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).as_deref(),
        Some("Seite eins Seite zwei Seite drei")
    );
}
