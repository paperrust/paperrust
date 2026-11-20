//! Gaps E9/E10/E12: slides from PPTX/ODP, slide HTML, mail → PDF with a test generator.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pl_parsers::office::{PdfConverter, Slide, slides, slides_html};
use pl_parsers::{ParseContext, ParserRegistry};

fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in entries {
        let off = v.len() as u32;
        v.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(data);
        cd.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0u8; 12]);
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = v.len() as u32;
    let n = entries.len() as u16;
    v.extend_from_slice(&cd);
    v.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    v.extend_from_slice(&cd_off.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v
}

const PPTX: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
const ODP: &str = "application/vnd.oasis.opendocument.presentation";

fn pptx() -> Vec<u8> {
    let rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#;
    let pres = br#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
    let pres_rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#;
    let slide = |title: &str, body: &str| {
        format!(
            r#"<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p><a:p><a:r><a:t>zweite Zeile</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
        .into_bytes()
    };
    let s1 = slide("Erste &amp; beste", "Inhalt eins");
    let s2 = slide("Zweite", "Inhalt zwei");
    stored_zip(&[
        ("_rels/.rels", rels),
        ("ppt/presentation.xml", pres),
        ("ppt/_rels/presentation.xml.rels", pres_rels),
        ("ppt/slides/slide1.xml", &s1),
        ("ppt/slides/slide2.xml", &s2),
    ])
}

fn odp() -> Vec<u8> {
    let content = br#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0"><office:body><office:presentation><draw:page draw:name="p1"><draw:frame presentation:class="title"><draw:text-box><text:p>Titel Eins</text:p></draw:text-box></draw:frame><draw:frame presentation:class="outline"><draw:text-box><text:list><text:list-item><text:p>Punkt<text:s text:c="2"/>A</text:p></text:list-item></text:list></draw:text-box></draw:frame><presentation:notes><draw:frame><draw:text-box><text:p>Sprechernotiz</text:p></draw:text-box></draw:frame></presentation:notes></draw:page><draw:page draw:name="p2"><draw:frame><draw:text-box><text:p>Nur Text</text:p></draw:text-box></draw:frame></draw:page></office:presentation></office:body></office:document-content>"#;
    stored_zip(&[("mimetype", ODP.as_bytes()), ("content.xml", content)])
}

#[test]
fn pptx_slides_in_presentation_order_with_titles() {
    let s = slides(&pptx(), PPTX, &ParseContext::default()).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].title.as_deref(), Some("Zweite"));
    assert_eq!(s[1].title.as_deref(), Some("Erste & beste"));
    assert_eq!(s[1].lines, vec!["Inhalt eins", "zweite Zeile"]);
}

#[test]
fn odp_slides_with_title_and_notes() {
    let s = slides(&odp(), ODP, &ParseContext::default()).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].title.as_deref(), Some("Titel Eins"));
    assert_eq!(s[0].lines, vec!["Punkt  A"]);
    assert_eq!(s[0].notes, vec!["Sprechernotiz"]);
    assert_eq!(s[1].title, None);
    assert_eq!(s[1].lines, vec!["Nur Text"]);
}

#[test]
fn slides_html_one_page_per_slide() {
    let h = slides_html(&[
        Slide { title: Some("A <b>".into()), lines: vec!["x".into()], notes: vec![] },
        Slide { title: None, lines: vec!["y".into()], notes: vec!["n".into()] },
    ]);
    assert!(h.contains("<h1>A &lt;b&gt;</h1>"), "{h}");
    assert_eq!(h.matches("page-break-before:always").count(), 1, "{h}");
    assert!(h.contains("1 / 2") && h.contains("2 / 2"));
    assert!(h.contains("<i>Notes</i>"));
}

#[test]
fn legacy_office_now_parsed() {
    let r = ParserRegistry::default();
    for m in ["application/msword", "application/vnd.ms-excel", "application/vnd.ms-powerpoint"] {
        let p = r.get_parser_for_file(m, "a", None, true).unwrap();
        assert_eq!(p.class_name(), "TikaDocumentParser", "{m}");
    }
}

/// Test generator: writes a one-page PDF per call and remembers (MIME, input).
#[derive(Default)]
struct FakePdf {
    calls: Mutex<Vec<(String, String)>>,
}

fn tiny_pdf(label: &str) -> Vec<u8> {
    use lopdf::{Dictionary, Document, Object, Stream};
    let mut d = Document::with_version("1.5");
    let pages_id = d.new_object_id();
    let content = d.add_object(Stream::new(Dictionary::new(), format!("% {label}").into_bytes()));
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", vec![0.into(), 0.into(), 595.into(), 842.into()]);
    page.set("Contents", Object::Reference(content));
    let page_id = d.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", vec![Object::Reference(page_id)]);
    pages.set("Count", 1);
    d.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    let cat_id = d.add_object(cat);
    d.trailer.set("Root", Object::Reference(cat_id));
    let mut v = Vec::new();
    d.save_to(&mut v).unwrap();
    v
}

impl PdfConverter for FakePdf {
    fn convert(&self, path: &Path, mime_type: &str, out_dir: &Path) -> pl_parsers::Result<PathBuf> {
        let input = std::fs::read_to_string(path).unwrap_or_default();
        self.calls.lock().unwrap().push((mime_type.to_owned(), input));
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let out = out_dir.join(format!("{stem}.pdf"));
        std::fs::write(&out, tiny_pdf(&stem)).unwrap();
        Ok(out)
    }
}

const MAIL: &str = "From: Erika Muster <erika@example.com>\r\nTo: max@example.com\r\nSubject: Rechnung <42>\r\nDate: Tue, 02 Jan 2024 10:00:00 +0100\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHallo Max,\r\nsiehe https://example.com/r.\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hallo <b>Max</b></p><script>alert(1)</script>\r\n--b--\r\n";

fn mail_pages(layout: Option<i64>) -> (Vec<String>, Vec<(String, String)>) {
    let t = tempfile::tempdir().unwrap();
    let eml = t.path().join("mail.eml");
    std::fs::write(&eml, MAIL).unwrap();
    let fake = Arc::new(FakePdf::default());
    let mut r = ParserRegistry::default();
    r.set_mail_converter(Some(fake.clone()));
    let p = r.get_parser_for_file("message/rfc822", "mail.eml", None, true).unwrap();
    assert!(p.requires_pdf_rendition());
    let work = t.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let ctx = ParseContext { mail_pdf_layout: layout, work_dir: Some(work.clone()), ..Default::default() };
    let d = p.parse(&eml, "message/rfc822", &ctx).unwrap();
    let archive = d.archive_path.expect("archive");
    assert_eq!(archive, work.join("merged.pdf"), "generated files go to the work dir, not next to the original");
    let doc = lopdf::Document::load(&archive).unwrap();
    let labels: Vec<String> = doc
        .get_pages()
        .values()
        .map(|id| String::from_utf8_lossy(&doc.get_page_content(*id)).trim_start_matches("% ").trim().to_string())
        .collect();
    let calls = fake.calls.lock().unwrap().clone();
    (labels, calls)
}

#[test]
fn mail_pdf_layouts() {
    let (pages, calls) = mail_pages(None);
    assert_eq!(pages, vec!["email_as_html", "index"]);
    assert_eq!(calls[0].0, "text/html");
    assert!(calls[0].1.contains("Rechnung &lt;42&gt;"), "{}", calls[0].1);
    assert!(calls[0].1.contains("Erika Muster &lt;erika@example.com&gt;") || calls[0].1.contains("erika@example.com"));
    assert!(calls[0].1.contains("2024-01-02 09:00"), "UTC date expected: {}", calls[0].1);
    assert!(calls[0].1.contains("<a href=\"https://example.com/r\""), "{}", calls[0].1);
    assert_eq!(calls[1].0, "multipart/related");
    assert!(calls[1].1.contains("Content-Type: multipart/related"));
    assert_eq!(mail_pages(Some(2)).0, vec!["index", "email_as_html"]);
    assert_eq!(mail_pages(Some(3)).0, vec!["index"]);
    assert_eq!(mail_pages(Some(4)).0, vec!["email_as_html"]);
    assert_eq!(mail_pages(Some(0)).0, vec!["email_as_html", "index"]);
}

#[test]
fn mail_without_converter_has_no_archive() {
    let t = tempfile::tempdir().unwrap();
    let eml = t.path().join("mail.eml");
    std::fs::write(&eml, MAIL).unwrap();
    let r = ParserRegistry::default();
    let p = r.get_parser_for_file("message/rfc822", "mail.eml", None, true).unwrap();
    assert!(!p.requires_pdf_rendition());
    assert!(p.parse(&eml, "message/rfc822", &ParseContext::default()).unwrap().archive_path.is_none());
}
