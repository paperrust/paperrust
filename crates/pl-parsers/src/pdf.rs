//! PDF: embedded text ("does the PDF already have text?"), page count, tagged check and
//! XMP metadata like upstream. OCR is not part of this crate.
//!
//! upstream: src/paperless/parsers/utils.py@v3.4.2 (PDF_TEXT_MIN_LENGTH, is_tagged_pdf,
//! extract_pdf_text [pdftotext -layout], post_process_text, is_born_digital_text,
//! pdf_born_digital_text, get_page_count_for_pdf [pikepdf], extract_pdf_metadata [pikepdf
//! `open_metadata().items()`]), src/paperless/parsers/tesseract.py@v3.4.2
//! (RasterisedDocumentParser: MIME list, get_page_count, extract_metadata).
//! pikepdf 10.13 `models/metadata/_xmp.py` (XmpDocument._get_elements/_get_subelements,
//! DEFAULT_NAMESPACES).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use lopdf::{Document, LoadOptions, Object};

use crate::xmlutil::{Elem, parse_dom};
use crate::{DocumentParser, Limits, MetadataEntry, ParseContext, ParseError, ParsedDocument, Result, read_limited};

/// Minimum length for "born-digital" (upstream `PDF_TEXT_MIN_LENGTH`).
pub const PDF_TEXT_MIN_LENGTH: usize = 50;

/// pikepdf `DEFAULT_NAMESPACES` (URI → prefix).
pub const XMP_NAMESPACES: &[(&str, &str)] = &[
    ("adobe:ns:meta/", "x"),
    ("http://purl.org/dc/elements/1.1/", "dc"),
    ("http://ns.adobe.com/pdf/1.3/", "pdf"),
    ("http://www.aiim.org/pdfa/ns/id/", "pdfaid"),
    ("http://www.aiim.org/pdfa/ns/extension/", "pdfaExtension"),
    ("http://www.aiim.org/pdfa/ns/property#", "pdfaProperty"),
    ("http://www.aiim.org/pdfa/ns/schema#", "pdfaSchema"),
    ("http://www.aiim.org/pdfua/ns/id/", "pdfuaid"),
    ("http://www.npes.org/pdfx/ns/id/", "pdfxid"),
    ("http://ns.adobe.com/photoshop/1.0/", "photoshop"),
    ("http://prismstandard.org/namespaces/basic/1.0/", "prism"),
    ("http://prismstandard.org/namespaces/basic/2.0/", "prism2"),
    ("http://prismstandard.org/namespaces/basic/3.0/", "prism3"),
    ("http://www.w3.org/1999/02/22-rdf-syntax-ns#", "rdf"),
    ("http://ns.adobe.com/xap/1.0/", "xmp"),
    ("http://ns.adobe.com/xap/1.0/mm/", "xmpMM"),
    ("http://ns.adobe.com/xap/1.0/rights/", "xmpRights"),
    ("http://crossref.org/crossmark/1.0/", "crossmark"),
    ("http://www.niso.org/schemas/jav/1.0/", "jav"),
    ("http://ns.adobe.com/pdfx/1.3/", "pdfx"),
    ("http://www.niso.org/schemas/ali/1.0/", "ali"),
];

const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Loads a PDF (lenient, with repair; junk before `%PDF-` is skipped).
/// Panics of the library are caught. `None` if unreadable or password-protected.
pub fn load_document(data: &[u8], limits: &Limits) -> Option<Document> {
    let start = data.windows(5).take(1024).position(|w| w == b"%PDF-").unwrap_or(0);
    let body = &data[start..];
    let opts = LoadOptions { max_decompressed_size: Some(limits.max_entry_size.min(usize::MAX as u64) as usize), ..LoadOptions::default() };
    let res = catch_unwind(AssertUnwindSafe(|| Document::load_mem_with_options(body, opts)));
    match res {
        Ok(Ok(doc)) => {
            if doc.is_encrypted() && doc.encryption_state.is_none() {
                return None;
            }
            Some(doc)
        }
        _ => None,
    }
}

fn load_path(path: &Path, ctx: &ParseContext) -> Option<Document> {
    let data = read_limited(path, &ctx.limits).ok()?;
    load_document(&data, &ctx.limits)
}

/// `get_page_count_for_pdf`.
pub fn get_page_count(path: &Path, ctx: &ParseContext) -> Option<u32> {
    match load_path(path, ctx) {
        Some(doc) => {
            let n = catch_unwind(AssertUnwindSafe(|| doc.get_pages().len())).ok()?;
            u32::try_from(n).ok()
        }
        None => {
            tracing::warn!(target: "paperless.parsers.utils", "Unable to determine PDF page count for {}", path.display());
            None
        }
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

/// `is_tagged_pdf`: `/MarkInfo /Marked true` in the catalog.
pub fn is_tagged(doc: &Document) -> bool {
    let Ok(cat) = doc.catalog() else { return false };
    let Ok(mi) = cat.get(b"MarkInfo") else {
        return false;
    };
    let Some(Object::Dictionary(d)) = deref(doc, mi) else {
        return false;
    };
    match d.get(b"Marked").ok().and_then(|o| deref(doc, o)) {
        Some(Object::Boolean(b)) => *b,
        // bool(pikepdf object): number ≠ 0 / non-empty name counts as true
        Some(Object::Integer(i)) => *i != 0,
        Some(Object::Null) | None => false,
        Some(_) => true,
    }
}

pub fn is_tagged_pdf(path: &Path, ctx: &ParseContext) -> bool {
    load_path(path, ctx).is_some_and(|d| is_tagged(&d))
}

/// Text of all pages (replacement for `pdftotext -layout`; pages separated by form feed).
/// Reading order via text positions ([`crate::pdftext`], gap E14); if a page is not
/// readable that way, lopdf's simple text is used.
pub fn extract_text_from(doc: &Document, limits: &Limits) -> Option<String> {
    let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().take(limits.max_pdf_pages as usize).collect();
    let deadline = crate::Deadline::from_limits(limits);
    let limit = limits.max_entry_size.min(usize::MAX as u64) as usize;
    let mut out = String::new();
    for (p, id) in pages {
        if deadline.check().is_err() {
            tracing::warn!(target: "paperless.parsers.utils", "PDF text extraction timed out");
            break;
        }
        let r = catch_unwind(AssertUnwindSafe(|| match crate::pdftext::page_text(doc, id, limit) {
            Some(t) => Ok(t),
            None => doc.extract_text_with_limit(&[p], limit),
        }));
        if let Ok(Ok(t)) = r {
            if out.len() + t.len() > limits.max_text_chars {
                break;
            }
            out.push_str(&t);
        }
        out.push('\x0c');
    }
    let out = out.replace('\0', "");
    if out.is_empty() { None } else { Some(out) }
}

/// `extract_pdf_text`.
pub fn extract_pdf_text(path: &Path, ctx: &ParseContext) -> Option<String> {
    let doc = load_path(path, ctx)?;
    extract_text_from(&doc, &ctx.limits)
}

fn is_ws_not_nl(c: char) -> bool {
    crate::pystr::py_isspace(c) && c != '\r' && c != '\n'
}

/// `post_process_text`.
pub fn post_process_text(text: Option<&str>) -> Option<String> {
    let text = text?;
    if text.is_empty() {
        return None;
    }
    // 1. ([^\S\r\n]+) -> " "
    let mut a = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if is_ws_not_nl(c) {
            if !in_ws {
                a.push(' ');
                in_ws = true;
            }
        } else {
            a.push(c);
            in_ws = false;
        }
    }
    // 2. ([\n\r]+)([^\S\n\r]+) -> \1
    let mut b = String::with_capacity(a.len());
    let mut prev_nl = false;
    let chars = a.chars().peekable();
    for c in chars {
        if c == ' ' && prev_nl {
            continue;
        }
        prev_nl = c == '\n' || c == '\r';
        b.push(c);
    }
    // 3. ([^\S\n\r]+)$ -> "" ($ = end or before trailing \n)
    let mut c = b;
    if c.ends_with(' ') {
        let t = c.trim_end_matches(' ').len();
        c.truncate(t);
    } else if c.ends_with('\n') {
        let body = &c[..c.len() - 1];
        if body.ends_with(' ') {
            let t = body.trim_end_matches(' ').len();
            c = format!("{}\n", &body[..t]);
        }
    }
    let result = crate::pystr::py_strip(&c).replace('\0', " ");
    if result.is_empty() { None } else { Some(result) }
}

/// `is_born_digital_text`.
pub fn is_born_digital_text(text: Option<&str>, tagged: bool) -> bool {
    match text {
        None | Some("") => false,
        Some(t) => tagged || t.chars().count() > PDF_TEXT_MIN_LENGTH,
    }
}

/// `pdf_born_digital_text`: (normalized text, born-digital?).
pub fn pdf_born_digital_text(path: &Path, ctx: &ParseContext) -> (Option<String>, bool) {
    let Some(doc) = load_path(path, ctx) else {
        return (None, false);
    };
    let text = post_process_text(extract_text_from(&doc, &ctx.limits).as_deref());
    let bd = text.is_some() && is_born_digital_text(text.as_deref(), is_tagged(&doc));
    (text, bd)
}

// ---------------------------------------------------------------------------
// XMP
// ---------------------------------------------------------------------------

fn xmp_bytes(doc: &Document, limits: &Limits) -> Option<Vec<u8>> {
    let cat = doc.catalog().ok()?;
    let m = cat.get(b"Metadata").ok()?;
    match deref(doc, m)? {
        Object::Stream(s) => {
            let lim = limits.max_entry_size.min(usize::MAX as u64) as usize;
            if s.dict.get(b"Filter").is_ok() {
                s.decompressed_content_with_limit(lim).ok().or_else(|| Some(s.content.clone()))
            } else {
                Some(s.content.clone())
            }
        }
        _ => None,
    }
}

/// Python `repr(str)`.
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[derive(Debug, Clone)]
enum XmpVal {
    Text(String),
    List(Vec<Option<String>>),
    Set(Vec<Option<String>>),
}

impl XmpVal {
    fn to_py_string(&self) -> String {
        match self {
            XmpVal::Text(s) => s.clone(),
            XmpVal::List(v) => v.iter().map(|e| e.clone().unwrap_or_else(|| "None".into())).collect::<Vec<_>>().join(" "),
            XmpVal::Set(v) => {
                if v.is_empty() {
                    "set()".into()
                } else {
                    let items: Vec<String> = v.iter().map(|e| e.as_deref().map(py_repr).unwrap_or_else(|| "None".into())).collect();
                    format!("{{{}}}", items.join(", "))
                }
            }
        }
    }
}

fn subelements(node: &Elem) -> XmpVal {
    if let Some(alt) = node.find(NS_RDF, "Alt") {
        return XmpVal::Text(
            alt.children
                .first()
                .and_then(|c| c.text.clone())
                .unwrap_or_else(|| if alt.children.is_empty() { String::new() } else { "None".into() }),
        );
    }
    if let Some(bag) = node.find(NS_RDF, "Bag") {
        let mut v: Vec<Option<String>> = Vec::new();
        for it in &bag.children {
            if !v.contains(&it.text) {
                v.push(it.text.clone());
            }
        }
        return XmpVal::Set(v);
    }
    if let Some(seq) = node.find(NS_RDF, "Seq") {
        return XmpVal::List(seq.children.iter().map(|c| c.text.clone()).collect());
    }
    XmpVal::Text(String::new())
}

fn elem_value(node: &Elem) -> XmpVal {
    if let Some(t) = &node.text
        && !crate::pystr::py_strip(t).is_empty()
    {
        return XmpVal::Text(t.clone());
    }
    subelements(node)
}

/// pikepdf `PdfMetadata.items()` → upstream `extract_pdf_metadata`.
pub fn xmp_entries(xmp: &[u8], limits: &Limits) -> Vec<MetadataEntry> {
    if crate::pystr::py_strip(&String::from_utf8_lossy(xmp)).is_empty() {
        return Vec::new();
    }
    let root = match parse_dom(xmp, limits.max_depth, 200_000) {
        Ok(r) => r,
        Err(_) => {
            // pikepdf: second attempt without illegal bytes, otherwise empty XMP
            let cleaned: Vec<u8> = xmp.iter().copied().filter(|&b| matches!(b, 0x09 | 0x0a | 0x0d | 0x20..=0xff)).collect();
            match parse_dom(&cleaned, limits.max_depth, 200_000) {
                Ok(r) => r,
                Err(_) => return Vec::new(),
            }
        }
    };
    let Some(rdf) = root.find_descendant(NS_RDF, "RDF") else {
        return Vec::new();
    };
    let descs: Vec<&Elem> = rdf
        .children
        .iter()
        .filter(|d| {
            d.is(NS_RDF, "Description")
                && d.attrs.iter().any(|a| a.ns.as_deref() == Some(NS_RDF) && a.local == "about" && a.value.is_empty())
        })
        .collect();
    // Keys in iteration order (attributes, then child elements per Description)
    let mut keys: Vec<(Option<String>, String)> = Vec::new();
    for d in &descs {
        for a in &d.attrs {
            if a.ns.as_deref() == Some(NS_RDF) {
                continue;
            }
            keys.push((a.ns.clone(), a.local.clone()));
        }
        for c in &d.children {
            keys.push((c.ns.clone(), c.local.clone()));
        }
    }
    let lookup = |ns: &Option<String>, local: &str| -> Option<XmpVal> {
        for d in &descs {
            if let Some(a) = d.attrs.iter().find(|a| &a.ns == ns && a.local == local) {
                return Some(XmpVal::Text(a.value.clone()));
            }
            if let Some(c) = d.children.iter().find(|c| &c.ns == ns && c.local == local) {
                return Some(elem_value(c));
            }
        }
        None
    };
    let mut out = Vec::new();
    for (ns, local) in keys {
        let Some(ns_uri) = ns.clone() else { continue };
        let Some(val) = lookup(&ns, &local) else {
            continue;
        };
        let Some((_, prefix)) = XMP_NAMESPACES.iter().find(|(u, _)| *u == ns_uri) else {
            tracing::warn!(target: "paperless.parsers.utils", "Error reading metadata key {{{ns_uri}}}{local}: unknown namespace");
            continue;
        };
        out.push(MetadataEntry::new(ns_uri.clone(), *prefix, local, val.to_py_string()));
    }
    out
}

/// `extract_pdf_metadata`.
pub fn extract_pdf_metadata(path: &Path, ctx: &ParseContext) -> Vec<MetadataEntry> {
    let Some(doc) = load_path(path, ctx) else {
        tracing::warn!(target: "paperless.parsers.utils", "Could not open PDF metadata for {}", path.display());
        return Vec::new();
    };
    match xmp_bytes(&doc, &ctx.limits) {
        Some(x) => xmp_entries(&x, &ctx.limits),
        None => Vec::new(),
    }
}

pub const RASTER_MIME_TYPES: &[(&str, &str)] = &[
    ("application/pdf", ".pdf"),
    ("image/jpeg", ".jpg"),
    ("image/png", ".png"),
    ("image/tiff", ".tif"),
    ("image/gif", ".gif"),
    ("image/bmp", ".bmp"),
    ("image/webp", ".webp"),
    ("image/heic", ".heic"),
];

/// upstream `RasterisedDocumentParser` without OCR: PDF with its own text (born-digital) yields
/// that text; everything else needs OCR → [`ParseError::OcrRequired`].
#[derive(Debug, Default)]
pub struct RasterisedDocumentParser;

impl DocumentParser for RasterisedDocumentParser {
    fn class_name(&self) -> &'static str {
        "RasterisedDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust Tesseract OCR Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        RASTER_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _f: &str, _p: Option<&Path>) -> Option<i32> {
        RASTER_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        true
    }
    fn requires_pdf_rendition(&self) -> bool {
        false
    }
    fn parse(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        if mime_type != "application/pdf" {
            return Err(ParseError::OcrRequired);
        }
        let (text, born_digital) = pdf_born_digital_text(path, ctx);
        if !born_digital {
            return Err(ParseError::OcrRequired);
        }
        Ok(ParsedDocument { text: text.unwrap_or_default(), date: None, archive_path: None })
    }
    fn page_count(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Option<u32> {
        if mime_type == "application/pdf" { get_page_count(path, ctx) } else { None }
    }
    fn extract_metadata(&self, path: &Path, mime_type: &str, ctx: &ParseContext) -> Vec<MetadataEntry> {
        if mime_type != "application/pdf" {
            return Vec::new();
        }
        extract_pdf_metadata(path, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_process() {
        // Python: `$` without MULTILINE matches only the end or before the last "\n"
        assert_eq!(post_process_text(Some("  a   b \n   c  \n")).as_deref(), Some("a b \nc"));
        assert_eq!(post_process_text(Some(" \x0c \n ")), None);
        // Values from upstream post_process_text (reference/venv)
        assert_eq!(post_process_text(Some("a \n")).as_deref(), Some("a"));
        assert_eq!(post_process_text(Some("x\r\n  y \r\n")).as_deref(), Some("x\r\ny"));
        assert_eq!(post_process_text(Some("\x0c a\x0cb ")).as_deref(), Some("a b"));
        assert_eq!(post_process_text(Some("a\u{a0}\u{a0}b\n\u{2028} c")).as_deref(), Some("a b\nc"));
        assert_eq!(post_process_text(None), None);
        assert_eq!(post_process_text(Some("a\0b")).as_deref(), Some("a b"));
    }

    #[test]
    fn born_digital() {
        assert!(!is_born_digital_text(None, true));
        assert!(is_born_digital_text(Some("kurz"), true));
        assert!(!is_born_digital_text(Some("kurz"), false));
        assert!(is_born_digital_text(Some(&"x".repeat(51)), false));
        assert!(!is_born_digital_text(Some(&"x".repeat(50)), false));
    }

    #[test]
    fn xmp_values() {
        let x = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:pdf="http://ns.adobe.com/pdf/1.3/" pdf:Producer="P" xmlns:foo="urn:foo" foo:x="1">
<dc:title><rdf:Alt><rdf:li xml:lang="x-default">T</rdf:li></rdf:Alt></dc:title>
<dc:creator><rdf:Seq><rdf:li>A</rdf:li><rdf:li>B</rdf:li></rdf:Seq></dc:creator>
<dc:subject><rdf:Bag><rdf:li>k1</rdf:li></rdf:Bag></dc:subject>
<dc:format>application/pdf</dc:format>
</rdf:Description><rdf:Description rdf:about="uuid:x" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:rights>R</dc:rights></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let e = xmp_entries(x, &Limits::default());
        let kv: Vec<(String, String)> = e.iter().map(|m| (format!("{}:{}", m.prefix, m.key), m.value.clone())).collect();
        assert_eq!(
            kv,
            vec![
                ("pdf:Producer".into(), "P".into()),
                ("dc:title".into(), "T".into()),
                ("dc:creator".into(), "A B".into()),
                ("dc:subject".into(), "{'k1'}".into()),
                ("dc:format".into(), "application/pdf".into()),
            ]
        );
    }
}
